# Operator Guide

## Purpose

This guide collects the operator-facing contracts that the architecture
decision records defer to runtime configuration and deployment practice. Each
section names the threat or caveat it addresses, the substrate behavior the
operator can rely on, and the action the operator must take to close the
residual gap. The guide is the canonical reference target for the deferred
contracts in [ADR-0029](../adr/0029-threat-model.md) (subprocess threat
expansion) and [ADR-0017](../adr/0017-concurrency-limits.md) (concurrency
limits).

## Subprocess privilege dropping (E-NEW-1)

Threat E-NEW-1 in [ADR-0029](../adr/0029-threat-model.md) is that a spawned
child inherits substrate's UID, GID, and OS capabilities, allowing the child to
act with substrate's full filesystem and process privileges.

The optional configuration key `subprocess.drop_privs_to_uid` instructs
substrate to call `setuid(target_uid)` in the `pre_exec` hook before `exec`,
dropping the child to a less-privileged UID. The operator contract is:

- Run substrate under an account that is permitted to drop to the target UID.
  `setuid` to an arbitrary UID requires substrate to start with sufficient
  privilege (typically running as root or holding `CAP_SETUID` on Linux). A
  drop-privs configuration without that starting privilege fails the spawn.
- Choose a `target_uid` that owns no sensitive files and holds no extra
  capabilities. The intent is to confine the child to a minimal account, so the
  target UID should be a dedicated, unprivileged service account.
- When `drop_privs_to_uid` is left unset, child processes run as the same user
  as substrate. This is the default and is documented here as a residual risk:
  the binary allowlist and the environment-variable allowlist (Layer 5 of
  [ADR-0004](../adr/0004-security-model.md)) reduce but do not eliminate the
  privilege surface. Operators who spawn binaries that should not hold
  substrate's privileges MUST set `drop_privs_to_uid`.
- Privilege dropping does not replace the binary allowlist. Both controls are
  required: the allowlist limits which programs may run, and the UID drop limits
  what a running program may touch.

## Subprocess orphan cleanup on Linux (D-NEW-4)

Threat D-NEW-4 in [ADR-0029](../adr/0029-threat-model.md) is that a `SIGKILL`
delivered to substrate on Linux leaves the child running as an orphan adopted by
init or systemd.

Substrate sets `PR_SET_PDEATHSIG(SIGTERM)` in the child's `pre_exec` hook before
`exec`, per [ADR-0053](../adr/0053-process-lifecycle-cascade-contract.md). When
the substrate parent dies, the kernel delivers `SIGTERM` to the child
automatically. The operator contract is:

- Child binaries spawned through substrate MUST implement a `SIGTERM` cleanup
  handler. A child that ignores `SIGTERM` will continue running after substrate
  dies; the kernel signal is delivered but not acted upon. The cleanup handler
  should release resources, flush state, and exit promptly.
- `PR_SET_PDEATHSIG` is reset across an intervening `exec` only in the child
  itself; it is configured immediately before `exec` so that the death-signal
  association is in place for the lifetime of the child.
- This control is Linux-specific. On macOS the equivalent protection is the
  watchdog pipe described in the next section.
- The cleanup latency the child achieves is bounded by its own handler. To
  guarantee termination of a non-cooperative child, rely on the startup orphan
  reaper ([ADR-0055](../adr/0055-orphan-reaper-on-startup.md)) rather than on the
  death signal alone.

## Subprocess orphan cleanup on macOS (D-NEW-5)

Threat D-NEW-5 in [ADR-0029](../adr/0029-threat-model.md) is that a `SIGKILL`
delivered to substrate on macOS leaves the child running as an orphan, because
`PR_SET_PDEATHSIG` is not available on macOS.

Substrate establishes a cooperative watchdog pipe between itself and each child
at spawn, per [ADR-0053](../adr/0053-process-lifecycle-cascade-contract.md).
Substrate holds the write end open; the child holds the read end. When the
substrate process dies, the write end closes and a child monitoring the read end
receives EOF and self-terminates. The operator contract is:

- Child binaries spawned on macOS that need prompt orphan cleanup MUST monitor
  the watchdog pipe read end and exit on EOF. The pipe is cooperative: a child
  that never reads the pipe will not notice substrate's death.
- A non-cooperative child that ignores the watchdog pipe is a residual risk. The
  backstop is the startup orphan reaper
  ([ADR-0055](../adr/0055-orphan-reaper-on-startup.md)): at the next substrate
  startup, the reaper scans for processes matching the watchdog-pipe fingerprint
  and terminates them. Orphans therefore survive only until the next substrate
  restart, not indefinitely.
- Operators who run only well-behaved, cooperative child binaries get prompt
  EOF-driven cleanup. Operators who must run binaries they do not control should
  schedule periodic substrate restarts, or prefer the macOS deployment knowing
  the reaper bounds orphan lifetime to the inter-restart window.

## blake3 rayon thread-pool caveat (ADR-0017)

[ADR-0017](../adr/0017-concurrency-limits.md) bounds CPU-bound work with a
Zone C semaphore sized to `num_cpus::get()` permits, configurable via
`[concurrency] cpu_permits`. The semaphore limits the number of simultaneous
blake3 hashing calls, but it does not limit the threads each call spins up.

blake3 uses `rayon` internally; a single blake3 call may spawn rayon worker
threads that are not subject to substrate's semaphore. The operator contract is:

- The `cpu_permits` semaphore caps concurrent blake3 calls, not concurrent
  rayon threads. With `cpu_permits = N`, up to N blake3 calls run at once, and
  each may use the rayon thread pool, so the true thread count can exceed N.
- `num_cpus::get()` counts hyper-threaded logical cores, not physical cores. On
  hyper-threaded hosts, N CPU-bound tasks may still cause visible latency
  spikes. Operators who observe contention should reduce `cpu_permits` below the
  logical-core count.
- To bound hashing concurrency tightly, configure a per-tool semaphore on the
  hashing tool, for example:

  ```toml
  [tools.hash_file.concurrency]
  max_concurrent = 0       # 0 inherits cpu_permits (Zone C default)
  ```

  Set `max_concurrent` to an explicit value below `cpu_permits` to throttle
  blake3 calls independently of other Zone C work.
- The rayon thread pool is shared process-wide across blake3 calls; it is not
  per-call. Reducing `cpu_permits` is the primary lever an operator has over
  total hashing CPU pressure.

## Cross-references

- [ADR-0029](../adr/0029-threat-model.md) - STRIDE-Lite threat model; source of
  E-NEW-1, D-NEW-4, and D-NEW-5
- [ADR-0017](../adr/0017-concurrency-limits.md) - concurrency limits; source of
  the blake3 rayon caveat
- [ADR-0004](../adr/0004-security-model.md) - security model; Layer 5 subprocess
  sandbox referenced by the privilege-drop contract
- [ADR-0053](../adr/0053-process-lifecycle-cascade-contract.md) -
  `PR_SET_PDEATHSIG` and macOS watchdog pipe
- [ADR-0055](../adr/0055-orphan-reaper-on-startup.md) - startup orphan reaper
  backstop for non-cooperative children
