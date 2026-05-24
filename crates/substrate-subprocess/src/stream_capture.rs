//! Reader tasks for the stdout/stderr stream multiplex per ADR-0054.
//!
//! Two `tokio` tasks are spawned per subprocess job:
//! - One reads from `ChildStdout`, emitting [`StreamChunk`] values.
//! - One reads from `ChildStderr`, emitting [`StreamChunk`] values.
//!
//! Each reader:
//! - Fills a 4 KiB buffer from the OS pipe.
//! - On buffer-full OR 100 ms flush timer: calls `mpsc::Sender::try_send`.
//! - On `try_send` `Err::Full`: drops the chunk, increments `stream_chunks_dropped`,
//!   emits a structured audit event `SUBSTRATE_STREAM_CHUNK_DROPPED`.
//! - Writes received bytes into the ring buffer regardless of mpsc success.
//! - Observes `CancellationToken` via `tokio::select! biased` (cancel as first arm).
//!
//! References: ADR-0054 §"Tokio Task and Channel Architecture",
//! ADR-0054 §"Flush Trigger", ADR-0054 §"Backpressure".

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use substrate_domain::subprocess::stream::Stream;
use substrate_domain::subprocess::{StreamChunk, SubprocessError};

use crate::spawn::{CHUNK_CAPACITY, ChildHandle, MPSC_CAPACITY};

/// Flush interval for the time-based chunk flush trigger per ADR-0054.
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Spawns the stdout and stderr reader tasks for a running child.
///
/// Extracts the `ChildStdout` and `ChildStderr` from `child` (takes them from
/// the piped stdio), then spawns two independent tokio tasks. Both tasks send
/// [`StreamChunk`] values to the shared `sender`. When all chunks are emitted
/// (child exits), the tasks close their send-side mpsc channel ends.
///
/// # Errors
///
/// Returns [`SubprocessError::SpawnFailed`] if the child's stdout or stderr
/// were not piped (which should not happen given `Stdio::piped()` in `spawn.rs`).
///
/// References: ADR-0054.
pub fn spawn_stream_captures(
    child: &mut tokio::process::Child,
    handle: &Arc<ChildHandle>,
    sender: mpsc::Sender<StreamChunk>,
) -> Result<(), SubprocessError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SubprocessError::SpawnFailed {
            source: std::io::Error::other("child stdout was not piped"),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| SubprocessError::SpawnFailed {
            source: std::io::Error::other("child stderr was not piped"),
        })?;

    // Spawn stdout reader.
    let stdout_handle = Arc::clone(handle);
    let stdout_sender = sender.clone();
    tokio::spawn(async move {
        read_stream(stdout, Stream::Stdout, stdout_handle, stdout_sender).await;
    });

    // Spawn stderr reader.
    let stderr_handle = Arc::clone(handle);
    tokio::spawn(async move {
        read_stream(stderr, Stream::Stderr, stderr_handle, sender).await;
    });

    Ok(())
}

/// Creates a bounded mpsc channel for stream chunks per ADR-0054.
///
/// Channel capacity is `MPSC_CAPACITY` (64) per ADR-0054 §"Tokio Task and
/// Channel Architecture". The sender is given to `spawn_stream_captures`;
/// the receiver is used by the dispatcher task in the registry.
#[must_use]
pub fn make_stream_channel() -> (mpsc::Sender<StreamChunk>, mpsc::Receiver<StreamChunk>) {
    mpsc::channel(MPSC_CAPACITY)
}

