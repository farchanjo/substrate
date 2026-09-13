# Subprocess BC Tool Cards (ADR-0007 narrative-arc, ADR-0052 BC)

This file documents the five `subprocess.*` tool cards following the USE/DOES/ARGS/RETURNS/NEXT/AVOID
narrative-arc template prescribed by [ADR-0007](../../adr/0007-tool-card-narrative-arc.md). The subprocess
BC itself is defined in [ADR-0052](../../adr/0052-subprocess-execution-architecture.md). Each card body
is bounded to the 180-token cap per ADR-0007; the inline `description` string in Rust source follows
the thin format from the 2026-05-22 amendment (one-liner, ≤100 chars, `See substrate skill.` closing).

## subprocess.spawn

**USE**: when an agent needs to execute an external binary and capture its stdout/stderr as a job.

**DOES**: validates the binary against `security.subprocess_binary_allowlist`, sanitises the environment,
resolves `cwd` inside the path jail, then spawns the child via `tokio::process::Command` and registers
an async job in the job registry.

**ARGS**:

- `binary_path` (string, absolute) — path to the binary; must be in `security.subprocess_binary_allowlist`
- `args` ([]string) — argv passed verbatim; no shell expansion applied
- `env_allowlist` ([]string) — environment variable names inherited from the server environment
- `env_override` (map[string]string) — additional key=value pairs merged after allowlist; hard-banned vars rejected
- `cwd` (string, absolute) — working directory; must be under `security.allowed_paths`
- `stdin_kind` (enum: none | piped | file_path) — how stdin is supplied to the child
- `stdin_file_path` (string, optional) — path to a file used as stdin when `stdin_kind=file_path`
- `capture_kind` (enum: stream | in_memory | tmp_file) — output capture strategy
- `timeout_secs` (int, optional) — wall-clock deadline; child is SIGTERM-then-SIGKILLed on expiry
- `idempotency_key` (uuid7, optional) — deduplication key per ADR-0040; same key returns existing job
- `elicitation_confirmed` (bool) — must be `true`; subprocess.spawn always requires elicitation

**RETURNS**: `{job_id, job_state: Pending}` with hints `{subprocess_pid (set on Running), subprocess_pgid (set on Running), cascade_kill_pgid: true, confirm_destructive: true, polling_endpoint: "subprocess.result"}`.

**NEXT**: `subprocess.result`, `subprocess.cancel`

**AVOID**: elicitation required — never call with `elicitation_confirmed: false` or omitted; do not pass
shell metacharacters in `args` (argv is passed verbatim, no shell parsing occurs; use `subprocess.spawn`
with `binary_path=/bin/sh` and explicit `args` if shell evaluation is needed and allowed).

---

## subprocess.list

**USE**: when an agent needs to enumerate active or recent subprocess jobs for the current client session.

**DOES**: queries the job registry for subprocess jobs belonging to the calling client, applies optional
state and client-id filters, and returns a paginated list of `SubprocessHandle` records.

**ARGS**:

- `client_id_filter` (string, optional) — advisory filter; bounded to caller's own session; cross-client enumeration is forbidden
- `state_filter` ([]SubprocessState, optional) — restrict results to one or more job states
- `page_cursor` (string, optional) — opaque base64 cursor from a prior response
- `page_size` (int, default 50, max 500) — number of entries per page

**RETURNS**: `{items: [SubprocessHandle], next_cursor?}` with hints `{confirm_destructive: true, cascade_kill_pgid: true, next_action_suggested: "subprocess.cancel"}`.

**NEXT**: `subprocess.cancel`, `subprocess.result`

**AVOID**: cross-client enumeration is forbidden; `client_id_filter` is advisory and is always clamped to
the caller's own session regardless of the value supplied.

---

## subprocess.cancel

**USE**: when an agent needs to terminate a running subprocess job, either gracefully or immediately.

**DOES**: sends SIGTERM to the child process (and optionally the process group), waits up to the configured
drain window for a clean exit, then sends SIGKILL if the child has not exited; removes associated tmp files
and updates the job state to `Cancelled`.

**ARGS**:

- `job_id` (uuid7) — identifier of the subprocess job to cancel
- `force` (bool, default false) — when `true`, skips the drain window and sends SIGKILL immediately

**RETURNS**: `{job_state: Cancelled | already_done, cascade_summary: {pgid, children_signaled, tmp_files_removed}}` with hints `{confirm_destructive: true, cascade_kill_pgid: true, next_action_suggested: "subprocess.result"}`.

