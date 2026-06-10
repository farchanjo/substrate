---
status: accepted
accepted_date: 2026-05-24
date: 2026-05-24
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0054 — Subprocess stdout/stderr Stream Multiplex

## Context and Problem Statement

[ADR-0052](0052-subprocess-execution-architecture.md) introduces subprocess
execution as a new bounded context with job-backed execution (Bucket E). Unlike
archive or filesystem jobs, a subprocess produces two continuous byte streams
(stdout and stderr) that must be delivered to the MCP client in near-real-time
while the process is running.

The job control-plane from [ADR-0040](0040-async-job-control-plane.md) provides
a `notifications/progress` push channel with a 250 ms / 1 pct throttle designed
for progress percentages. That throttle is inappropriate for stream delivery:
a subprocess writing log lines at high frequency needs a lower-latency flush
cadence, and the progress percentage model (0–100) does not map cleanly to an
unbounded byte stream.

The question is: how should stdout and stderr bytes be chunked, buffered,
and pushed to the client via the MCP notification channel, and how should
backpressure and aggregate retention be handled?

## Decision Drivers

- The MCP STDIO transport serializes all frames on a single channel;
  stream events must not starve other tool calls.
- Backpressure from a slow MCP client must not block the child process from
  writing to its own stdout/stderr (which would cause the child to hang on a
  full pipe buffer).
- The `notifications/progress` schema from MCP 2025-11-25 is the only push
  mechanism available; stream chunks must fit within its payload model.
- Binary subprocess output (non-UTF-8) must be representable; base64 encoding
  is the chosen transport encoding.
- The audit trail from [ADR-0038](0038-audit-event-semantics.md) must capture
  dropped chunks as observable events.
- The aggregate result for `subprocess.result` must be bounded to prevent
  unbounded memory accumulation.

## Considered Options

- Option A: Collect all stdout/stderr into a file; deliver the aggregate only
  on completion.
- Option B: Bounded mpsc channel per stream, chunk-driven flushing with an
  independent cadence from ADR-0040 throttling (selected).
- Option C: WebSocket or SSE side-channel for streaming (rejected: substrate
  is STDIO-only per ADR-0005).
- Option D: Merge stdout and stderr into a single stream, interleaved by
  arrival order.

## Decision Outcome

Chosen option: "Option B — bounded mpsc channel per stream with independent
flush cadence", because it decouples the subprocess writer from the MCP push
channel via buffering, prevents client slowness from stalling the child, and
extends the existing job notification model with minimal protocol surface.

Option D (merged stream) is rejected: callers cannot separate stdout from stderr
post-hoc; the streams carry distinct semantic meaning (log lines vs error lines).

### Stream Chunk Payload

Each stream notification extends the `ProgressEvent` shape from
[ADR-0040](0040-async-job-control-plane.md) with the following additional fields:

```
{
  "progressToken": "<job_id>",
  "progress": <bytes_emitted_mod_100>,
  "total": null,
  "job_id": "<uuid7>",
  "job_state": "Running",
  "stream": "stdout" | "stderr",
  "chunk_base64": "<base64-encoded bytes, max 4 KiB decoded>",
  "seq": <u64 monotonic, per-job, per-stream>,
  "chunk_bytes": <u32 decoded byte count>
}
```

`progress` is `bytes_emitted % 100` (a proxy metric to satisfy the MCP
`notifications/progress` schema; it does not represent percent-to-completion).
`total` is always null because the stream length is unknown.

The `seq` counter is a per-job per-stream `AtomicU64`, reset to zero at spawn.
Clients use `seq` to detect dropped chunks and to reorder chunks if delivery is
out of order (which is not expected over STDIO but is defensively handled).

The 250 ms / 1 pct progress throttle from ADR-0040 does NOT apply to stream
events. Stream events have their own flush trigger described below.

### Tokio Task and Channel Architecture

The following sequence diagram shows the interactions between client, MCP server,
the worker task, and the stream channels for a typical subprocess execution.

