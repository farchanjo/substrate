---
status: accepted
accepted_date: 2026-05-24
date: 2026-05-24
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0055 — Orphan Reaper on Startup

## Context and Problem Statement

[ADR-0053](0053-process-lifecycle-cascade-contract.md) establishes the cascade
kill contract for subprocess lifecycle. That contract handles cleanup in the
common case: cooperative cancellation, graceful SIGTERM drain, and SIGKILL as a
last resort. It does not fully address the crash scenario on macOS, where
`PR_SET_PDEATHSIG` is unavailable and the watchdog pipe pattern is cooperative.

When substrate is killed with SIGKILL on macOS (or crashes unexpectedly on any
platform), two classes of orphan resources may remain:

- **Orphan temporary files**: `.tmp.<uuid7>` files left by interrupted
  transactional writes (per [ADR-0033](0033-transactional-write-pattern.md))
  and `.substrate-subprocess-stream-*.tmp` staging artifacts.
- **Orphan subprocess children** (macOS only): processes launched via
  `subprocess.spawn` that did not receive EOF on their watchdog pipe and
  therefore did not self-terminate.

Without a reaper, these resources accumulate across restarts and may consume
disk space or CPU indefinitely.

The question is: at what point in the startup sequence should cleanup run, what
should be reaped, and what are the constraints on the reaper itself?

## Decision Drivers

- Cleanup must be non-fatal; a reaper failure must not prevent the server from
  accepting requests.
- The reaper must run after the capability probe (per
  [ADR-0042](0042-capability-adapter-factory.md)) so it can use the same
  per-platform detection results.
- The reaper must complete before the first MCP `initialize` is accepted so
  that orphan tmp files do not interfere with in-flight writes during the new
  session.
- On macOS, the child reaper must not kill processes that are still attached
  to a live substrate instance (if two instances start simultaneously).
- All reaper actions must be recorded in the audit trail per
  [ADR-0038](0038-audit-event-semantics.md).

## Considered Options

- Option A: No reaper; document that operators must manually clean up after
  crashes.
- Option B: Startup reaper that runs once, before accepting requests, with
  bounded duration (selected).
- Option C: Background periodic reaper running throughout the server lifetime.
- Option D: Reaper implemented as a separate binary invoked by a system daemon.

## Decision Outcome

Chosen option: "Option B — single-shot startup reaper with bounded duration",
because it cleans up the most common crash remnants without adding background
overhead to the running server (Option C) and without requiring additional
distribution artifacts (Option D).

### Trigger and Placement in Startup Sequence

The reaper runs once during substrate startup in the following position:

1. Config load.
2. Capability probe (`probe_capabilities`, per ADR-0042).
3. PathJail initialization and refuse-degraded check.
4. **Orphan reaper runs here.**
5. rmcp service starts accepting `initialize`.

The reaper must complete within `startup.orphan_reap_max_duration_secs`
(default 30 s). If this budget is exceeded, the reaper is abandoned and startup
continues. A `tracing::warn!` is emitted identifying any reaper step that was
abandoned.

### Singleton Lock

To prevent two substrate instances starting simultaneously from reaping each
other's live state, the reaper acquires a file-based lock before proceeding:

- Lock path: `<first-allowed_path>/.substrate-reaper.lock` where
  `<first-allowed_path>` is the first entry in the `security.allowed_paths`
  configuration list. If no `allowed_paths` is configured, the reaper is
  skipped entirely.
- Lock mechanism: `nix::fcntl::flock` with `LOCK_EX | LOCK_NB`. If the lock
  cannot be acquired (another instance holds it), the reaper skips silently
  (logging at `tracing::debug!`) and startup continues.
- The lock is released when the reaper function returns, before the rmcp
  service starts.

### Tmp File Reaper

The following diagram shows the decision tree for the orphan reaper.

