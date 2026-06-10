---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0017 — Concurrency Limits (Per-Tool Semaphores, Global Cap)

## Context and Problem Statement

`substrate` executes tool calls that span three async zones (A: async-native, B: sync I/O via spawn_blocking, C: CPU-bound via spawn_blocking + Semaphore). Without explicit concurrency limits, simultaneous tool calls from an LLM agent can exhaust file descriptors, saturate all CPU cores with hashing tasks, or cause the OS to thrash between blocking threads. The MCP protocol does not itself serialize tool calls; the server must impose its own limits.

## Decision Drivers

- Zone C (blake3, sha2, regex scanning) is CPU-saturating; unlimited concurrent calls degrade all other zones.
- Zone B spawn_blocking threads are limited by tokio's blocking thread pool (default 512); we need a tighter application-level limit.
- rmcp's stdio transport serializes the framing layer but not the execution layer — tool calls may overlap if the caller pipelines requests.
- Limits must be configurable via TOML without recompilation.
- Back-pressure must be surfaced as a structured error (`ToolError::ResourceExhausted`) rather than a silent queue.

## Considered Options

1. Global tokio thread pool + per-zone `tokio::sync::Semaphore` — accepted.
2. `rayon::ThreadPoolBuilder` with a fixed thread count for Zone C — rejected; rayon's blocking model does not integrate with tokio's cancellation primitives (CancellationToken); mixing executors complicates cancellation (see ADR-0006).
3. Tower middleware `tower::limit::ConcurrencyLimit` — rejected; rmcp does not expose a Tower service interface in transport-io mode.
4. OS-level `ulimit` enforcement only — rejected; ulimit applies to the whole process, not per-zone or per-tool.
5. Unbounded concurrency with circuit-breaker — rejected; circuit-breaker adds complexity and still allows burst saturation before the breaker trips.

## Decision Outcome

Chosen option: "tokio multi-thread pool + per-zone Semaphores + configurable global cap", because it is composable with CancellationToken, configurable at runtime, and maps directly onto the three async zones defined in ADR-0003.

### Global Tokio Thread Pool

tokio's multi-thread runtime uses `num_cpus::get()` worker threads by default. This is not overridden; the OS scheduler distributes work across cores. The pool is shared by all three zones.

### Zone C CPU-Bound Semaphore

A single `Arc<Semaphore>` is initialized at startup with `num_cpus::get()` permits and stored in the application context:

```rust
let cpu_semaphore = Arc::new(Semaphore::new(num_cpus::get()));
```

Every Zone C tool call acquires one permit before entering `spawn_blocking`. The permit is released when the blocking closure returns or the `CancellationToken` fires. This ensures at most N simultaneous CPU-saturating operations, where N equals the number of logical cores.

The permit count is configurable via `[concurrency] cpu_permits` in the TOML config; the default is `num_cpus::get()`.

### Zone B Semaphores (Per-Tool)

Zone B tools (sysinfo snapshots, procfs reads, directory walks) do not require a global cap because they are I/O-latency-bound rather than CPU-saturating. However, per-tool semaphores can be configured for tools where simultaneous calls would cause OS resource pressure (e.g., recursive directory listing):

```toml
[tools.fs_find.concurrency]
max_concurrent = 4
```