```mermaid
sequenceDiagram
    participant Client
    participant MCPServer as MCP Server
    participant Worker as Worker Task
    participant StdoutChan as stdout mpsc:64
    participant StderrChan as stderr mpsc:64
    participant Dispatcher as Dispatcher Task

    Client->>MCPServer: subprocess.spawn (confirmed)
    MCPServer->>Worker: spawn ChildHandle
    Worker->>StdoutChan: create mpsc channel bounded 64
    Worker->>StderrChan: create mpsc channel bounded 64
    Worker->>Dispatcher: spawn dispatcher task
    Note over Worker: reads tokio ChildStdout into 4 KiB chunks
    Worker->>StdoutChan: try_send StreamChunk
    Worker->>StdoutChan: try_send StreamChunk
    Dispatcher->>StdoutChan: recv chunk
    Dispatcher->>MCPServer: notifications/progress stream=stdout seq=0
    MCPServer-->>Client: notifications/progress
    Worker->>StderrChan: try_send StreamChunk
    Dispatcher->>StderrChan: recv chunk
    Dispatcher->>MCPServer: notifications/progress stream=stderr seq=0
    MCPServer-->>Client: notifications/progress
    Note over Worker: child exits exit_code=0
    Worker->>StdoutChan: close channel
    Worker->>StderrChan: close channel
    Dispatcher->>MCPServer: notifications/progress job_state=Succeeded exit_code=0 seq=terminal
    MCPServer-->>Client: notifications/progress terminal
    Client->>MCPServer: subprocess.result(job_id)
    MCPServer-->>Client: result exit_code stdout_aggregate_base64 stderr_aggregate_base64
```

**Reader tasks**: two tokio tasks (one for stdout, one for stderr) are spawned
when the `ChildHandle` is created. Each task calls `tokio::io::AsyncReadExt::read_buf`
in a loop into a 4 KiB `BytesMut` buffer. When the buffer is full or the flush
timer fires (see Flush Trigger), the task calls `mpsc::Sender::try_send`. When
the child closes the stream (EOF), the task sends a sentinel chunk with zero
bytes and closes its end of the mpsc channel.

**Dispatcher task**: a single tokio task per job drains both mpsc channels using
`tokio::select!` over both receivers. It emits `notifications/progress` for each
received chunk in the order they are dequeued. The dispatcher task is the only
entity that calls `notifications/progress`; reader tasks never do so directly.

### Flush Trigger

A chunk is flushed (the mpsc `try_send` is called) when either of the following
conditions is true:

- The 4 KiB buffer is full (chunk boundary).
- 100 ms have elapsed since the last `try_send` for this stream (time-based flush).

The 100 ms timer is implemented as `tokio::time::interval(Duration::from_millis(100))`
raced against the read future using `tokio::select! biased` with the read arm
first (per the biased-select rule in [ADR-0037](0037-async-cancellation-patterns.md)).

### Backpressure

When `mpsc::Sender::try_send` returns `Err::Full` (the channel holds 64 unread
chunks and the dispatcher has not drained them), the chunk is dropped and:

- The process-global `AtomicU64` counter `SUBSTRATE_STREAM_CHUNKS_DROPPED` is
  incremented.
- An audit event `SUBSTRATE_STREAM_CHUNK_DROPPED` is emitted with payload
  `{job_id, stream, dropped_bytes, seq}`.
- The reader task does NOT block or slow down. The child process is not affected
  because the pipe buffer between substrate and the child is independent of the
  mpsc channel.

Operators and agents can observe `stream_chunks_dropped` in the terminal job
result (see Result Shape below) and in the audit log.

### Aggregate Retention Ring Buffer

Each `ChildHandle` maintains two in-memory ring buffers, one per stream:

- Maximum capacity: `subprocess.aggregate_buffer_bytes` (default 65536 = 64 KiB
  per stream, 128 KiB total per job). This is configurable per job via
  the `subprocess.spawn` argument `aggregate_buffer_bytes`; the server cap is
  `subprocess.aggregate_buffer_bytes_max` (default 1 MiB).
- Write policy: newest-byte-wins ring buffer. When the ring buffer is full,
  the oldest bytes are overwritten. The `stdout_aggregate_truncated` and
  `stderr_aggregate_truncated` boolean fields in the result indicate truncation.
- The ring buffer receives bytes from the reader task directly, independent of
  the mpsc channel. Dropped chunks (due to full mpsc) still enter the ring buffer.

### Result Shape

`subprocess.result(job_id)` returns:

