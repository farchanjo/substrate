---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0031 — File Handle and Resource Lifecycle

## Context and Problem Statement

`substrate` opens file handles, spawns child processes, and holds in-memory buffers on behalf of tool calls. Each resource has a finite OS limit (file descriptor cap, process table slots, virtual memory). A misbehaving or timed-out tool call must not leak these resources. Additionally, large files passed through buffers (e.g., read-file, hash-file) must not cause OOM on an agent that requests an unexpectedly large file.

## Decision Drivers

- The OS default `ulimit -n` on macOS and Linux is 256–1024 per process; a file descriptor leak will crash the server.
- In-memory buffers for file content must be bounded to prevent OOM; the default 8 MiB covers typical config/log files while the 64 MiB ceiling prevents runaway reads.
- Child processes created by tools (e.g., process inspection via nix) must be cleaned up even if the parent task is cancelled.
- mpsc channels used for internal streaming must be bounded to prevent unbounded queue growth.
- RAII is the canonical Rust resource-management pattern; Drop must close all handles.

## Considered Options

1. tokio::fs::File via RAII + bounded buffers + FD check at startup + kill_on_drop — accepted.
2. Explicit `close()` calls at the end of each tool function — rejected; cancellation via `CancellationToken` can bypass explicit close calls if the future is dropped before reaching them.
3. Reference-counted handle pools — rejected; adds complexity without benefit for short-lived per-request files.
4. Unlimited in-memory buffers with streaming to the caller — rejected; rmcp's transport-io buffers the full response in memory before writing; streaming does not reduce peak allocation.
5. `sendfile` / zero-copy for large files — deferred; the current MCP response format requires base64 or inline text; zero-copy is not applicable until a binary transfer protocol is designed.

## Decision Outcome

Chosen option: "tokio::fs::File RAII + 8 MiB default / 64 MiB ceiling + FD startup check + kill_on_drop + bounded channels", because RAII guarantees cleanup on both happy-path completion and cancellation-induced future drop, and bounded buffers protect against OOM without requiring streaming infrastructure.

### File Handle Lifecycle (RAII)

`tokio::fs::File` implements `Drop` which closes the underlying OS file descriptor. All file opens in tool implementations use `tokio::fs::File::open` or `File::create` and are stored in local variables, never in `Arc` or `Mutex`. This guarantees that when the containing future is dropped (either by completion or cancellation), the file descriptor is closed.

Pattern enforced by code review:

```rust
// Correct: RAII, closed on drop
let file = tokio::fs::File::open(&path).await?;
let content = read_bounded(&file, ctx.config.buffer_limit).await?;
// file drops here

// Forbidden: storing in Arc/Mutex across .await without clear Drop path
// let shared = Arc::new(Mutex::new(file));
```

Files are never stored in shared state (`Arc<Mutex<File>>`). If multiple tool stages need access to the same file, it is re-opened for each stage.

### In-Memory Buffer Limits

| Limit | Value | Configurable |
|-------|-------|--------------|
| Default per-tool in-memory buffer | 8 MiB | Yes, via `[tools.<name>] max_bytes` |
| Hard ceiling (process-wide) | 64 MiB | No; compile-time constant |
| Chunk size for streaming reads | 64 KiB | No |

When a read operation would exceed the tool's configured limit, the tool returns `ToolError::OutputTooLarge` with the actual byte count and the limit. The hard 64 MiB ceiling is enforced in the `read_bounded` utility function and cannot be overridden by config:

```rust
const BUFFER_HARD_CEILING: usize = 64 * 1024 * 1024; // 64 MiB

pub async fn read_bounded(file: &File, limit: usize) -> Result<Bytes> {
    let limit = limit.min(BUFFER_HARD_CEILING);
    // ... read up to limit bytes, error if more available
}
```

### File Descriptor Cap Check at Startup

At server startup, before accepting any tool calls, the server checks the current process FD limit:

```rust
let (soft, hard) = nix::sys::resource::getrlimit(Resource::RLIMIT_NOFILE)?;
if soft < MIN_REQUIRED_FDS {
    warn!("FD limit {} is below recommended {}; tool calls may fail", soft, MIN_REQUIRED_FDS);
}
```

`MIN_REQUIRED_FDS` is 512. If the soft limit is below this, a warning is logged to stderr. If the hard limit permits it, the server attempts to raise the soft limit to `min(hard, 4096)` using `setrlimit`. Failure to raise is non-fatal but logged.

### mpsc Channel Cap

All internal `tokio::sync::mpsc` channels used for streaming results between tasks are created with a capacity of 256:

```rust
let (tx, rx) = mpsc::channel(256);
```

This applies to:
- Grep result streaming (one item per matching line batch).
- Directory walk streaming (one item per file entry batch).
- Archive entry streaming.

256 is chosen as a balance between memory usage (each item is a small struct, ~256 bytes → 64 KiB max queue) and back-pressure effectiveness. If a consumer is slow, senders block on the channel, which propagates back-pressure to the spawn_blocking thread via the async `send().await` call.

### Child Process Lifecycle

`tokio::process::Command` is used for tools that need to spawn OS processes. All `Child` handles are created with `kill_on_drop(true)`:

```rust
let mut child = Command::new("some-tool")
    .kill_on_drop(true)
    .spawn()?;
```

This ensures that if the tool future is dropped (due to timeout or cancellation), the child process receives SIGKILL and is reaped. The `kill_on_drop` flag is verified in code review for every `Command` construction.

For processes that must be waited on to collect exit status, a `tokio::select!` races `child.wait()` against the `CancellationToken`:

```rust
tokio::select! {
    status = child.wait() => status?,
    _ = ctx.token.cancelled() => {
        child.kill().await.ok();
        return Err(ToolError::Cancelled);
    }
}
```