If no per-tool limit is configured, Zone B tools run without a Semaphore (bounded only by tokio's blocking thread pool limit of 512).

### Global Request Cap

rmcp's transport-io layer processes one frame at a time from the stdio stream. Pipelined requests are queued in the framing buffer. The application does not impose an additional global in-flight cap beyond the per-zone Semaphores, because:

1. The stdio framing serializes request arrival.
2. Per-zone Semaphores already bound the total threads in use.
3. Adding a global cap would require a coordination layer that rmcp does not expose.

If future profiling shows the framing queue causes memory pressure, a bounded channel cap (256 messages, matching ADR-0031's channel cap) will be introduced.

### Semaphore Permit API

All Semaphore permit acquisitions MUST use `Arc<Semaphore>::acquire_owned()` to obtain an `OwnedSemaphorePermit`:

```rust
let permit: OwnedSemaphorePermit = Arc::clone(&ctx.cpu_semaphore)
    .acquire_owned()
    .await
    .map_err(|_| ToolError::semaphore_closed())?;
// spawn_blocking proceeds; permit drops when async fn returns.
```

`OwnedSemaphorePermit` is `'static` and survives across `.await` points. `SemaphorePermit<'_>` borrows the `Semaphore` and cannot be held across awaits; it is forbidden in async code. See [ADR-0037](0037-async-cancellation-patterns.md) for the full permit lifetime rule.

### Semaphore Construction Guard

`cpu_bound_max` is validated as a `NonZeroUsize` at config-load time. `Semaphore::new(0)` creates a semaphore that can never be acquired; it is rejected at startup with error code `SUBSTRATE_CONFIG_INVALID`. The validation runs before the tokio runtime is initialized so that the error is reported synchronously to stderr. See [ADR-0036](0036-startup-error-contract.md) for the startup error contract.

```rust
let cpu_permits = NonZeroUsize::new(config.concurrency.cpu_permits.unwrap_or(num_cpus::get()))
    .ok_or_else(|| StartupError::config_invalid("concurrency.cpu_permits must be non-zero"))?;
let cpu_semaphore = Arc::new(Semaphore::new(cpu_permits.get()));
```

### Maximum Waiters Cap

The Semaphore queue depth is bounded to prevent Denial-of-Service accumulation from slow clients. When the number of tasks waiting to acquire a permit exceeds `max_waiters` (default: 256 per pool), new acquire attempts are rejected immediately with `ToolError::ResourceExhausted` carrying error code `SUBSTRATE_RESOURCE_LIMIT`.

The cap is implemented by checking the Semaphore's available permits against a counter of current waiters before calling `acquire_owned()`. If `waiters >= max_waiters`, the tool returns the error without entering the Semaphore queue.

```toml
[concurrency]
max_waiters = 256
```

This prevents a scenario where thousands of slow-client tool calls all queue on the Semaphore, consuming memory and exhausting tokio task slots before any work is completed.

### Global Zone B Semaphore

A mandatory global Semaphore is maintained for all `spawn_blocking` calls across all tools, sized to `num_cpus * 4` permits:

```rust
let zone_b_semaphore = Arc::new(Semaphore::new(num_cpus::get() * 4));
```

This bounds the total number of concurrent `spawn_blocking` calls process-wide. Without this cap, tokio's default blocking thread pool ceiling of 512 threads can be reached silently, causing new `spawn_blocking` calls to queue inside tokio with no back-pressure visible to the application layer.

Zone C tools (CPU-bound) still use the separate `cpu_semaphore` (sized to `num_cpus`). Zone B tools (I/O-bound blocking) use `zone_b_semaphore`. A Zone C call acquires both permits: the `cpu_semaphore` permit first, then the `zone_b_semaphore` permit, in that order (consistent ordering prevents deadlock).

### Shutdown Drain

On `SIGTERM` or `SIGINT` (see [ADR-0032](0032-signal-safety.md)), the root `CancellationToken` is cancelled. The dispatch layer then waits up to `shutdown_drain_secs` (default: 5 seconds) for all in-flight tool `JoinSet` entries to resolve. Tools that are blocked on Semaphore permit acquisition during drain are aborted via `JoinSet::abort_all()` after the drain window expires. Processes spawned by tools are sent `SIGKILL` via `tokio::process::Command::kill()` after the drain window.

### Cancellation Latency SLO

The target latency from `CancellationToken::cancel()` signal to tool abort completion is 1 second. To meet this SLO:

- `spawn_blocking` closures MUST call `CancellationToken::is_cancelled()` between processing chunks (e.g., between files in a directory walk, between line batches in a grep scan). Chunk sizes are tuned so that each chunk completes within 200 ms, leaving headroom for the 1-second SLO.
- Semaphore permit acquisitions are always raced against the `CancellationToken` using `tokio::select! { biased; ... }` so that a cancelled tool does not wait the full `semaphore_wait_secs` for a permit.

### Back-Pressure Error

When a Semaphore permit cannot be acquired within a configurable wait (default: equal to the tool's timeout), the tool returns `ToolError::ResourceExhausted` with the semaphore name and current permit count. This is a structured error visible to the LLM agent.

### Configuration Summary

```toml
[concurrency]
cpu_permits = 0          # 0 = num_cpus::get() at runtime
semaphore_wait_secs = 30 # how long to wait for a permit before ResourceExhausted

[tools.fs_find.concurrency]
max_concurrent = 4

[tools.hash_file.concurrency]
max_concurrent = 0       # 0 = inherits cpu_permits (Zone C default)
```

### Consequences

#### Positive

- CPU cores are never fully starved by hashing; async I/O and MCP dispatch remain responsive.
- Per-tool limits give operators fine-grained control without code changes.
- Semaphore permits integrate cleanly with CancellationToken: permit acquisition is an `await` point and can be raced with token cancellation.

#### Negative

- `num_cpus::get()` counts hyper-threaded logical cores, not physical cores. On hyper-threaded systems, N CPU-bound tasks may still cause visible latency spikes. Operators can reduce `cpu_permits` in config.
- blake3 with `rayon` internally spawns rayon threads that are not subject to our Semaphore. The Semaphore limits the number of simultaneous blake3 *calls*, but each call may still spin up rayon's thread pool. This interaction must be documented in the [operator guide](../operations/operator-guide.md).
- Per-tool Semaphores add startup allocation proportional to the number of configured tools.

## Validation

- Benchmark: fire 2×N CPU-bound hash calls concurrently; measure that peak CPU usage stays below 100% × N cores and that async I/O latency remains under 50 ms during the burst.
- Unit test: acquire all permits; assert that the (N+1)th call returns `ToolError::ResourceExhausted` within `semaphore_wait_secs + 1s`.
- Integration test: configure `max_concurrent = 1` for `fs_find`; assert that two simultaneous `fs_find` calls are serialized, not run in parallel.

## Cross-References

- ADR-0006: CancellationToken and timeout; Semaphore permit acquisition is an `.await` point subject to timeout.
- ADR-0016: tool error taxonomy; `ToolError::ResourceExhausted` definition.
- [ADR-0032](0032-signal-safety.md): signal safety; SIGTERM triggers drain; `JoinSet::abort_all()` on shutdown.
- [ADR-0036](0036-startup-error-contract.md): startup error contract; `SUBSTRATE_CONFIG_INVALID` for zero-permit Semaphore.
- [ADR-0037](0037-async-cancellation-patterns.md): `OwnedSemaphorePermit` lifetime rule; `JoinSet` rule; biased-select for permit acquisition vs cancellation.

## Amendments

### 2026-05-24 — Subprocess concurrency limits (ADR-0052)

[ADR-0052](0052-subprocess-execution-architecture.md) introduces Bucket E (always-async subprocess dispatch) and requires four new concurrency configuration keys under the `[subprocess]` TOML section. These keys extend the concurrency model of this ADR without modifying the existing Zone A/B/C Semaphore machinery.

New configuration keys:

- `subprocess.max_per_client` (u32, default 4) — maximum number of simultaneously active subprocess jobs per client_id. Enforcement uses the same per-client job counter already maintained by the JobRegistry (ADR-0040). Exceeding this limit returns `SUBSTRATE_QUOTA_EXCEEDED` synchronously from `subprocess.spawn` without creating a job entry.
- `subprocess.max_concurrent` (u32, default 8) — global maximum number of simultaneously active subprocess jobs across all clients. Enforced by a dedicated `Arc<Semaphore>` with `max_concurrent` permits, acquired before the Bucket E job worker is spawned.
- `subprocess.spawn_rate_per_sec` (f64, default 1.0) — token-bucket rate limit on `subprocess.spawn` invocations per client_id. Clients that exceed this rate receive `SUBSTRATE_RESOURCE_LIMIT` without consuming a concurrent-subprocess permit. The token bucket is replenished continuously at the configured rate; burst capacity equals `spawn_rate_per_sec * 2.0` tokens (two-second burst).
- `subprocess.aggregate_buffer_bytes` (usize, default 65536) — per-stream ring buffer capacity for the aggregated stdout and stderr content stored by `subprocess.result`. When the ring buffer is full and the child is still writing, new chunks are dropped and `subprocess_stream_chunks_dropped` in the hints map is incremented.

Enforcement rules consistent with this ADR:

- The subprocess `Arc<Semaphore>` permit MUST be acquired using `Arc::clone(&ctx.subprocess_semaphore).acquire_owned().await` in async scope. The `OwnedSemaphorePermit` is passed into the Bucket E job closure and held until the JobEntry reaches a terminal state. The permit MUST NOT be moved into a `spawn_blocking` closure (per ADR-0037 and the `panic = "abort"` constraint of ADR-0014).
- Permit release on terminal state transition is explicit: the job worker drops the `OwnedSemaphorePermit` as part of the terminal state write inside the `parking_lot::Mutex` lock sequence, before the result watch channel is set.

Cross-references: [ADR-0040](0040-async-job-control-plane.md) — JobRegistry quota enforcement; [ADR-0052](0052-subprocess-execution-architecture.md) — subprocess execution architecture.

### 2026-05-24 (revision 2) — subprocess.tmp_root configuration key

The TmpFile capture branch formalised in the
[ADR-0054](0054-subprocess-stream-multiplex.md) amendment introduces a new
configuration key that determines where subprocess stream transit files are
written. This key extends the `[subprocess]` TOML section defined above.

New configuration key:

- `subprocess.tmp_root` (`Option<PathBuf>`, default: first entry in
  `policy.roots`) — the directory under which TmpFile capture-mode stream
  transit files and their final renamed counterparts are stored. When absent
  from the TOML config, substrate resolves the default at startup by taking
  the first entry from `policy.roots`; if `policy.roots` is empty, startup
  fails with `SUBSTRATE_CONFIG_INVALID`.

Validation rules applied at startup, before the tokio runtime is initialised:

1. The configured path is canonicalised. If canonicalisation fails (path does
   not exist or is not a directory), startup fails with `SUBSTRATE_CONFIG_INVALID`.
2. The canonicalised path MUST be a prefix of at least one entry in
   `policy.roots`, or MUST equal a `policy.roots` entry. A `tmp_root` that
   falls outside every `policy.roots` entry is rejected with
   `SUBSTRATE_CONFIG_INVALID`, because the path jail enforced by
   [ADR-0004](0004-security-model.md) would prevent substrate from creating or
   reading files there.
3. Both validations use the same `SUBSTRATE_CONFIG_INVALID` error code and
   startup error contract as defined in [ADR-0036](0036-startup-error-contract.md).

When `subprocess.tmp_root` is not present in the TOML config and no default
can be derived (empty `policy.roots`), `SubprocessRegistry::new` receives
`tmp_root = None`. Any subsequent `subprocess.spawn` call with
`capture_kind = "tmp_file"` returns `SUBSTRATE_INVALID_INPUT` synchronously.

Cross-references: [ADR-0054](0054-subprocess-stream-multiplex.md) — TmpFile capture
branch; [ADR-0033](0033-transactional-write-pattern.md) — transit file naming and
atomic rename invariants; [ADR-0036](0036-startup-error-contract.md) — startup
error contract; [ADR-0004](0004-security-model.md) — path jail.