```
{
  "job_id": "<uuid7>",
  "job_state": "Succeeded" | "Failed" | "Cancelled" | "Killed",
  "exit_code": <i32 | null>,
  "stdout_aggregate_base64": "<base64>",
  "stderr_aggregate_base64": "<base64>",
  "stdout_aggregate_truncated": <bool>,
  "stderr_aggregate_truncated": <bool>,
  "stream_chunks_dropped": <u64>,
  "stdout_bytes_total": <u64>,
  "stderr_bytes_total": <u64>
}
```

`exit_code` is null when the process was killed via `SIGKILL` (exit code is
undefined in that case on POSIX) or when `job_state` is `Cancelled` before
the process exited.

### Terminal Notification

When the child exits or is killed, the dispatcher task emits a final
`notifications/progress` event with `job_state` set to the terminal state and
an additional `exit_code` field. This terminal event has `chunk_base64` set
to an empty string and `seq` incremented beyond all previous stream events for
that job. Clients should use this event to trigger `subprocess.result`.

### Cleanup on Cancellation

When the `ChildHandle` `CancellationToken` fires (per ADR-0053 cascade kill
chain):

1. The reader tasks observe the cancellation in their `select!` arm and close
   their send-side mpsc channel ends.
2. The dispatcher task detects the closed channels (receiver returns `None`)
   and drains any buffered chunks before emitting the terminal notification.
3. Ring buffers are not cleared on cancellation; the aggregate data up to the
   point of cancellation is available via `subprocess.result`.

### New Config Keys

- `subprocess.aggregate_buffer_bytes` — default per-job ring buffer size per
  stream in bytes (default 65536).
- `subprocess.aggregate_buffer_bytes_max` — server-enforced hard cap on
  `aggregate_buffer_bytes` in `subprocess.spawn` arguments (default 1048576).
- `subprocess.stream_flush_interval_ms` — time-based flush interval in
  milliseconds (default 100).

### New Audit Events

- `SUBSTRATE_STREAM_CHUNK_DROPPED` — emitted when backpressure causes a chunk
  to be discarded. Payload: `{job_id, stream, dropped_bytes, seq, timestamp}`.

## Consequences

### Positive

- Child process writes are never blocked by MCP client slowness; the pipe
  buffer between child and substrate remains unconsumed only when the reader
  task is behind, which is bounded by `subprocess.stream_flush_interval_ms`.
- The 4 KiB chunk size and 100 ms flush cadence balance throughput and latency
  for the common case of log-intensive CLI tools.
- The ring buffer ensures `subprocess.result` always returns the most recent
  bytes of output even when the full stream was truncated.
- `stream_chunks_dropped` in the result gives agents an explicit signal that
  the stream was lossy and the aggregate may be incomplete.

### Negative

- Two additional tokio tasks per subprocess (reader stdout, reader stderr) plus
  one dispatcher task bring the total overhead per subprocess to three tasks.
  With `subprocess.max_concurrent = 8` (from ADR-0052), this is at most 24
  additional tasks, well within tokio's default ceiling.
- base64 encoding inflates stream data by approximately 33%; a 4 KiB decoded
  chunk becomes approximately 5.5 KiB on the wire. This is acceptable for the
  intended use cases but should be noted by operators expecting large binary output.
- The ring buffer's newest-byte-wins eviction discards the earliest output when
  the buffer overflows. For long-running verbose processes, the beginning of
  stdout/stderr is lost. Operators can increase `aggregate_buffer_bytes` to
  retain more history.

### Risks

- If the dispatcher task panics (which causes process abort under `panic = "abort"`,
  per [ADR-0014](0014-build-system-and-toolchain.md)), all in-flight subprocess
  jobs lose their stream output. Mitigation: the dispatcher task MUST use `?`
  propagation and not `unwrap()` on channel operations; a closed channel is an
  expected condition (child exit), not a panic.

## Validation

- Unit test: reader task with a 4 KiB + 1 byte input; assert exactly two chunks
  are emitted (one full, one partial remainder).
- Unit test: fill the mpsc channel to capacity (64 chunks); assert the 65th
  chunk is dropped, `SUBSTRATE_STREAM_CHUNKS_DROPPED` increments by 1, and
  the ring buffer still receives the dropped bytes.
- Unit test: assert ring buffer truncation: write 70 KiB to a 64 KiB buffer;
  assert `stdout_aggregate_truncated = true` and the retained bytes are the
  last 64 KiB.
- Integration test: spawn a subprocess that writes 10 lines to stdout; assert
  all 10 lines appear in `stdout_aggregate_base64` after base64 decode.
