---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0033 — Transactional Write Pattern (Temp + Atomic Rename + Cleanup)

## Context and Problem Statement

Substrate performs disk mutations on behalf of LLM agents: `archive.tar.create`, `archive.zip.extract`, `fs.write`, `fs.copy`, and any future tool that writes to the filesystem. These operations may be interrupted mid-flight by:

- Operator cancellation via `CancellationToken` propagation.
- Out-of-disk-space errors (ENOSPC) that leave a partially written file.
- Process crash or panic during a multi-step write sequence.

A partial file is indistinguishable from a complete file by name alone. If a subsequent tool call — or a retry of the same call — reads the partial output, it may act on corrupt data, corrupt an archive member list, or silently produce garbage output. There is no safe recovery path for a partial write that has already been observed under the final target path.

## Decision Drivers

- Partial files must never become visible to subsequent tool calls under the intended target name.
- Cancellation must leave the filesystem in a clean state without operator intervention.
- ENOSPC errors must be reported before any bytes are written, where possible, rather than after a partial write.
- Cross-device rename failures (EXDEV) must be structurally prevented.
- Cleanup must be best-effort and must not block error delivery to the client.

## Considered Options

1. Temp-file + atomic rename + cleanup-on-cancel (selected)
2. Write to target directly; truncate on failure
3. Write to target directly; delete on failure
4. Shadow copy: write to a parallel `.bak` file; swap on success

## Decision Outcome

Chosen option: "Temp-file + atomic rename + cleanup-on-cancel", because `rename(2)` is atomic on POSIX same-filesystem paths, and writing to a `.tmp` sibling guarantees the target name is either absent or fully written — never partially populated.

### Mandatory pattern for all write-to-disk operations

Every operation in substrate that writes bytes to disk MUST follow this sequence:

#### Step 1 — Compute target and create parent

Resolve the target path through `strict-path` (ADR-0004). Create parent directories with `tokio::fs::create_dir_all` if they do not exist.

#### Step 2 — Open temp file in the same directory

Construct the temp filename as `<target_filename>.tmp.<uuid7>` and open it in the same directory as the target. Using the same directory guarantees both paths are on the same filesystem, preventing EXDEV failures when `rename(2)` is called in step 4.

```
<target_dir>/<filename>.tmp.<uuid7>
```

The UUID7 suffix ensures uniqueness even under concurrent operations targeting the same output file.

#### Step 3 — Register cleanup callback

Before writing any bytes, register a cleanup callback that removes the `.tmp` file. The cleanup is wired into two separate paths:

- A `tokio::select!` arm racing the write future against the `CancellationToken`. If cancellation fires, the write future is dropped and the cleanup arm runs.
- The `?` propagation path for any error returned by the write operation.

This ordering guarantees that cleanup runs regardless of whether the termination is cooperative (cancellation) or exceptional (IO error, panic guard).

#### Step 4 — Write content

Stream or write the full content to the `.tmp` file. For archive operations, all members are written before step 5.

#### Step 5 — Atomic rename on success

On successful completion of all writes:

```rust
tokio::fs::rename(&tmp_path, &target_path).await?;
```

`rename(2)` is atomic on POSIX for same-filesystem paths. The target path transitions from absent (or its previous content) to the complete new file in a single kernel operation. No intermediate state is visible.

#### Step 6 — Cleanup on cancellation or error

When the cancellation token fires or the write returns an error:

```rust
if let Err(e) = tokio::fs::remove_file(&tmp_path).await {
    tracing::warn!(path = %tmp_path, error = %e, "cleanup of tmp file failed");
}
```

Cleanup failure is logged at WARN but does not alter the error returned to the client. Cleanup runs before the error response is sent, with a best-effort timeout of one second. A cleanup that exceeds one second is abandoned; the error response is sent immediately.

### Archive extraction variant

For `archive.zip.extract` and `archive.tar.extract`, the unit of atomicity is the extraction root directory, not individual member files:

1. Compute `<target_root>.tmp.<uuid7>/` as the temp extraction directory in the same parent as the intended root.
2. Extract all archive members into the temp directory.
3. On all-members-success: `tokio::fs::rename(&tmp_dir, &target_root).await`.
4. On any member error or cancellation: `tokio::fs::remove_dir_all(&tmp_dir).await` (logged on failure; does not block error delivery).