/// Single-stream reader task.
///
/// Reads from `reader` into a 4 KiB buffer. Flushes on buffer-full or 100 ms
/// interval. Writes all bytes into the ring buffer. For `CaptureKind::TmpFile`,
/// also persists bytes to the transit file via `TmpFileWriter`. Sends to `sender`
/// via `try_send`; drops on `Err::Full` and increments the dropped counter.
///
/// Processing order per ADR-0054 §"`TmpFile` Branch":
/// 1. Ring buffer (in-memory aggregate safety net).
/// 2. Tmp file write (if `capture_kind == TmpFile`).
/// 3. mpsc `try_send` (live notifications; may drop on backpressure).
///
/// On `TmpFileWriter::write` error: logs via `tracing::error!` and continues the
/// read loop. Partial persistence is preferable to aborting the worker entirely.
///
/// Exits when `handle.cancel` fires or when EOF is reached on `reader`.
async fn read_stream<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    stream: Stream,
    handle: Arc<ChildHandle>,
    sender: mpsc::Sender<StreamChunk>,
) {
    let mut buf = vec![0u8; CHUNK_CAPACITY];
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    // The first tick fires immediately; skip it so we don't flush a zero-byte chunk.
    interval.tick().await;
    let mut accumulated: Vec<u8> = Vec::with_capacity(CHUNK_CAPACITY);
    let mut byte_offset: u64 = 0;

    // Resolve the TmpFileWriter for this stream once, outside the hot loop.
    let tmp_writer = match stream {
        Stream::Stdout => handle.stdout_tmp_writer.as_ref().map(Arc::clone),
        Stream::Stderr => handle.stderr_tmp_writer.as_ref().map(Arc::clone),
    };

    loop {
        tokio::select! {
            biased;

            // Cancellation arm (biased: checked first on each poll).
            () = handle.cancel.cancelled() => {
                // Flush any accumulated bytes, then exit.
                if !accumulated.is_empty() {
                    flush_chunk(
                        &accumulated,
                        stream,
                        &handle,
                        &sender,
                        byte_offset,
                    );
                }
                break;
            },

            // Read arm.
            n = reader.read(&mut buf) => {
                match n {
                    Ok(0) => {
                        // EOF: flush accumulated bytes, then finalize the TmpFileWriter.
                        if !accumulated.is_empty() {
                            flush_chunk(
                                &accumulated,
                                stream,
                                &handle,
                                &sender,
                                byte_offset,
                            );
                            // byte_offset update omitted: loop exits immediately after.
                        }
                        finalize_on_eof(tmp_writer.as_ref(), stream, &handle).await;
                        break;
                    },
                    Ok(n) => {
                        let data = &buf[..n];
                        // 1. Write to ring buffer unconditionally (even if mpsc drops).
                        write_ring(&handle, stream, data).await;
                        // 2. Persist to tmp file if capture_kind == TmpFile.
                        //    On write error: log and continue — partial persistence
                        //    beats aborting the capture worker entirely.
                        if let Some(writer) = &tmp_writer
                            && let Err(e) = writer.write(data).await
                        {
                            error!(
                                target: "substrate_audit",
                                event = "SUBSTRATE_SUBPROCESS_TMP_WRITE_ERROR",
                                job_id = %handle.job_id,
                                stream = %stream,
                                bytes_attempted = data.len(),
                                error = %e,
                                "TmpFileWriter::write failed; continuing capture (partial persistence)"
                            );
                        }
                        // 3. Accumulate for mpsc flush.
                        accumulated.extend_from_slice(data);
                        byte_offset += u64::try_from(n).unwrap_or(u64::MAX);

                        // Flush when 4 KiB threshold hit.
                        if accumulated.len() >= CHUNK_CAPACITY {
                            flush_chunk(
                                &accumulated,
                                stream,
                                &handle,
                                &sender,
                                byte_offset.saturating_sub(
                                    u64::try_from(accumulated.len()).unwrap_or(u64::MAX),
                                ),
                            );
                            accumulated.clear();
                        }
                    },
                    Err(e) => {
                        warn!(
                            job_id = %handle.job_id,
                            stream = %stream,
                            error = %e,
                            "stream reader I/O error; exiting reader task"
                        );
                        break;
                    },
                }
            },

            // Time-based flush arm: fires every 100 ms per ADR-0054.
            _ = interval.tick() => {
                if !accumulated.is_empty() {
                    flush_chunk(
                        &accumulated,
                        stream,
                        &handle,
                        &sender,
                        byte_offset.saturating_sub(
                            u64::try_from(accumulated.len()).unwrap_or(u64::MAX),
                        ),
                    );
                    accumulated.clear();
                }
            },
        }
    }
}