- Integration test: cancel a subprocess mid-stream; assert the terminal
  notification carries `job_state=Cancelled` and `subprocess.result` returns
  partial aggregate with `stream_chunks_dropped >= 0`.

## Links

- [ADR-0040](0040-async-job-control-plane.md) — job control-plane; ProgressEvent shape
- [ADR-0038](0038-audit-event-semantics.md) — audit event semantics
- [ADR-0052](0052-subprocess-execution-architecture.md) — subprocess BC architecture
- [ADR-0053](0053-process-lifecycle-cascade-contract.md) — cascade kill + cleanup ordering

## Amendments

### 2026-05-24 — TmpFile capture branch (closes ADR-0033 cross-cutting hook)

The original decision outcome specified the live `notifications/progress` channel
with a bounded `mpsc(64)` per stream and a 64 KiB ring buffer aggregate. It
deferred the TmpFile persistence path — whose naming convention was formalised in
the [ADR-0033](0033-transactional-write-pattern.md) amendment of the same date —
to a follow-up implementation wave. This amendment closes that gap.

#### Activation condition

When `SubprocessRequest.capture_kind` equals `"tmp_file"`, the
substrate-subprocess adapter activates the TmpFile branch in addition to the
standard live notification path. The two paths are not mutually exclusive: stream
chunks continue to flow through the `mpsc(64)` dispatcher and are emitted as
`notifications/progress` events at the same 100 ms / 4 KiB flush cadence defined
in the original decision outcome. The tmp file provides the authoritative
persistence path; the live channel remains best-effort (backpressure may still
drop chunks from the mpsc channel, as documented in the Backpressure section).

#### Temporary file creation

Two `tokio::fs::File` writers are opened at spawn time — one for stdout, one for
stderr — at the transit paths defined by [ADR-0033](0033-transactional-write-pattern.md):

```
<tmp_root>/.substrate-subprocess-stream-<job_id>.stdout.tmp.<uuid7>
<tmp_root>/.substrate-subprocess-stream-<job_id>.stderr.tmp.<uuid7>
```

where `<tmp_root>` is resolved from the `subprocess.tmp_root` configuration key
(see [ADR-0017](0017-concurrency-limits.md) amendment). Files are created with
mode `0600` (owner read/write only) to prevent subprocess output from leaking to
other users on shared hosts.

Both files are created before the child process is spawned so that cleanup
registration covers both writers regardless of which stream produces output first.
The transit paths are added to `ChildHandle.tmp_files` immediately on creation.

#### Write ordering in the reader task

For each 4 KiB chunk read from the OS pipe, the reader task executes the
following sequence in order:

1. Append bytes to the in-memory ring buffer (aggregate retention, unchanged).
2. Await `writer.write_all(chunk).await` — file write is the authoritative
   persistence path and is not best-effort. A write error here transitions the job
   to `Failed` with `SUBSTRATE_IO_ERROR`.
3. Call `mpsc::Sender::try_send(chunk)` — best-effort live delivery. If the
   channel is full, the chunk is dropped from the live channel but has already
   been persisted to disk in step 2. The `SUBSTRATE_STREAM_CHUNKS_DROPPED`
   counter and audit event still fire on mpsc drop.

#### Finalisation on terminal Succeeded

When the child exits with a zero exit code and the job transitions to `Succeeded`:

1. `writer.flush().await` is called for both writers to drain OS buffers.
2. `tokio::fs::rename(transit_path, final_path).await` performs the atomic rename
   per [ADR-0033](0033-transactional-write-pattern.md):

```
<tmp_root>/.substrate-subprocess-stream-<job_id>.stdout
<tmp_root>/.substrate-subprocess-stream-<job_id>.stderr
```

The rename is atomic on POSIX because source and destination share the same
filesystem (both live under `tmp_root`). After a successful rename, the transit
path entry is removed from `ChildHandle.tmp_files` so the cascade cleanup chain
does not eradicate the persisted final file.

3. The final paths are recorded in `SubprocessResult.stdout_tmp_path` and
   `SubprocessResult.stderr_tmp_path` (both `Option<PathBuf>`).

#### Cleanup on non-Succeeded terminal states