This prevents a partially extracted directory tree from being visible under the final target name.

### Disk-space preflight

Before opening any `.tmp` file, substrate MUST perform a disk-space preflight:

1. Determine the target directory (or its first existing ancestor).
2. Call `statvfs` (via the `nix` crate on POSIX platforms) to obtain `f_bavail` (available blocks) and `f_bsize` (block size).
3. Compute available bytes as `f_bavail * f_bsize`.
4. Compare against `projected_output_bytes * 1.10` (ten percent safety margin).
5. If available space is insufficient, return `SUBSTRATE_STORAGE_FULL` ([ADR-0034](0034-kernel-induced-error-codes.md)) immediately, before creating the `.tmp` file.

The projected size is the uncompressed output size for extraction, the source file size for copy, and the caller-supplied or measured size for write. When the size is unknown (streaming write), the preflight is skipped and ENOSPC is handled as a write error in step 6.

### Cleanup ordering guarantee

Cleanup MUST complete (or time out at one second) before the error response is dispatched to the MCP client. This ordering ensures that by the time the client receives the error and issues a retry or diagnostic call, the `.tmp` file is no longer present.

### Consequences

#### Positive

- The target path is either absent or fully written; no partial state is observable.
- Cancellation leaves no `.tmp` debris in the common case (one-second best-effort cleanup).
- ENOSPC is reported before any bytes are written when output size is known.
- The pattern composes with archive members: all-or-nothing extraction is the default.

#### Negative

- Disk usage temporarily peaks at `2x output_size` during the write phase (source and `.tmp` coexist).
- Single-second cleanup timeout means `.tmp` files may persist if the cleanup itself encounters a slow or unresponsive filesystem.
- `statvfs` preflight adds one syscall per write-tool invocation on POSIX; not available on Windows (future platform gate required).

## Validation

- Unit tests inject a `CancellationToken` mid-write and assert that no `.tmp` file remains and no partial target file is created.
- Unit tests simulate ENOSPC by mocking `statvfs` and assert `SUBSTRATE_STORAGE_FULL` is returned before any file is opened.
- Integration tests exercise `archive.zip.extract` cancellation mid-member and verify the temp extraction directory is removed.
- Property tests generate adversarial filenames for the UUID7 suffix and verify no collision or path escape occurs.

## Cross-References

- [ADR-0004](0004-security-model.md) — Security model (path jail applied in step 1)
- [ADR-0016](0016-resource-limits.md) — Resource limits (size limits enforced before preflight)
- [ADR-0034](0034-kernel-induced-error-codes.md) — Kernel-induced error codes (`SUBSTRATE_STORAGE_FULL`)
- [ADR-0037](0037-async-cancellation-patterns.md) — Cancellation patterns (CancellationToken propagation)

## Amendments

### 2026-05-24 — Subprocess stream-capture tmp files use same pattern (ADR-0052/ADR-0054)

[ADR-0052](0052-subprocess-execution-architecture.md) and [ADR-0054](0054-subprocess-stream-capture.md) extend the transactional write pattern to subprocess stream capture. When `subprocess.spawn` is called with `capture_kind: tmp_file`, the stdout and stderr streams of the child process are written to temporary files using the same pattern mandated by this ADR.

Temporary file naming convention for subprocess stream capture:

```
<capture_root>/.substrate-subprocess-stream-<job_id>.<stream>.tmp.<uuid7>
```

where `<stream>` is `stdout` or `stderr`, `<job_id>` is the UUIDv7 job identifier, and `<uuid7>` is a fresh UUIDv7 generated at file creation time. The `<capture_root>` is a directory under an entry in `security.allowed_paths`. Both the stdout and stderr temporary files are created together before the child process is spawned, so that cleanup registration covers both files regardless of which stream becomes active first.

On terminal `Succeeded` state, the temporary files are atomically renamed to their final names by removing the `.tmp.<uuid7>` suffix:

```
<capture_root>/.substrate-subprocess-stream-<job_id>.stdout
<capture_root>/.substrate-subprocess-stream-<job_id>.stderr
```

