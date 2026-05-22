# Bounded Context: system-info

## Purpose

The system-info context provides read-only access to hardware and OS-level
metadata: kernel version and build information, system uptime, mounted
filesystem statistics, hostname, and CPU load averages. No tool in this context
modifies system state; all operations are idempotent. System-info tools are
typically called at the start of an agent session to establish environmental
context before invoking filesystem or process tools, and they are commonly used
in health-check and diagnostic workflows where the agent must report machine
state without taking action on it.

## Ubiquitous Language

The following terms have precise meanings within this context.

- **KernelVersion**: a structured record of the OS kernel identifier: name
  (e.g., `Linux`, `Darwin`), release string, version string, and machine
  architecture. Produced by `sys.uname`.
- **Uptime**: the duration, in seconds and as a human-readable string, since the
  system last booted. Produced by `sys.uptime`.
- **MountPoint**: a record describing a single mounted filesystem: device name,
  mount path, filesystem type, total bytes, used bytes, available bytes, and
  usage percentage. Produced by `sys.df`.
- **MemoryStats**: a sub-record of `SystemSnapshot` carrying total RAM, used
  RAM, free RAM, and swap totals in bytes.
- **HostName**: the short hostname of the machine as returned by the OS.
  Produced by `sys.hostname`.
- **LoadAverage**: a triplet of floating-point values representing the 1-minute,
  5-minute, and 15-minute exponential moving averages of the number of runnable
  processes. Produced by `sys.load_average`.
- **SystemSnapshot**: the aggregate root for `sys.info`: a composite record
  combining `KernelVersion`, `Uptime`, `MemoryStats`, and `LoadAverage` in a
  single response.

## Aggregates and Value Objects in Scope

Aggregates (owned by this context):

- `SystemSnapshot` - composite OS and hardware snapshot (produced by `sys.info`)

Value objects (from shared kernel):

- `AuditEvent` - emitted for tool invocations where observability requires it
  (read-only tools at INFO level, not marked `audit=true`)

## Tools Exposed

- `sys.info` - return a composite snapshot of kernel version, uptime, memory
  statistics, and load average in a single call; suitable for session
  initialization
- `sys.uptime` - return system uptime in seconds and as a human-readable
  duration string
- `sys.df` - list all mounted filesystems with capacity, usage, and percentage;
  equivalent to POSIX `df -h` output in structured form
- `sys.uname` - return the kernel name, release, version, and machine
  architecture; equivalent to POSIX `uname -a` in structured form
- `sys.hostname` - return the system hostname as reported by the OS
- `sys.load_average` - return the 1-, 5-, and 15-minute load averages as
  floating-point values

## Cross-references

- [ADR-0002](../../adr/0002-bounded-contexts.md) - defines this context and
  classifies all tools as zero mutation risk; no dry-run or elicitation applies
- [ADR-0004](../../adr/0004-security-model.md) - allowlist and path jail do not
  apply to this context (no path arguments); the outbound network feature flag
  is not used here
- [ADR-0005](../../adr/0005-stdio-transport.md) - all responses travel over the
  STDIO transport
- [ADR-0007](../../adr/0007-tool-card-narrative-arc.md) - tool cards carry
  `confirm_destructive: false` and `readOnlyHint: true` for all six tools
- [ADR-0010](../../adr/0010-error-taxonomy.md) - most likely error codes:
  `SUBSTRATE_TIMEOUT` (sysctl call exceeds deadline), `SUBSTRATE_INTERNAL_ERROR`
  (unexpected sysinfo failure)
- [ADR-0028](../../adr/0028-platform-feature-gates.md) - platform divergence is
  the defining characteristic of this context; Linux uses procfs and sysfs,
  macOS uses sysctl

## Platform Feature Gates

- **Memory statistics** (`sys.info`, `sys.df`): on Linux, the `sysinfo` crate
  reads `/proc/meminfo` and `/proc/mounts`. On macOS, it uses `host_statistics64`
  via `libproc`. Both paths are wrapped in Zone B `spawn_blocking`.
- **Load averages** (`sys.load_average`): on Linux, read from
  `/proc/loadavg`. On macOS, retrieved via `getloadavg(3)` through `nix`.
  The port trait returns an identical three-element array on both platforms.
- **Kernel version** (`sys.uname`): uses `nix::sys::utsname::uname()`, which
  maps to the POSIX `uname(2)` syscall available on both platforms.
- **Hostname** (`sys.hostname`): uses `nix::unistd::gethostname()`, available
  on both platforms.
- **Uptime** (`sys.uptime`): on Linux, parsed from `/proc/uptime`. On macOS,
  obtained via `sysctl kern.boottime` and subtracted from current time. Both
  return a `Duration` through the same port trait.
- **Disk free** (`sys.df`): on Linux, uses `statvfs(2)` via `nix` for each
  mount entry from `/proc/mounts`. On macOS, uses `getmntinfo(3)` via
  `nix::sys::statfs`. The `MountPoint` value object is identical on both platforms.

## Recent Amendments

- 2026-05-21 — system-info tools (`sys.info`, `sys.uptime`, `sys.df`,
  `sys.uname`, `sys.hostname`, `sys.load_average`) are Bucket-A sync-inline per
  [ADR-0040](../../adr/0040-async-job-control-plane.md). They use native procfs
  (Linux) and sysctl/libc bindings (macOS), with zero subprocess invocation per
  [ADR-0044](../../adr/0044-no-subprocess-policy.md).