```mermaid
flowchart TD
    START[startup reaper begins] --> LOCK{acquire flock on\n.substrate-reaper.lock}
    LOCK -- failed lock another instance -- > SKIP[skip reaper log debug]
    LOCK -- acquired --> TMPREAP[tmp file reaper]

    TMPREAP --> SCAN[for each root in policy.roots\nscan for glob *.tmp.uuid7 pattern]
    SCAN --> AGE{file age\n> orphan_reap_age_secs?}
    AGE -- No --> NEXT[skip this file]
    AGE -- Yes --> REMOVE[tokio fs remove_file]
    REMOVE --> AUDIT1[emit SUBSTRATE_ORPHAN_TMP_REAPED\npath age_secs size_bytes]
    AUDIT1 --> NEXT

    TMPREAP --> STREAMSCAN[scan for .substrate-subprocess-stream-*.tmp]
    STREAMSCAN --> SAGECHECK{file age\n> orphan_reap_age_secs?}
    SAGECHECK -- No --> SNEXT[skip]
    SAGECHECK -- Yes --> SREMOVE[tokio fs remove_file]
    SREMOVE --> AUDIT2[emit SUBSTRATE_ORPHAN_TMP_REAPED stream variant]

    TMPREAP --> CHILDREAP{macOS only}
    CHILDREAP -- Linux --> DONE[release lock]
    CHILDREAP -- macOS --> PROCLIST[scan process table via\nsysctl KERN_PROC_ALL\nfor SUBSTRATE_WATCHDOG_FD in env]

    PROCLIST --> PARENT{parent PID matches\nlive substrate instance?}
    PARENT -- Yes --> SKIP2[skip this process]
    PARENT -- No --> KILLPG[killpg pgid SIGTERM]
    KILLPG --> DRAIN[sleep cascade_drain_secs]
    DRAIN --> ALIVE{still alive?}
    ALIVE -- No --> AUDIT3[emit SUBSTRATE_ORPHAN_CHILD_REAPED]
    ALIVE -- Yes --> KILLPG2[killpg pgid SIGKILL]
    KILLPG2 --> AUDIT3

    AUDIT3 --> DONE
    DONE --> RMCP[accept MCP initialize]
```

**Scan scope**: each entry in `policy.roots` (the configured `allowed_paths`
list) is scanned non-recursively for files matching the pattern
`*.tmp.<uuid7>` where `<uuid7>` matches the regular expression
`[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}`.
Additionally, files matching `".substrate-subprocess-stream-*.tmp"` are scanned
regardless of UUIDv7 format.

**Age threshold**: files must be older than `startup.orphan_reap_age_secs`
(default 600 s = 10 minutes) to be reaped. This prevents the reaper from
removing tmp files belonging to a concurrent in-flight write in another
(legitimately running) substrate instance. The 10-minute threshold is
conservative; the transactional write pattern in ADR-0033 removes its tmp files
on completion, error, or cancellation; surviving files are almost certainly
from a crash.

**New audit events**:

- `SUBSTRATE_ORPHAN_TMP_REAPED` — emitted for each removed tmp file.
  Payload: `{path, age_secs, size_bytes, file_kind: "write_tmp" | "stream_tmp"}`.

### macOS Child Reaper

On macOS only, the reaper scans the process table for orphaned subprocess
children. The scan uses `sysctl(CTL_KERN, KERN_PROC, KERN_PROC_ALL)` to obtain
a list of `kinfo_proc` entries, then calls `proc_pidinfo(pid, PROC_PIDENVNAME)`
to retrieve the environment of each process. Processes that satisfy all of the
following criteria are considered orphan substrate children:

- Their environment contains the key `SUBSTRATE_WATCHDOG_FD`.
- Their parent PID (`kinfo_proc.p_ppid`) does not match the PID of any
  currently running `substrate-mcp-server` process (verified via a secondary
  `sysctl(KERN_PROC_PID, parent_pid)` call checking the binary name).
- Their process age (current time minus `kinfo_proc.p_starttime`) is greater
  than `startup.orphan_reap_age_secs`.

For each matching process:

1. `nix::sys::signal::killpg(pgid, Signal::SIGTERM)` is called.
2. Sleep `subprocess.cascade_drain_secs` (default: `runtime.shutdown_drain_secs`).
3. If still alive: `nix::sys::signal::killpg(pgid, Signal::SIGKILL)`.
4. Emit `SUBSTRATE_ORPHAN_CHILD_REAPED` audit event.

**New audit event**:

- `SUBSTRATE_ORPHAN_CHILD_REAPED` — emitted for each reaped orphan child.
  Payload: `{pid, pgid, command, parent_pid, age_secs, kill_required: bool}`.

**Safety constraint**: the reaper MUST NOT call `killpg` on a `pgid` that
belongs to a process whose parent PID is a live substrate instance. The parent
PID check described above enforces this. If the parent PID check cannot be
resolved (for example, because the parent has exited and the PID has been
recycled to another process), the reaper skips the kill and logs
`tracing::warn!` with the ambiguous PID.

### New Config Keys

- `startup.orphan_reap_age_secs` — minimum file or process age in seconds
  before reaping is performed (default 600).
