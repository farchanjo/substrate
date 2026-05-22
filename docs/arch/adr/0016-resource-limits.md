---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0016 — Resource Limits

## Context and Problem Statement

substrate runs as an unprivileged child process of an LLM agent. Because the agent controls tool arguments, adversarial or misconfigured inputs could cause the server to exhaust system memory, file descriptors, or disk I/O throughput. Resource limits must be enforced at the tool layer to protect the host system and sibling processes, while remaining tunable for operators with legitimate high-volume workloads.

## Decision Drivers

- A single unbounded tool call (e.g., reading a multi-gigabyte file into memory) must not OOM the host.
- File descriptor exhaustion would break all concurrent tool calls silently; a minimum FD floor must be asserted at startup.
- Streaming is preferable to buffering; tools that can emit results incrementally must do so to keep memory usage flat.
- Archive operations are a known amplification vector (zip bomb, deeply nested archives); input size must be bounded independently of output size.
- `find`-type operations over large directory trees can generate unbounded result sets; pagination must be enforced.
- All limits must be overridable by operators via TOML configuration (ADR-0011) to avoid blocking legitimate workloads.

## Considered Options

1. Per-tool memory cap (8 MiB default, 64 MiB ceiling) + FD ulimit assertion + streaming-by-default + archive input cap (1 GiB) + find pagination (5000 results).
2. Process-level `RLIMIT_AS` only, no per-tool enforcement.
3. No limits in MVP; rely on OS OOM killer.
4. Container-level cgroup limits with no in-process enforcement.

## Decision Outcome

Chosen option: "layered in-process resource limits with TOML overrides", because they provide deterministic, observable behaviour at the tool boundary without requiring container orchestration, and the limit values are conservative enough to protect typical developer workstations while being tunable for server deployments.

### Consequences

#### Positive

- **Per-tool memory cap**: each tool call that buffers output allocates into a bounded byte buffer. Default cap: 8 MiB. Maximum configurable ceiling: 64 MiB. If a tool output would exceed the cap, it returns `ERR_OUTPUT_TOO_LARGE` with a structured error indicating the cap in effect. Operators configure per-tool overrides under `[runtime.tool_memory_cap_bytes]` in TOML.
- **File descriptor floor**: at startup, substrate reads the current soft `RLIMIT_NOFILE`. If it is below 1024, the process attempts `setrlimit` to raise it. If the raise fails and the soft limit is below 256, startup aborts with exit code 78 and a diagnostic message on stderr.
- **Streaming by default**: tools that read files, list directories, or stream archive entries emit results as MCP progress notifications rather than accumulating a full response. Buffering is opt-in (e.g., for tools where the agent needs the complete result atomically).
- **Archive input cap**: archive extraction and inspection tools reject input files larger than 1 GiB by default. The limit is configurable via `[runtime.archive_max_input_bytes]`. The check is performed on the declared file size before reading content, guarding against zip-bomb decompression amplification.
- **find result pagination**: directory enumeration tools cap results at 5000 entries per call. When the cap is reached, the tool returns the partial result with a `next_cursor` token for pagination. The default is configurable via `[runtime.find_max_results]`. This prevents accidental full-filesystem traversal from returning millions of entries.

#### Negative

- Per-tool memory caps require each tool implementation to thread a `BoundedWriter` through its output path, adding boilerplate that must be enforced by code review and lint.
- The 8 MiB default cap will cause legitimate failures for tools reading large but not adversarial files (e.g., a 20 MiB log file). Operators must consciously raise the cap; the error message must clearly explain how to do so.
- FD floor assertions at startup add latency (~1 ms) and a syscall that may be disallowed in some sandbox environments (e.g., macOS App Sandbox). In those environments, the assertion is skipped with a WARN log rather than aborting.
- Archive input cap based on declared file size can be bypassed by a file with a falsified size header; content-length verification during streaming is the correct mitigation but is deferred to a future ADR.

## Validation

- Unit tests assert that `BoundedWriter` returns `ERR_OUTPUT_TOO_LARGE` at exactly the configured byte boundary.
- Integration test: a tool call reading a synthetic 9 MiB file with default cap returns `ERR_OUTPUT_TOO_LARGE`; the same call with cap raised to 16 MiB succeeds.
- Integration test: archive extraction of a 1.1 GiB synthetic archive returns `ERR_INPUT_TOO_LARGE` before any decompression occurs.
- Integration test: directory enumeration over a tree with 6000 entries returns exactly 5000 results plus a valid `next_cursor`.
- CI startup test asserts that substrate logs the FD soft limit on startup and does not abort on a standard macOS developer workstation (default `RLIMIT_NOFILE` = 256 on some versions; the raise to 1024 must succeed or log a warning without aborting).

## Cumulative Memory Guard

The per-tool memory cap stated in the Decision Outcome (8 MiB default, 64 MiB ceiling) is reconciled with the 10-concurrent 200 MiB RSS SLO from ADR-0039 as follows: the configurable ceiling for per-tool memory is reduced to **32 MiB** (not 64 MiB as originally stated). This ensures that 10 concurrent tools each hitting their ceiling consume at most 320 MiB of heap before accounting for substrate overhead, which is bounded under the 200 MiB idle RSS baseline plus operational headroom.