On `Cancelled`, `Failed`, or SIGKILL-forced termination, both temporary files are removed via `tokio::fs::remove_file` in the cancellation cleanup path. This cleanup MUST NOT be implemented as a `Drop` impl, consistent with the `panic = "abort"` constraint from [ADR-0014](0014-build-system-and-toolchain.md) which prevents unwind-based RAII inside blocking closures from executing reliably. Cleanup is instead explicitly driven from the cancel arm of the job worker's `tokio::select!` loop, matching the pattern described in this ADR.

Orphan temporary files — those that were not cleaned up because substrate was killed (e.g., via SIGKILL on macOS where `PR_SET_PDEATHSIG` is unavailable) — are reaped at next substrate startup by the orphan reaper routine specified in [ADR-0055](0055-subprocess-orphan-reaper.md). The reaper identifies orphan subprocess stream files by the `.substrate-subprocess-stream-` prefix and removes them before accepting new tool calls.

The disk-space preflight defined in this ADR applies to subprocess stream capture: before creating the stdout and stderr temporary files, substrate checks available space in the `<capture_root>` filesystem and returns `SUBSTRATE_STORAGE_FULL` if the projected capture size exceeds available capacity. When the projected size is unknown (streaming capture without a known size bound), the preflight is skipped and ENOSPC is handled as a write error during capture.

Cross-references: [ADR-0052](0052-subprocess-execution-architecture.md) — subprocess execution architecture; [ADR-0054](0054-subprocess-stream-capture.md) — stream capture and aggregation; [ADR-0055](0055-subprocess-orphan-reaper.md) — orphan reaper.

### 2026-05-24 (revision 2) — TmpFile capture finalisation invariants

The [ADR-0054](0054-subprocess-stream-multiplex.md) amendment of the same date
closes the implementation of the TmpFile capture branch. The following invariants
extend the subprocess stream-capture amendment above and MUST be enforced by any
implementation of that branch.

**Mode 0600 on creation.** Every subprocess stream transit file MUST be created
with permissions mode `0600` (owner read/write only). This prevents subprocess
output from being readable by other users on multi-user hosts, where the shared
`tmp_root` directory may be world-accessible. The mode MUST be set at file
creation time, not applied as a subsequent `chmod`, to avoid a race window in
which a concurrent process could observe a file with broader permissions.

**Atomic rename precondition.** The atomic rename guarantee from step 5 of this
ADR (`rename(2)` is atomic on POSIX same-filesystem paths) holds for subprocess
stream files only because the transit path and the final path both reside under
`tmp_root`. This is guaranteed structurally: both the `.tmp.<uuid7>` transit name
and the final name omit the suffix and live in the same directory. Operators MUST
NOT configure `subprocess.tmp_root` to span a filesystem boundary with
`policy.roots` entries; validation at startup enforces that `tmp_root` canonicalises
to a path inside `policy.roots` (returning `SUBSTRATE_CONFIG_INVALID` on failure).

**ENOENT-safe cleanup.** The `cleanup_tmp_files` routine MUST silently ignore
`ENOENT` errors when removing transit files. The orphan reaper introduced by
[ADR-0055](0055-subprocess-orphan-reaper.md) may remove a transit file between the
terminal state write and the cleanup call (a legitimate race on SIGKILL-forced
shutdown). Treating `ENOENT` as a fatal error in cleanup would mask the real
terminal state and prevent the error response from reaching the client.

**No Drop-based cleanup.** Cleanup of transit files MUST NOT be implemented via a
`Drop` impl. Under `panic = "abort"` (per [ADR-0014](0014-build-system-and-toolchain.md)),
unwinding does not run `Drop` destructors; a panic in any task aborts the process
before `Drop` can fire. Cleanup MUST be driven explicitly from the cancel arm of
the job worker's `tokio::select!` loop, consistent with the pattern described in
this ADR.

Cross-references: [ADR-0054](0054-subprocess-stream-multiplex.md) — TmpFile capture
branch; [ADR-0014](0014-build-system-and-toolchain.md) — `panic = "abort"` cleanup
contract; [ADR-0017](0017-concurrency-limits.md) — `subprocess.tmp_root` configuration.
