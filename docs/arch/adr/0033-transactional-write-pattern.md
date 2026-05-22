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

## Cross-references

- [ADR-0004](0004-security-model.md) — Security model (path jail applied in step 1)
- [ADR-0016](0016-resource-limits.md) — Resource limits (size limits enforced before preflight)
- [ADR-0034](0034-kernel-induced-error-codes.md) — Kernel-induced error codes (`SUBSTRATE_STORAGE_FULL`)
- [ADR-0037](0037-async-cancellation-patterns.md) — Cancellation patterns (CancellationToken propagation)