A process-level RSS guard runs at 5-second intervals on a dedicated `tokio::task` using `/proc/self/status` (Linux) or `task_info` (macOS) to sample the current resident set size. If the sampled RSS exceeds `max_process_rss_bytes` (default `268435456` = 256 MiB, configurable under `[runtime.max_process_rss_bytes]`), substrate enters a **30-second cooldown** during which new incoming tool calls are immediately rejected with `SUBSTRATE_RESOURCE_LIMIT` before entering the tool handler. In-flight tool calls are not interrupted. The cooldown timer resets on each interval where RSS remains above the threshold; it expires only after a full 30-second interval with RSS below the threshold.

This guard is in addition to, not a replacement for, the per-tool `BoundedWriter` cap.

## Abuse Detection

If more than 20 `SUBSTRATE_RESOURCE_LIMIT` errors are emitted in a rolling 60-second window (counting both per-tool cap rejections and process-level RSS cooldown rejections), substrate emits a WARN audit event with `event = "memory_cap_abuse"` and increments a process-lifetime counter exposed in the `active_requests_at_start` audit field context. Operators investigating memory pressure incidents can cross-reference this event with the `correlation_id` values of the rejected calls to identify the responsible workflow or agent session.

## Disk-Space Preflight

Before any mutating write operation (`fs.write`, `archive.create`, `archive.extract`), substrate performs a disk-space preflight check by calling `statvfs` (via the `nix` crate on Unix) on the target path's filesystem. If the available free space is less than `declared_input_size + 10%` of that size as a safety margin, the tool returns immediately with `SUBSTRATE_STORAGE_FULL` (see [ADR-0034](0034-kernel-induced-error-codes.md)) without performing any write. This prevents partial writes that corrupt output files and avoids filling the filesystem entirely.

The declared input size is the byte length of the content to be written as provided in the tool arguments, not a pre-read of the content.

## Error Payload Shape

`SUBSTRATE_RESOURCE_LIMIT` and `SUBSTRATE_STORAGE_FULL` responses MUST include a structured `data` field in the MCP error payload:

```json
{
  "code": "SUBSTRATE_RESOURCE_LIMIT",
  "data": {
    "observed_bytes": 9437184,
    "limit_bytes": 8388608
  }
}
```

This allows the calling agent to determine whether the limit is a per-tool cap (in which case `limit_bytes` equals the per-tool ceiling) or a process-level RSS limit (in which case `observed_bytes` is the current process RSS and `limit_bytes` is `max_process_rss_bytes`). The agent can use this information to retry with smaller inputs, split operations, or escalate to the operator.

## Log Rotation

When `[logging].target = "file"`, `tracing_appender::rolling` is used with the following configurable parameters:

- `max_log_file_bytes` (default `104857600` = 100 MiB): the maximum size of a single log file before rotation.
- `log_rotate_count` (default `7`): the number of rotated log files retained before the oldest is deleted.

Log rotation is performed by `tracing_appender::rolling::RollingFileAppender` with a `daily` or `size` trigger as configured. Rotation events are logged at INFO level to stderr (even when the file target is active) so that log management systems can detect rotation without parsing the rotated files.

## Amendment — proc.tree node cap (2026-05-22)

`proc.tree` is a Bucket A snapshot tool (ADR-0040) that returns a nested process hierarchy. Its only documented bound was the depth cap (`max_depth`, default and ceiling 32). Depth alone does not bound payload size: rooted at a wide node such as PID 1, even a shallow tree serializes the entire process forest (hundreds of KiB), overrunning model context windows and tripping client-side response-size limits.

This amendment adds a **total-node cap** to `proc.tree`, complementing the depth cap and aligning with the `find` pagination limit already defined above:

- `proc.tree` serializes at most `node_cap` nodes per response. Default and hard ceiling: **500** (the same order as `protocol.max_page_size`). A request `max_nodes` is clamped into `1..=500`.
- The tree is built breadth-respecting depth-first with a shared node budget; once the budget is exhausted, remaining subtrees are dropped rather than partially serialized.
- When truncation occurs the response sets `truncated: true` and echoes `node_cap`, alongside `node_count`. The tool's `content` instructs the agent to narrow `root_pid` or lower `max_depth`.

Unlike the cursor-paginated `find` limit, `proc.tree` does not paginate: a nested tree has no stable linear cursor. Truncation with an explicit indicator is the bound; agents refine the query instead of paging.

## Cross-References

- ADR-0006 — Streaming protocol (defines how per-tool progress notifications are structured)
- [ADR-0040](0040-async-job-control-plane.md) — Bucket classification (`proc.tree` is Bucket A, depth- and node-capped)
- ADR-0017 — Timeout and cancellation model (timeouts interact with streaming loops)
- ADR-0029 — Tool error codes (`ERR_OUTPUT_TOO_LARGE`, `ERR_INPUT_TOO_LARGE` are defined here)
- [ADR-0033](0030-performance-budgets.md) — Error budget policy (resource limit breaches consume error budget)
- [ADR-0034](0034-kernel-induced-error-codes.md) — Kernel-induced error codes (`SUBSTRATE_STORAGE_FULL` defined here)
- [ADR-0038](0038-audit-event-semantics.md) — Audit event semantics (`active_requests_at_start` counter sourced from the same atomic used by RSS guard)