**NEXT**: `subprocess.result`

**AVOID**: elicitation required for `force=true` per ADR-0004 Layer 4; do not call repeatedly in a tight
loop — cancel is idempotent but each call emits an audit event and incurs a registry write.

---

## subprocess.result

**USE**: when an agent needs to retrieve the final output and exit code of a completed subprocess job.

**DOES**: long-polls the job registry up to `wait_ms`, then returns the terminal result including
`exit_code`, aggregated stdout/stderr (base64-encoded, bounded to `subprocess.aggregate_buffer_bytes`),
and stream-drop count.

**ARGS**:

- `job_id` (uuid7) — identifier of the subprocess job
- `wait_ms` (int, default 0, max bounded by `subprocess.aggregate_buffer_bytes` window) — long-poll ceiling before returning current state
- `include_aggregates` (bool, default true) — whether to include `stdout_aggregate_base64` and `stderr_aggregate_base64` in the response

**RETURNS**: `{exit_code, stdout_aggregate_base64, stderr_aggregate_base64, stream_chunks_dropped, duration_ms, terminal_state}` with hints `{confirm_destructive: true, cascade_kill_pgid: true, next_action_suggested: "subprocess.list"}`.

**NEXT**: `subprocess.list` (for batch follow-up)

**AVOID**: aggregates are bounded to `subprocess.aggregate_buffer_bytes` (default 64 KiB per stream);
for full streaming output, subscribe to the notifications/progress channel during execution rather than
relying on the aggregate after completion.

---

## subprocess.signal

**USE**: when an agent needs to deliver a specific POSIX signal to a running subprocess or its process group.

**DOES**: looks up the subprocess job in the registry, resolves the target PID or PGID, validates against
PID-0/1/2 protection rules (ADR-0035), and delivers the requested signal via `kill(2)`.

**ARGS**:

- `job_id` (uuid7) — identifier of the subprocess job
- `signal` (enum: SIGTERM | SIGINT | SIGKILL | SIGSTOP | SIGCONT | SIGUSR1 | SIGUSR2 | SIGHUP) — signal to deliver
- `target` (enum: process | process_group, default process) — whether to signal only the child PID or the entire process group

**RETURNS**: `{delivered: bool, target_pid_or_pgid, signal_name}` with hints `{confirm_destructive: true, cascade_kill_pgid: true, next_action_suggested: "subprocess.cancel"}`.

**NEXT**: `subprocess.cancel`, `subprocess.result`

**AVOID**: elicitation required for SIGKILL, SIGTERM, and SIGSTOP per ADR-0004 Layer 4; PID 0/1/2
protection per ADR-0035 still applies (substrate-mcp-server is excluded from self-signal).

---

## subprocess.search

**USE**: when an agent needs to locate specific log lines or output patterns in a subprocess job's stdout/stderr ring buffer.

**DOES**: decodes the ADR-0054 ring buffer for the specified job, splits on newline boundaries into a logical line array, compiles `pattern` as a Rust `regex::Regex` (10 MiB DFA cap, linear-time NFA/DFA engine), filters matching lines, and returns a paginated `SearchMatch` list with 1-based `line_number` values per stream.

**ARGS**:

- `job_id` (uuid7) — identifier of the subprocess job whose ring buffer is searched
- `pattern` (string, 1..=1024 bytes) — Rust regex pattern; compiled per-call; invalid patterns return `SUBSTRATE_INVALID_INPUT`
- `streams` ([]enum: stdout | stderr, default ["stdout","stderr"]) — which streams to search
- `case_insensitive` (bool, default false) — passed to `RegexBuilder::case_insensitive`
- `page_cursor` (string, optional) — opaque base64 cursor from a prior `subprocess.search` response
- `page_size` (int, default 50, max 10000) — number of matching lines per page per ADR-0057

**RETURNS**: `{matches: [SearchMatch], total_matches, next_cursor?}` where each `SearchMatch` carries `{stream, line_number, line_text}`. Hints: `{confirm_destructive: true, cascade_kill_pgid: true, next_action_suggested: "subprocess.result"}`.

**NEXT**: `subprocess.result` (for full aggregate), `subprocess.search` (next page via `next_cursor`)

**AVOID**: binary ring buffers with no `\n` bytes return `SUBSTRATE_INVALID_INPUT` ("aggregate is binary; use full base64 aggregate"); the ring buffer is bounded to `aggregate_buffer_bytes` (default 64 KiB per stream) — lines written before the buffer wrapped are not visible; do not use for forensic completeness, only for recent output inspection.
