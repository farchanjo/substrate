---
status: accepted
accepted_date: 2026-05-24
date: 2026-05-24
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0051 — Per-Process Resource Stats (proc.stats + proc.top)

## Context and Problem Statement

The `process` bounded context exposes `proc.list` and `proc.tree` for enumerating
running processes. These tools return identity and command-line metadata but do not
return resource consumption data. LLM agents performing workload profiling, memory
leak diagnosis, or CPU contention analysis need per-process resource snapshots:
resident set size, virtual memory, CPU utilization, open file descriptor count,
thread count, and process state.

Without `proc.stats` and `proc.top`, agents are forced to cross-reference
`proc.list` output with system-level `sys.load_average`, producing unreliable
estimates. The question is: which OS sources should back these tools, and how
do the PID safety constraints from [ADR-0035](0035-path-safety-hardening.md)
apply to process introspection?

## Decision Drivers

- Zero conflict with [ADR-0044](0044-no-subprocess-policy.md): both tools
  observe existing processes through kernel interfaces only; no process is
  spawned and no subprocess is invoked.
- PID-0/1/2 filter from [ADR-0035](0035-path-safety-hardening.md) must apply:
  these PIDs are kernel or init; their resource stats are always available but
  signaling them is forbidden. Reading stats is permitted; the filter applies
  only to `proc.signal`.
- Bucket A classification per [ADR-0040](0040-async-job-control-plane.md): both
  tools are snapshots capped at a page size; latency must be sub-millisecond
  for a single PID and under 50 ms for a full top-N list.
- Platform gates follow the `cfg(target_os)` convention from
  [ADR-0028](0028-platform-feature-gates.md): Linux uses `/proc/<pid>/`, macOS
  uses `sysctl KERN_PROC` and `task_info` via `libc`.
- Pagination follows the base64-opaque cursor pattern from ADR-0008.

## Considered Options

- Option A: Use the `sysinfo` crate exclusively for both tools.
- Option B: Use the `procfs` crate on Linux and `libc`/`nix` on macOS, with
  `sysinfo` as a fallback.
- Option C: Parse `/proc/<pid>/stat` directly on Linux, `sysctl KERN_PROC` on
  macOS, no crate fallback.

## Decision Outcome

Chosen option: "Option B — procfs crate on Linux, libc/nix on macOS, sysinfo
as fallback", because it provides the best data fidelity on each platform while
retaining cross-platform coverage for exotic kernels.

### Tool Definitions

**proc.stats(pid)**

Arguments:

- `pid` — process ID (u32, required)

Returns a `ProcessStats` aggregate containing:

- `pid` — u32
- `rss_bytes` — resident set size in bytes (u64)
- `virt_bytes` — virtual address space size in bytes (u64)
- `cpu_pct` — CPU utilization percentage 0.0–100.0 as a delta since last
  `proc.stats` call for this PID; 0.0 on cold start (f32)
- `threads` — number of threads in the process (u32)
- `fds` — number of open file descriptors (u32); null on macOS when the
  calling process lacks permission to read `proc_pidinfo`
- `uid` — real user ID of the process owner (u32)
- `start_time` — process start time as a Unix timestamp in seconds (u64)
- `state` — one of `Running`, `Sleeping`, `Stopped`, `Zombie`, `Idle`,
  `Unknown` (string enum)
- `command` — executable name without arguments, max 255 bytes (string)

Bucket: A (inline sync).

**proc.top(sort_by, limit, filter)**

Arguments:

- `sort_by` — one of `mem`, `cpu`, `pid`, `fds` (string, default `mem`)
- `limit` — maximum entries to return (u32, default 20, max 200)
- `filter` — optional substring match against `command` field (string, optional)
- `cursor` — base64-opaque pagination cursor (string, optional)

Returns a paginated list of `ProcessStats` entries sorted by the requested
field. Pagination uses the cursor pattern from ADR-0008. Page size is capped
at `limit` and must not exceed the configured `process.top_max_page_size`
(default 200).

Bucket: A (inline sync, capped page size).

### Per-Platform Data Source

The following diagram shows the per-platform data sources for `proc.stats`.

```mermaid
flowchart TD
    REQ[proc.stats or proc.top called] --> PLAT{target_os?}
    PLAT -- linux --> LX[Linux path]
    PLAT -- macos --> MX[macOS path]
    PLAT -- other --> FB[sysinfo crate fallback]

    LX --> L1[/proc/pid/stat\nrss virt state threads start_time]
    LX --> L2[/proc/pid/status\nuid threads VmRSS VmSize]
    LX --> L3[/proc/pid/fd count via nix opendir]
    L1 & L2 & L3 --> LDELTA[delta cpu_pct from Arc Mutex CpuSnapshot per pid]
    LDELTA --> RESP[ProcessStats]

    MX --> M1[sysctl KERN_PROC KERN_PROC_PID\nkinfo_proc struct via nix]
    MX --> M2[proc_pidinfo PROC_PIDTASKINFO\ntask_info struct via libc]
    MX --> M3[proc_pidinfo PROC_PIDLISTFDS\nfd count via libc]
    M1 & M2 & M3 --> MDELTA[delta cpu_pct from Arc Mutex CpuSnapshot per pid]
    MDELTA --> RESP

    FB --> SI[sysinfo Process snapshot]
    SI --> RESP
```

