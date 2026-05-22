# Bounded Context: filesystem-query

## Purpose

The filesystem-query context provides read-only inspection of filesystem
metadata and content. Its tools answer questions about what exists, where it is,
how large it is, what it contains, and whether its integrity is intact. No tool
in this context modifies OS state; all operations are idempotent and carry zero
mutation risk. This makes filesystem-query the natural starting point for any
agent workflow that must discover or examine files before deciding whether to act
on them.

## Ubiquitous Language

The following terms have precise meanings within this context.

- **Entry**: a single filesystem object returned by a directory listing or
  recursive walk. Carries path, kind, size, and modification time.
- **Stat**: a snapshot of a single path's metadata: size, permissions, owner,
  timestamps, and file kind. Produced by `fs.stat`.
- **FileKind**: an enumeration distinguishing regular files, directories,
  symbolic links, and special files (device, socket, pipe).
- **Glob**: a pattern string (e.g., `*.rs`, `**/test_*.rs`) matched against
  entry names during a recursive walk.
- **PageCursor**: an opaque token returned when a result set is paginated;
  pass it back as `page_cursor` to retrieve the next page. Defined in the
  shared kernel.
- **DirectoryListing**: the aggregate root for a single `fs.read_dir` result:
  a bounded list of entries plus an optional next cursor.
- **StatResult**: the aggregate root for a single `fs.stat` result.
- **DiskUsageTree**: the aggregate root for a recursive hash or size report
  (produced indirectly through `fs.hash` applied to a tree).
- **Integrity**: the property that a file's content matches an expected digest;
  verified by `fs.hash`.

## Aggregates and Value Objects in Scope

Aggregates (owned by this context):

- `DirectoryListing` - collection of `Entry` values with pagination metadata
- `StatResult` - single-path metadata snapshot
- `ReadResult` - file content with byte range and encoding metadata

Value objects (from shared kernel):

- `JailedPath` - validated, allowlist-confirmed path passed to every OS call
- `PageCursor` - opaque pagination token

## Tools Exposed

- `fs.find` - recursive walk emitting entries matching a glob, mtime filter, or
  file kind; supports pagination via `page_cursor`
- `fs.read` - read file content as UTF-8 text or base64-encoded bytes, with
  optional byte-range selection
- `fs.read_dir` - list the immediate children of a directory; returns entries
  with kind, size, and modification time
- `fs.stat` - retrieve metadata for a single path: size, permissions, owner,
  timestamps, and file kind
- `fs.hash` - compute a content digest (BLAKE3 or SHA-256) for a file and
  return the hex-encoded result; useful for integrity verification before and
  after mutations

## Cross-references

- [ADR-0002](../../adr/0002-bounded-contexts.md) - defines this context and its
  mutation-risk classification (none)
- [ADR-0004](../../adr/0004-security-model.md) - allowlist and path jail apply
  to every path argument even for read-only tools
- [ADR-0005](../../adr/0005-stdio-transport.md) - all tool responses travel over
  the STDIO transport
- [ADR-0007](../../adr/0007-tool-card-narrative-arc.md) - narrative-arc template
  governs tool descriptions for all five tools
- [ADR-0010](../../adr/0010-error-taxonomy.md) - `SUBSTRATE_NOT_FOUND`,
  `SUBSTRATE_PATH_OUTSIDE_ALLOWLIST`, and `SUBSTRATE_TIMEOUT` are the primary
  error codes for this context
- [ADR-0025](../../adr/0025-bounded-context-interactions.md) - shared kernel
  value objects (`JailedPath`, `PageCursor`) cross the boundary here
- [ADR-0028](../../adr/0028-platform-feature-gates.md) - `fs.hash` uses Zone C
  (`blake3` with `mmap`/`rayon`) behind a CPU semaphore; `fs.read_dir` and
  `fs.stat` use Zone A (`tokio::fs`)

## Platform Feature Gates

- **BLAKE3 hashing** (`fs.hash`): uses the `rayon` and `mmap` features of the
  `blake3` crate, which exploit multiple CPU cores and memory-mapped I/O. This
  is Zone C work; it runs inside `spawn_blocking` behind a
  `Semaphore(num_cpus)`. Available on both Linux and macOS.
- **Directory walking** (`fs.find`): uses the `ignore` crate (sync iterator)
  wrapped in Zone B `spawn_blocking`. The `ignore` crate respects `.gitignore`
  files on both platforms identically.
- **File metadata** (`fs.stat`, `fs.read_dir`): uses `tokio::fs::metadata` and
  `tokio::fs::read_dir` (Zone A) on both platforms. Symbolic link handling uses
  `nix::sys::stat::lstat` for the `is_symlink` field.
- There are no Linux-only or macOS-only code paths in this context for MVP.
  Platform gates for procfs or sysctl are limited to the process and system-info
  contexts.

Under the `fs-index` Cargo feature (default OFF), per-OS adapters are introduced
(Linux: `statx`/`openat2`/`inotify`; macOS: `getattrlistbulk`/FSEvents/`O_NOFOLLOW_ANY`)
selected at runtime by the capability factory; see
[ADR-0041](../../adr/0041-filesystem-index-native-tiers.md) and
[ADR-0042](../../adr/0042-capability-adapter-factory.md). The invariant above
applies to the default-feature MVP only.

## Recent Amendments

- 2026-05-21 — `fs.find` now supports an optional filesystem index
  ([ADR-0041](../../adr/0041-filesystem-index-native-tiers.md)) under the
  `fs-index` Cargo feature; lazy lstat + write-through + watcher + TTL +
  snapshot atomic-swap; native per-OS adapters via the capability factory
  ([ADR-0042](../../adr/0042-capability-adapter-factory.md)); Bucket-B auto-mode
  dispatch ([ADR-0040](../../adr/0040-async-job-control-plane.md)) when result
  set exceeds `inline_max_entries`.
