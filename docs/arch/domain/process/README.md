# Bounded Context: process

## Purpose

The process context provides inspection and control of running OS processes.
Its read tools give agents a snapshot of the current process tree, resource
usage, and parent-child relationships without modifying system state. Its
control tool, `proc.signal`, delivers POSIX signals to processes and carries
high mutation risk: a SIGKILL or SIGTERM is irreversible in its effect on the
target process. For this reason, `proc.signal` requires both a dry-run pass and
elicitation confirmation for SIGKILL, SIGTERM, and SIGSTOP. Agents should call
`proc.list` or `proc.tree` first to confirm the target PID and its identity
before issuing any signal.

## Ubiquitous Language

The following terms have precise meanings within this context.

- **Pid**: a process identifier, represented as a non-negative integer. Pids are
  ephemeral; the same integer may refer to a different process after a restart.
- **ProcessSnapshot**: a point-in-time record of a single process: PID, parent
  PID, command name, arguments, CPU usage, RSS, virtual memory, and state.
  Produced by `proc.list`.
- **Signal**: a POSIX signal number or symbolic name (e.g., `SIGTERM`, `15`)
  to be delivered to a target process. Signals with irreversible effects
  (SIGKILL, SIGTERM, SIGSTOP) require elicitation.
- **ProcessFilter**: a predicate applied to the process list to reduce results:
  by PID, by command name substring, by UID, or by ancestor PID.
- **ResourceUsage**: a sub-record within `ProcessSnapshot` carrying CPU
  percentage, RSS bytes, virtual memory bytes, and open file descriptor count.
- **ProcessList**: the aggregate root for a `proc.list` result: a collection of
  `ProcessSnapshot` values, optionally filtered, with pagination metadata.
- **ProcessHandle**: the aggregate root for a `proc.tree` result: a rooted tree
  of `ProcessSnapshot` nodes linked by parent-child PID relationships.

## Aggregates and Value Objects in Scope

Aggregates (owned by this context):

- `ProcessList` - filtered collection of process snapshots
- `ProcessHandle` - rooted process tree

Value objects (from shared kernel):

- `AuditEvent` - emitted after every `proc.signal` delivery

## Tools Exposed

- `proc.list` - list running processes with their PID, command, resource usage,
  and state; supports filtering by PID, name, or UID and pagination
- `proc.tree` - return the full parent-child process hierarchy rooted at a
  given PID (default: PID 1), with resource usage at each node
- `proc.signal` - deliver a POSIX signal to a process by PID; dry-run mode
  returns the signal name and target command without sending; elicitation
  required for SIGKILL, SIGTERM, and SIGSTOP

## Cross-references

- [ADR-0002](../../adr/0002-bounded-contexts.md) - defines this context;
  classifies read tools as zero mutation risk and `proc.signal` as high risk
  requiring elicitation for destructive signals
- [ADR-0004](../../adr/0004-security-model.md) - elicitation layer applies to
  `proc.signal` for SIGKILL, SIGTERM, and SIGSTOP; dry-run gate applies for all
  signal deliveries
- [ADR-0005](../../adr/0005-stdio-transport.md) - elicitation requests and tool
  responses travel over the STDIO channel
- [ADR-0007](../../adr/0007-tool-card-narrative-arc.md) - `proc.signal` tool
  card carries `confirm_destructive: true`; NEXT hints from `proc.list` and
  `proc.tree` point to `proc.signal` for workflow chaining
- [ADR-0010](../../adr/0010-error-taxonomy.md) - key error codes:
  `SUBSTRATE_NOT_FOUND` (PID not found), `SUBSTRATE_PERMISSION_DENIED` (signal
  to privileged PID), `SUBSTRATE_CONFIRMATION_REQUIRED` (destructive signal
  awaiting elicitation)
- [ADR-0025](../../adr/0025-bounded-context-interactions.md) - process context
  has no shared aggregates with other contexts; PIDs passed from `proc.list`
  output to `proc.signal` input are plain integers at the composition root
- [ADR-0028](../../adr/0028-platform-feature-gates.md) - platform divergence is
  significant here: Linux uses procfs, macOS uses sysctl

## Platform Feature Gates

- **Process enumeration** (`proc.list`, `proc.tree`): on Linux, the `procfs`
  crate reads `/proc/<pid>/status`, `/proc/<pid>/stat`, and `/proc/<pid>/fd`
  directly. On macOS, the `sysinfo` crate uses `libproc` and `sysctl` calls.
  Both paths are wrapped in Zone B `spawn_blocking` and expose an identical port
  trait so domain code is platform-neutral.
- **Per-thread CPU time**: available via `/proc/<pid>/task/` on Linux only.
  The `ProcessSnapshot` field `thread_cpu_times` is populated on Linux and
  returns an empty list on macOS without error.
- **Open file descriptor count** (`ResourceUsage.fd_count`): on Linux, counted
  from `/proc/<pid>/fd/` directory entries. On macOS, approximated from
  `proc_pidinfo` via `sysinfo`. Values may differ by a small constant due to
  platform accounting differences.
- **Signal delivery** (`proc.signal`): uses `nix::sys::signal::kill`, which is
  POSIX and available on both platforms.
- **Memory-mapped region listing**: deferred to a future release; requires
  `/proc/<pid>/maps` on Linux and `vmmap` on macOS.

## Recent Amendments

- 2026-05-21 — `proc.list`, `proc.tree`, and `proc.signal` remain Bucket-A
  sync-inline tools per [ADR-0040](../../adr/0040-async-job-control-plane.md)
  (snapshot-instant). `proc.signal` continues to require elicitation for
  SIGKILL/SIGTERM/SIGSTOP per the security model. The no-subprocess policy
  ([ADR-0044](../../adr/0044-no-subprocess-policy.md)) explicitly carves out
  process introspection: substrate observes processes via `/proc` and `kill(2)`,
  never spawns them.