### Linux Implementation Detail

Data is sourced from `spawn_blocking` (Zone B) per the async zone policy in
[ADR-0003](0003-crate-stack-and-async-zones.md):

- `/proc/<pid>/stat` — fields 14 (`utime`) and 15 (`stime`) for CPU time, field
  24 (`rss` in pages, multiplied by `getconf PAGE_SIZE`), field 20 (`num_threads`),
  field 9 (`starttime` in clock ticks divided by `sysconf(_SC_CLK_TCK)`).
- `/proc/<pid>/status` — `Uid:` line for real UID, `Name:` for command.
- `/proc/<pid>/fd/` directory: entry count via `nix::fcntl::openat` +
  `nix::unistd::getdents64`; no `std::fs::read_dir` to avoid hidden heap
  allocation per entry.

CPU percent delta: a `Arc<Mutex<HashMap<u32, (u64, u64)>>>` caches
`(utime + stime, wall_clock_ticks)` per PID. On each call the delta is computed
and the entry updated. The cache is bounded to `process.stats_cache_max_pids`
(default 4096) via LRU eviction; old entries are evicted when the process no
longer appears in `/proc`.

### macOS Implementation Detail

Data is sourced from `spawn_blocking` (Zone B):

- `sysctl(CTL_KERN, KERN_PROC, KERN_PROC_PID, pid)` returns a `kinfo_proc`
  struct containing `p_stat`, `p_uid`, `p_comm`, and start time.
- `proc_pidinfo(pid, PROC_PIDTASKINFO)` returns a `proc_taskinfo` struct
  containing `pti_resident_size`, `pti_virtual_size`, `pti_total_user`,
  `pti_total_system`, `pti_threadnum`.
- `proc_pidinfo(pid, PROC_PIDLISTFDS)` returns the count of open FDs; the call
  may return `EPERM` for processes owned by other users, in which case `fds`
  is set to null in the response.

All three calls use `libc` FFI gated `#[cfg(target_os = "macos")]`.

### Process Safety Constraints

The process safety rules from [ADR-0035](0035-path-safety-hardening.md) define
that PID 1, 2, and kernel threads in that range are not signal targets. For
`proc.stats` and `proc.top`, reading stats for any PID is permitted (read-only
introspection). The restriction on PID 0/1/2 applies only to `proc.signal`
(mutation). This is consistent with ADR-0044's statement that "process
introspection observes existing processes only."

However, `proc.top` returns processes sorted by resource usage. A process list
that includes PID 1 is valid output; the caller is responsible for not
signaling such processes.

### New Config Keys

- `process.top_max_page_size` — maximum entries returned by `proc.top` per
  page (default: 200; hard cap enforced at startup).
- `process.stats_cache_max_pids` — maximum PID entries retained in the CPU
  delta cache (default: 4096; LRU eviction when full).

## Consequences

### Positive

- Agents can identify top memory-consuming and top CPU-consuming processes
  without shell access, supporting automated remediation workflows.
- Bucket A classification means `proc.stats` adds no latency to the MCP
  request pipeline.
- The LRU delta cache enables accurate CPU utilization tracking across
  repeated calls without retaining unbounded state.

### Negative

- The `fds` field returns null on macOS for cross-user processes; agents
  must handle null without treating it as an error.
- CPU percent on the first call for any PID is always 0.0 (cold start). The
  `structuredContent.hints.cold_start` flag informs the agent.
- On Linux containers with restrictive seccomp profiles, `/proc/<pid>/fd`
  enumeration may be blocked; the adapter falls back to null for `fds` with a
  `tracing::debug!` log rather than returning an error for the whole call.

### Risks

- The LRU delta cache retains a reference to a PID until eviction. If a PID
  is reused by the OS for a new process, the first `proc.stats` call for the
  new process will return a stale CPU delta. The `start_time` field in the
  response allows the caller to detect PID reuse.

## Validation

- Unit test: construct `ProcessStats` from a fixed `/proc/<pid>/stat` fixture;
  assert all field values match expected derived values.
- Unit test: call `proc.stats` for the same PID twice with a mocked clock
  advance of 100 ms; assert `cpu_pct` is non-zero on the second call.
- Unit test: assert `proc.top` with `limit=5` returns at most 5 entries and a
  non-null cursor when the process table has more than 5 entries.
- Integration test: call `proc.stats(getpid())` on a live server; assert
  `rss_bytes > 0`, `virt_bytes > 0`, `threads >= 1`.
- Integration test: call `proc.top` with `sort_by=mem` on Linux; assert the
  first entry has `rss_bytes >= rss_bytes` of subsequent entries.

## Links

- [ADR-0002](0002-bounded-contexts.md) — process bounded context definition
- [ADR-0028](0028-platform-feature-gates.md) — platform cfg-gate conventions
- [ADR-0035](0035-path-safety-hardening.md) — process PID safety constraints
- [ADR-0040](0040-async-job-control-plane.md) — Bucket A classification