- `startup.orphan_reap_max_duration_secs` — wall-clock budget for the entire
  reaper run (default 30). Reaper is abandoned after this duration; startup
  continues.

### Interaction with subprocess Feature Gate

The macOS child reaper is compiled only when the `subprocess` Cargo feature is
active. When `subprocess` is not enabled, only the tmp file reaper runs.
The tmp file reaper is always active (tmp files may be created by the
transactional write pattern even without the subprocess feature).

## Consequences

### Positive

- Orphan tmp files from crashed writes are cleaned up automatically without
  operator intervention.
- On macOS, orphan subprocess children from a crashed substrate instance are
  terminated before the new instance accepts requests, preventing resource
  accumulation.
- The singleton lock prevents multiple simultaneous startups from interfering
  with each other's live state.
- All reaper actions are recorded in the audit trail, providing forensic
  visibility into what was cleaned up and why.

### Negative

- Scanning process environments on macOS via `proc_pidinfo(PROC_PIDENVNAME)` is
  a privileged operation; it may return `EPERM` for processes owned by other
  users. The reaper skips such processes and logs at `tracing::debug!`.
- The 30-second reaper budget adds up to 30 s to the worst-case startup time
  in an environment with many orphan processes. Operators can reduce this via
  `startup.orphan_reap_max_duration_secs`.
- The 10-minute age threshold means tmp files from a crash in the last 10
  minutes are not reaped on the next startup. This is intentional to avoid
  interfering with concurrent instances.

### Risks

- The parent PID reuse race: if a substrate process exits and its PID is reused
  by an unrelated process before the reaper runs, the reaper may incorrectly
  consider an orphan as having a live substrate parent and skip the kill.
  Mitigation: the reaper also checks the binary name (`kinfo_proc.p_comm`)
  of the parent PID; if the name does not match `substrate-mcp-server`, the
  parent check is disregarded and the orphan is reaped.

## Validation

- Unit test: place two files matching `*.tmp.<uuid7>` under a configured root
  (one older than `orphan_reap_age_secs`, one newer); assert only the older
  file is removed and `SUBSTRATE_ORPHAN_TMP_REAPED` is emitted exactly once.
- Unit test: place a stream tmp file older than the age threshold; assert it
  is removed and `SUBSTRATE_ORPHAN_TMP_REAPED` is emitted with
  `file_kind="stream_tmp"`.
- Unit test (macOS): mock `sysctl KERN_PROC_ALL` to return a process with
  `SUBSTRATE_WATCHDOG_FD` in its env and a parent PID that does not match any
  substrate instance; assert `killpg(SIGTERM)` is called and
  `SUBSTRATE_ORPHAN_CHILD_REAPED` is emitted.
- Unit test (macOS): mock a process whose parent PID matches a live substrate
  binary; assert no kill is sent.
- Integration test: start substrate with the `subprocess` feature; kill it
  with `SIGKILL` leaving a mock orphan process; restart; assert the orphan is
  reaped and `SUBSTRATE_ORPHAN_CHILD_REAPED` appears in the audit log before
  the first `notifications/progress` event.

## Links

- [ADR-0014](0014-build-system-and-toolchain.md) — panic=abort; startup sequence
- [ADR-0032](0032-signal-safety.md) — signal safety; SIGTERM/SIGINT drain
- [ADR-0033](0033-transactional-write-pattern.md) — transactional write pattern; .tmp.<uuid7> naming
- [ADR-0038](0038-audit-event-semantics.md) — audit event semantics
- [ADR-0052](0052-subprocess-execution-architecture.md) — subprocess BC architecture
- [ADR-0053](0053-process-lifecycle-cascade-contract.md) — cascade kill contract; watchdog pipe
- [ADR-0068](0068-launch-detached-supervisor-and-orphan-governance.md) — launch reaper extends this file-only reaper to a process adopt-or-reap reaper

## Amendment — 2026-06-30 — Extended to a process adopt-or-reap reaper by the launch BC (ADR-0068)

[ADR-0068](0068-launch-detached-supervisor-and-orphan-governance.md) extends the startup reaper recorded here from a temporary-file-only reaper to one that also adopts or reaps **detached supervised processes** on boot. The launch supervisor records each child with a `start_epoch` start-time pin; the reaper re-validates the live start-time before any adopt/re-attach/`killpg`, treating a mismatch as pid recycling (`SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED`, no signal sent). The temporary-file reaping recorded in this ADR is unchanged and runs alongside the process reaper.