### Explicit Flush Requirement

`tokio::fs::File::drop` closes the underlying file descriptor but does NOT flush buffered data. Any writable `tokio::fs::File` or `tokio::io::BufWriter<File>` MUST call `.flush().await` before going out of scope. `BufWriter` additionally requires `.shutdown().await` to flush the inner writer and close the underlying file:

```rust
// Correct: explicit flush + shutdown for BufWriter.
let file = tokio::fs::File::create(&path).await?;
let mut writer = tokio::io::BufWriter::new(file);
writer.write_all(&data).await?;
writer.flush().await?;
writer.shutdown().await?;
// writer (and inner File) drop here; FD is closed.

// Forbidden: relying on Drop to flush.
// let mut writer = tokio::io::BufWriter::new(file);
// writer.write_all(&data).await?;
// // writer drops here -- buffered data is silently discarded.
```

A Clippy lint rule is documented in the workspace `[lints]` section requiring `flush` before `drop` for writable files. Code review enforces this for any `BufWriter` or direct `File::write_all` usage.

### NFS and FUSE Close Hazard

`tokio::fs::File::drop` calls `close(2)` synchronously on the tokio thread that drops the `File`. On NFS or FUSE mounts, `close(2)` can block for seconds while the network flushes or the FUSE daemon processes the close request. This stalls the tokio executor thread for the duration, degrading all other concurrent tool calls.

Workaround for paths that may reside on network filesystems: convert the `tokio::fs::File` to a `std::fs::File` via `.into_std().await` and perform the final close off the executor in a `spawn_blocking` call:

```rust
// For paths that may be on NFS or FUSE:
let std_file = tokio_file.into_std().await;
tokio::task::spawn_blocking(move || drop(std_file)).await?;
```

This pattern is required when the tool's configured root paths include directories known or suspected to reside on network filesystems. For purely local paths (e.g., `/tmp`, `/var`), the standard `drop` is acceptable.

### Temp File Lifecycle

Any tool that writes to disk (archive create, `fs.write`, `fs.copy`, any multi-step transform) MUST follow the atomic write pattern using a temporary file:

1. Open `<target_path>.tmp.<uuid>` for writing (use `tempfile::NamedTempFile::new_in(target_dir)` to ensure the temp file is on the same filesystem as the target, enabling atomic rename).
2. Write all data to the temp file. Flush and shutdown as required by the Explicit Flush Requirement above.
3. On success: rename the temp file to the final path atomically via `tokio::fs::rename`.
4. On cancellation: delete the temp file.

The cancellation cleanup MUST be implemented as a `tokio::select!` arm, not a `Drop` impl, because Rust has no async `Drop` and a synchronous `Drop` cannot await the `remove_file` future. See [ADR-0037](0037-async-cancellation-patterns.md) for the async-Drop limitation rule.

```rust
let tmp = tempfile::NamedTempFile::new_in(target_path.parent().unwrap())?;
let tmp_path = tmp.path().to_owned();

tokio::select! {
    biased;
    result = write_and_flush(&tmp, &data) => {
        result?;
        tokio::fs::rename(&tmp_path, &target_path).await?;
    }
    _ = ctx.token.cancelled() => {
        tokio::fs::remove_file(&tmp_path).await.ok();
        return Err(ToolError::Cancelled);
    }
}
```

See also [ADR-0033](0033-transactional-write-pattern.md) for the full transactional write contract that builds on this pattern.

### Consequences

#### Positive

- File descriptors are always closed, even under cancellation or panic (tokio catches panics in tasks and drops the future).
- Buffer limits prevent a single `read_file` call on a 10 GiB log file from OOMing the server.
- `kill_on_drop` eliminates zombie processes without requiring explicit cleanup code in every tool.
- Bounded channels prevent unbounded memory growth during slow consumers.

#### Negative

- The 64 MiB hard ceiling means that tools cannot return file contents larger than 64 MiB inline. Tools operating on large files must either stream (not currently supported in MCP transport-io) or return a truncated result with a `truncated: true` flag.
- Re-opening files for each stage incurs additional `open(2)` syscalls; for NFS-mounted paths this may add latency.
- The startup FD check using `nix::sys::resource::getrlimit` is POSIX-only; on Windows (if ever targeted), an alternative implementation is required.

## Validation

- Unit test: open a file larger than 8 MiB; assert `read_bounded` returns `ToolError::OutputTooLarge`.
- Unit test: drop a `Child` with `kill_on_drop(true)` and confirm the child PID is no longer in `/proc` (Linux) or `ps` output (macOS) within 1 second.
- Integration test: cancel a running tool mid-read; assert that no file descriptors from that tool call remain open (check via `/proc/self/fd` on Linux).
- Startup test: run the server with a soft FD limit of 64; assert the warning is logged and the server attempts `setrlimit`.
- Code review checklist: every `Command` construction includes `.kill_on_drop(true)`; every `mpsc::channel` call specifies capacity 256 or less; no unbounded `mpsc::unbounded_channel` usage.

## Cross-References

- ADR-0006: CancellationToken and timeout; cancel propagation is what triggers RAII cleanup paths.
- ADR-0016: tool error taxonomy; `ToolError::OutputTooLarge` and `ToolError::Cancelled` definitions.
- [ADR-0032](0032-signal-safety.md): signal safety; SIGTERM triggers root `CancellationToken` cancellation which drives cleanup select! arms.
- [ADR-0033](0033-transactional-write-pattern.md): transactional write pattern; full atomic write and rollback contract.
- [ADR-0037](0037-async-cancellation-patterns.md): async-Drop limitation; cleanup must run in select! arms, not Drop impls; biased-select rule.