On `Cancelled`, `Failed`, `Killed`, or `TimedOut`, the transit `.tmp.<uuid7>`
paths remain in `ChildHandle.tmp_files` and the standard
`cleanup_tmp_files` cascade removes them. `SubprocessResult.stdout_tmp_path` and
`stderr_tmp_path` are `None`. Cleanup MUST handle `ENOENT` gracefully (the orphan
reaper from [ADR-0055](0055-orphan-reaper-on-startup.md) may have already removed
the file between the terminal state write and the cleanup call).

#### Dual-channel flow

The following diagram shows the parallel persistence and live notification paths
when `capture_kind = "tmp_file"`.

```mermaid
sequenceDiagram
    participant Pipe as Kernel Pipe
    participant Reader as Reader Task
    participant Ring as Ring Buffer (64 KiB)
    participant TmpFile as Tmp File (disk)
    participant Chan as mpsc:64
    participant Dispatcher as Dispatcher Task
    participant Client as MCP Client

    Pipe->>Reader: chunk (up to 4 KiB)
    Reader->>Ring: append bytes (in-memory aggregate)
    Reader->>TmpFile: write_all(chunk) [authoritative, awaited]
    Reader->>Chan: try_send(chunk) [best-effort, may drop]
    Chan->>Dispatcher: recv chunk
    Dispatcher->>Client: notifications/progress stream=stdout seq=N

    Note over Reader,TmpFile: On terminal Succeeded
    Reader->>TmpFile: flush()
    Reader->>TmpFile: rename transit→final (atomic POSIX)
    Note over TmpFile: final path reported in SubprocessResult

    Note over Reader,Chan: On Cancelled/Failed/Killed/TimedOut
    Reader->>TmpFile: cleanup_tmp_files removes transit path (ENOENT-safe)
```

#### New SubprocessResult fields

`SubprocessResult` gains two optional fields:

- `stdout_tmp_path: Option<PathBuf>` — absolute path to the final stdout capture
  file; set only when `capture_kind == "tmp_file"` and `state == Succeeded`.
- `stderr_tmp_path: Option<PathBuf>` — absolute path to the final stderr capture
  file; set only when `capture_kind == "tmp_file"` and `state == Succeeded`.

These fields are surfaced in the `subprocess.result` tool response and in the
hints map extension defined by [ADR-0040](0040-async-job-control-plane.md).

#### SubprocessRegistry::new signature change

`SubprocessRegistry::new` gains a `tmp_root: Option<PathBuf>` parameter (last
positional). When `None`, any `subprocess.spawn` call with `capture_kind ==
"tmp_file"` returns `SUBSTRATE_INVALID_INPUT` synchronously without creating a
job entry. This makes TmpFile mode operationally opt-in: operators that have not
configured `subprocess.tmp_root` cannot accidentally trigger disk persistence.

#### Cross-references

- [ADR-0033](0033-transactional-write-pattern.md) — transit file naming convention and
  atomic rename invariants
- [ADR-0014](0014-build-system-and-toolchain.md) — `panic = "abort"` cleanup contract
  (no unwind-based RAII; cleanup driven from `tokio::select!` cancel arm)
- [ADR-0017](0017-concurrency-limits.md) — `subprocess.tmp_root` configuration key
- [ADR-0040](0040-async-job-control-plane.md) — hints map extension for
  `subprocess_stdout_tmp_path` and `subprocess_stderr_tmp_path`

### 2026-05-24 — Ring buffer is source of truth for pagination and search (ADR-0057)

[ADR-0057](0057-subprocess-output-pagination-and-search.md) introduces line-based
pagination on `subprocess.result` and a new `subprocess.search` tool. Both
features read exclusively from the in-memory ring buffers defined in this ADR
(the "Aggregate Retention Ring Buffer" section). No additional persistence layer
is introduced.

Key invariants that remain unchanged:

- The ring buffer capacity bounds are unchanged: default 64 KiB per stream,
  server cap 1 MiB per stream (`subprocess.aggregate_buffer_bytes_max`).
- Newest-byte-wins eviction still applies; ADR-0057 pagination and search operate
  on whatever bytes are currently retained in the buffer.
- `stdout_aggregate_truncated` and `stderr_aggregate_truncated` flags remain
  authoritative signals that the ring buffer overflowed during the job lifetime.
  Agents SHOULD surface these flags when presenting paginated or search results to
  indicate that earlier output was discarded.
- The ring buffer is populated from the reader task directly and independently
  of the mpsc channel; dropped mpsc chunks (backpressure) do NOT affect ring
  buffer contents.