/// Finalizes the [`TmpFileWriter`](crate::tmp_file::TmpFileWriter) for the given stream on EOF.
///
/// Called from the `Ok(0)` (EOF) arm of `read_stream`. This is the **primary**
/// finalize call: it triggers the atomic rename from the transit path to the
/// final path per ADR-0033. `registry.result()` may also call `finalize()`, but
/// `TmpFileWriter::finalize` is idempotent so the second call is a no-op.
///
/// Best-effort: errors are logged via `tracing` but not propagated — the reader
/// task result is discarded by the spawn site, and partial persistence is
/// preferable to aborting the capture worker.
async fn finalize_on_eof(
    tmp_writer: Option<&Arc<crate::tmp_file::TmpFileWriter>>,
    stream: Stream,
    handle: &ChildHandle,
) {
    let Some(writer) = tmp_writer else { return };
    match writer.finalize().await {
        Ok(final_path) => {
            info!(
                target: "substrate_audit",
                event = "SUBSTRATE_SUBPROCESS_TMP_FINALISED",
                job_id = %handle.job_id,
                stream = %stream,
                final_path = %final_path.display(),
                "TmpFileWriter finalised on stream EOF"
            );
            handle.unregister_tmp_path(writer.tmp_path()).await;
        },
        Err(e) => {
            error!(
                target: "substrate_audit",
                event = "SUBSTRATE_SUBPROCESS_TMP_FINALISE_FAILED",
                job_id = %handle.job_id,
                stream = %stream,
                error = %e,
                "TmpFileWriter::finalize failed on stream EOF; transit file may remain"
            );
        },
    }
}

/// Writes `data` into the appropriate ring buffer on `handle`.
async fn write_ring(handle: &ChildHandle, stream: Stream, data: &[u8]) {
    let n = u64::try_from(data.len()).unwrap_or(u64::MAX);
    match stream {
        Stream::Stdout => {
            handle.stdout_ring.lock().await.push(data);
            handle.stdout_bytes_total.fetch_add(n, Ordering::Relaxed);
        },
        Stream::Stderr => {
            handle.stderr_ring.lock().await.push(data);
            handle.stderr_bytes_total.fetch_add(n, Ordering::Relaxed);
        },
    }
}

/// Constructs a [`StreamChunk`] and calls `try_send` on `sender`.
///
/// On `Err::Full`:
/// - Increments `handle.stream_chunks_dropped`.
/// - Emits structured audit event `SUBSTRATE_STREAM_CHUNK_DROPPED`.
/// - Does NOT block. The ring buffer already received the bytes via `write_ring`.
fn flush_chunk(
    data: &[u8],
    stream: Stream,
    handle: &ChildHandle,
    sender: &mpsc::Sender<StreamChunk>,
    byte_offset: u64,
) {
    let seq = match stream {
        Stream::Stdout => handle.stdout_seq.fetch_add(1, Ordering::Relaxed),
        Stream::Stderr => handle.stderr_seq.fetch_add(1, Ordering::Relaxed),
    };

    let chunk = StreamChunk {
        job_id: handle.job_id.clone(),
        stream,
        seq,
        chunk: data.to_vec(),
        byte_offset,
        timestamp: time::OffsetDateTime::now_utc(),
    };

    if let Err(mpsc::error::TrySendError::Full(_)) = sender.try_send(chunk) {
        // Backpressure: drop the chunk from the mpsc but keep ring buffer bytes.
        // Err::Closed is silently ignored (job cancelled, no further sends needed).
        handle.stream_chunks_dropped.fetch_add(1, Ordering::Relaxed);
        // ADR-0054 audit event.
        warn!(
            target: "substrate_audit",
            event = "SUBSTRATE_STREAM_CHUNK_DROPPED",
            job_id = %handle.job_id,
            stream = %stream,
            dropped_bytes = data.len(),
            seq = seq,
        );
    }
}

/// Re-export for callers that need the channel mpsc types.
pub use mpsc::{Receiver as StreamReceiver, Sender as StreamSender};

/// Type alias for the stream mpsc channel item.
pub use substrate_domain::subprocess::stream::StreamChunk as Chunk;