Cross-reference: [ADR-0057](0057-subprocess-output-pagination-and-search.md).

### 2026-06-10 — Canonical chunk payload aligned to the shipped wire format

The "Stream Chunk Payload", "Result Shape", and "Terminal Notification"
sections above were authored ahead of the implementation. The shipped
`rmcp` stream notifier (`crates/substrate-mcp-server/src/handlers/rmcp_stream_notifier.rs`)
and the domain `StreamChunk` value object
(`crates/substrate-domain/src/subprocess/stream.rs`) diverge from that draft
on three points. The code is ground truth; this amendment corrects the ADR to
match it.

**1. Counter field name is `seq` (not `chunk_seq`).**
The per-job, per-stream monotonic counter is serialized on the wire under the
key `seq`, and the domain field is `StreamChunk.seq: u64`. An earlier
implementation emitted it under the non-spec name `chunk_seq`; that has been
renamed to the ADR name `seq`. The "Stream Chunk Payload" block above (which
already uses `seq`) is therefore authoritative; any reference to `chunk_seq`
elsewhere is stale and refers to this same field.

**2. `byte_offset` is part of the per-chunk frame.**
Each per-chunk notification carries an additional `byte_offset` field — the
cumulative byte offset of the FIRST byte of the chunk relative to the start of
the stream (domain field `StreamChunk.byte_offset: u64`). It is packed inside
the notification `message` JSON object alongside the other chunk fields. The
terminal frame does NOT carry `byte_offset`. The corrected per-chunk payload
is:

```
{
  "progressToken": "<job_id>",
  "progress": 0.0,
  "total": null,
  "job_id": "<job_id>",
  "job_state": "running",
  "stream": "stdout" | "stderr",
  "chunk_base64": "<base64-encoded bytes, max 4 KiB decoded>",
  "chunk_bytes": <u32 decoded byte count>,
  "seq": <u64 monotonic, per-job, per-stream>,
  "byte_offset": <u64 cumulative offset of the first byte in this chunk>
}
```

**3. `progress` is a fixed sentinel, not `bytes_emitted % 100`.**
The "Stream Chunk Payload" section describes `progress` as `bytes_emitted % 100`.
The shipped notifier does NOT compute a modulo wrap and does NOT emit a
cumulative byte count in `progress`. Instead it emits a fixed sentinel:
per-chunk frames carry `progress: 0.0` with `total: null`; the terminal frame
carries `progress: 100.0` with `total: 100.0`. Byte tracking is conveyed
exclusively through the additive `byte_offset` field, never through `progress`.
(The separate non-stream job `ProgressEvent` from
[ADR-0040](0040-async-job-control-plane.md) is unrelated: it uses a true
`progress: u8` in `0..=100` with `total: u32` default 100, no wrap.)

**4. `exit_code` rides inside the terminal `message` payload and is always
`null`.**
The terminal notification carries `exit_code` packed inside the `message` JSON
object (alongside `job_id`, `job_state`, `chunk_base64`, `chunk_bytes`, and
`seq`) — NOT as a top-level rmcp field and NOT in a separate hints map. Its
value is ALWAYS JSON `null`, because the `StreamChunkObserver::on_terminal`
trait surface carries neither the child exit code nor the terminal `seq`; both
are emitted as `null`. The corrected terminal payload is:

```
{
  "progressToken": "<job_id>",
  "progress": 100.0,
  "total": 100.0,
  "job_id": "<job_id>",
  "job_state": "succeeded" | "failed" | "cancelled" | "timed_out",
  "exit_code": null,
  "chunk_base64": "",
  "chunk_bytes": 0,
  "seq": null
}
```

The authoritative exit code is always retrieved via `subprocess.result`
(the "Result Shape" section), where `exit_code` is `<i32 | null>`. `exit_code:
null` is permitted for SIGKILL-killed or pre-exit-cancelled processes, as the
"Result Shape" section already states. Note also that the `job_state` values on
the wire are snake_case (`running`, `succeeded`, `failed`, `cancelled`,
`timed_out`), matching the `#JobState` serde `rename_all = "snake_case"`
contract; the PascalCase forms shown in the original draft above are
illustrative only.

Cross-references: [ADR-0040](0040-async-job-control-plane.md) — `ProgressEvent`
percentage semantics; [ADR-0057](0057-subprocess-output-pagination-and-search.md)
— ring buffer as source of truth.
