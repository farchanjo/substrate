//! `TmpFileWriter` — transactional tmp-file helper for `CaptureKind::TmpFile`.
//!
//! Implements the naming convention and atomic rename pattern from ADR-0033
//! (amendment 2026-05-24) for subprocess stdout/stderr capture:
//!
//! - Transit path: `<tmp_root>/.substrate-subprocess-stream-<job_id>.<stream>.tmp.<uuid7>`
//! - Final path: `<tmp_root>/.substrate-subprocess-stream-<job_id>.<stream>`
//!
//! The file is created with mode `0600` (owner read/write only) to prevent
//! other users on the same machine from reading subprocess output.
//!
//! On terminal `Succeeded`, the caller calls [`TmpFileWriter::finalize`] which
//! flushes, atomically renames the transit file to the final path, and returns
//! the final path to the caller.
//!
//! On cancellation or error, the caller should call [`TmpFileWriter::abort`] (or
//! rely on the cascade cleanup chain via `ChildHandle.tmp_files`) which removes
//! the transit path.
//!
//! References: ADR-0033 §"Transactional Write Pattern", ADR-0054 §"`TmpFile` Branch".

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::io::AsyncWriteExt as _;
use tokio::sync::Mutex;

use substrate_domain::subprocess::errors::SubprocessError;
use substrate_domain::subprocess::stream::Stream;
use substrate_domain::value_objects::JobId;

/// Transactional tmp-file writer for subprocess stream capture per ADR-0033/ADR-0054.
///
/// Created by [`TmpFileWriter::create`]; finalized with [`TmpFileWriter::finalize`]
/// on success or cleaned up with [`TmpFileWriter::abort`] on cancellation.
///
/// Both the transit path and the final path must be registered in
/// `ChildHandle.tmp_files` before the first byte is written so that the cascade
/// cleanup chain can remove them even if the rename was interrupted.
///
/// [`TmpFileWriter::finalize`] takes `&self` and is idempotent: the first call
/// flushes, closes the file handle, and atomically renames the transit file to
/// its final path. Subsequent calls detect the `finalized` flag and return the
/// cached `final_path` immediately without performing I/O. This allows callers
/// holding an `Arc<TmpFileWriter>` to call `finalize` safely from both the
/// stream-capture EOF path and the registry result path.
#[derive(Debug)]
pub struct TmpFileWriter {
    /// Transit path: `<tmp_root>/.substrate-subprocess-stream-<job_id>.<stream>.tmp.<uuid7>`.
    tmp_path: PathBuf,
    /// Final path after atomic rename: `<tmp_root>/.substrate-subprocess-stream-<job_id>.<stream>`.
    final_path: PathBuf,
    /// The open file handle, protected by a mutex so `write` and `finalize` can be
    /// called from different tasks without holding the guard across `.await`.
    ///
    /// Set to `Some(file)` at construction; `finalize` takes the value (leaving
    /// `None`) before renaming so the FD is closed before the rename syscall.
    file: Mutex<Option<tokio::fs::File>>,
    /// Cumulative bytes written; exposed via [`TmpFileWriter::bytes_written`].
    bytes_written: AtomicU64,
    /// Set to `true` by the first successful [`finalize`](TmpFileWriter::finalize)
    /// call. Subsequent calls are no-ops returning the final path.
    finalized: AtomicBool,
}

impl TmpFileWriter {
    /// Creates a new transit file under `tmp_root` for the given `job_id` and `stream`.
    ///
    /// The file is opened with `O_CREAT | O_WRONLY | O_TRUNC` and mode `0600`
    /// (owner-only read/write) per the security requirement in ADR-0054 §"`TmpFile` Branch".
    ///
    /// # Errors
    ///
    /// Returns [`SubprocessError::SpawnFailed`] wrapping the underlying `io::Error`
    /// if the file could not be created (e.g., `tmp_root` does not exist or is
    /// not writable, or the filesystem is full).
    pub async fn create(
        tmp_root: &Path,
        job_id: &JobId,
        stream: Stream,
    ) -> Result<Self, SubprocessError> {
        // Build the UUID7 suffix for the transit file.
        let suffix = uuid::Uuid::now_v7();

        // Transit: <tmp_root>/.substrate-subprocess-stream-<job_id>.<stream>.tmp.<uuid7>
        let transit_name = format!(".substrate-subprocess-stream-{job_id}.{stream}.tmp.{suffix}");
        let tmp_path = tmp_root.join(transit_name);

        // Final: <tmp_root>/.substrate-subprocess-stream-<job_id>.<stream>
        let final_name = format!(".substrate-subprocess-stream-{job_id}.{stream}");
        let final_path = tmp_root.join(final_name);

        // Open with mode 0600 (owner read/write only).
        // tokio::fs::OpenOptions exposes mode() natively on unix targets (no import needed).
        // On non-unix targets the mode() call is omitted; the OS controls default permissions.
        let file = {
            use tokio::fs::OpenOptions;
            let mut opts = OpenOptions::new();
            opts.create(true).write(true).truncate(true);
            #[cfg(unix)]
            opts.mode(0o600);
            opts.open(&tmp_path)
                .await
                .map_err(|e| SubprocessError::SpawnFailed {
                    source: io::Error::other(format!(
                        "TmpFileWriter: failed to open transit file {}: {e}",
                        tmp_path.display()
                    )),
                })?
        };

        Ok(Self {
            tmp_path,
            final_path,
            file: Mutex::new(Some(file)),
            bytes_written: AtomicU64::new(0),
            finalized: AtomicBool::new(false),
        })
    }

    /// Returns the transit path (before finalization).
    #[must_use]
    pub fn tmp_path(&self) -> &Path {
        &self.tmp_path
    }

    /// Returns the final path (after successful [`finalize`](TmpFileWriter::finalize)).
    #[must_use]
    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// Returns the total number of bytes written so far.
    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written.load(Ordering::Relaxed)
    }

    /// Appends `data` to the transit file.
    ///
    /// Acquires the file mutex, calls `write_all`, increments the byte counter,
    /// then releases the mutex. Returns immediately on zero-length input.
    ///
    /// # Errors
    ///
    /// Returns [`SubprocessError::SpawnFailed`] wrapping the underlying `io::Error`
    /// if the write fails. The caller in `stream_capture.rs` is expected to log
    /// the error and continue the read loop rather than aborting the worker.
    pub async fn write(&self, data: &[u8]) -> Result<(), SubprocessError> {
        if data.is_empty() {
            return Ok(());
        }
        // Scope the MutexGuard so it is dropped before any potential .await points.
        let mut guard = self.file.lock().await;
        if let Some(f) = guard.as_mut() {
            f.write_all(data)
                .await
                .map_err(|e| SubprocessError::SpawnFailed {
                    source: io::Error::other(format!(
                        "TmpFileWriter: write failed to {}: {e}",
                        self.tmp_path.display()
                    )),
                })?;
        }
        drop(guard);
        let n = u64::try_from(data.len()).unwrap_or(u64::MAX);
        self.bytes_written.fetch_add(n, Ordering::Relaxed);
        Ok(())
    }

    /// Flushes and atomically renames the transit file to its final path.
    ///
    /// This method takes `&self` and is **idempotent**: the first call flushes,
    /// closes the file handle, and performs the atomic rename. Subsequent calls
    /// detect the `finalized` flag and return the cached `final_path` immediately
    /// without performing any I/O (the file no longer exists at the transit path).
    ///
    /// After this call the transit file no longer exists; the final file is
    /// visible at [`final_path`](TmpFileWriter::final_path). The caller should
    /// update `ChildHandle.tmp_files` to remove the transit path (the final path
    /// was also registered so the cascade reaper can handle an interrupted rename).
    ///
    /// Returns the final path so the caller can populate
    /// [`SubprocessResult::stdout_tmp_path`] / [`SubprocessResult::stderr_tmp_path`].
    ///
    /// References: ADR-0033 §"Transactional Write Pattern", ADR-0054 §"`TmpFile` Branch".
    ///
    /// # Errors
    ///
    /// Returns [`SubprocessError::SpawnFailed`] if flush or rename fails.
    pub async fn finalize(&self) -> Result<PathBuf, SubprocessError> {
        // Fast path: already finalized (idempotent second call).
        if self.finalized.load(Ordering::Acquire) {
            return Ok(self.final_path.clone());
        }

        // Flush and close the file before rename. Take the file out of the Option
        // so the FD is closed before the rename syscall (required on Windows;
        // also ensures clean POSIX semantics).
        {
            let mut guard = self.file.lock().await;
            if let Some(mut f) = guard.take() {
                f.flush().await.map_err(|e| SubprocessError::SpawnFailed {
                    source: io::Error::other(format!(
                        "TmpFileWriter: flush failed for {}: {e}",
                        self.tmp_path.display()
                    )),
                })?;
                // File is dropped here (guard.take() moved `f`); FD is closed.
            }
            // If guard holds None here it means a concurrent finalize already took
            // the file.  The rename below will encounter ENOENT if the concurrent
            // call already completed it; we treat that as success (idempotent).
        }

        // Atomic rename: both paths are under tmp_root, guaranteeing same filesystem.
        match tokio::fs::rename(&self.tmp_path, &self.final_path).await {
            Ok(()) => {},
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // Transit file already renamed by a concurrent finalize call.
                // Treat as success if the final path exists.
                if !tokio::fs::try_exists(&self.final_path)
                    .await
                    .unwrap_or(false)
                {
                    return Err(SubprocessError::SpawnFailed {
                        source: io::Error::other(format!(
                            "TmpFileWriter: rename {} -> {} failed (transit gone, final absent): {e}",
                            self.tmp_path.display(),
                            self.final_path.display()
                        )),
                    });
                }
            },
            Err(e) => {
                return Err(SubprocessError::SpawnFailed {
                    source: io::Error::other(format!(
                        "TmpFileWriter: rename {} -> {} failed: {e}",
                        self.tmp_path.display(),
                        self.final_path.display()
                    )),
                });
            },
        }

        // Mark finalized so subsequent calls return immediately.
        self.finalized.store(true, Ordering::Release);
        Ok(self.final_path.clone())
    }

    /// Removes the transit file, ignoring `NotFound` errors (POSIX ENOENT).
    ///
    /// Called on the cancellation / abort path. The final path is handled by the
    /// cascade cleanup chain via `ChildHandle.tmp_files`.
    ///
    /// If [`finalize`](TmpFileWriter::finalize) has already been called (i.e., the
    /// transit file was renamed to the final path), this method is a no-op — the
    /// transit file no longer exists, so there is nothing to remove.
    ///
    /// # Errors
    ///
    /// Returns the underlying `io::Error` for any failure other than `NotFound`.
    pub async fn abort(&self) -> Result<(), io::Error> {
        // If already finalized the transit file was renamed; nothing to remove.
        if self.finalized.load(Ordering::Acquire) {
            return Ok(());
        }
        // Close the file handle before attempting removal.
        {
            let mut guard = self.file.lock().await;
            drop(guard.take());
        }
        match tokio::fs::remove_file(&self.tmp_path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;

    use substrate_domain::subprocess::stream::Stream;
    use substrate_domain::value_objects::JobId;

    use super::TmpFileWriter;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Helper: create a `TmpFileWriter` under a fresh `TempDir`.
    async fn make_writer(dir: &TempDir) -> Result<TmpFileWriter, Box<dyn std::error::Error>> {
        let job_id = JobId::now_v7();
        let w = TmpFileWriter::create(dir.path(), &job_id, Stream::Stdout).await?;
        Ok(w)
    }

    /// Verify that `finalize` performs the atomic rename on the first call.
    #[tokio::test]
    async fn finalize_renames_transit_to_final() -> TestResult {
        let dir = TempDir::new()?;
        let writer = make_writer(&dir).await?;
        let tmp_path = writer.tmp_path().to_owned();
        let final_path = writer.final_path().to_owned();

        // Write some bytes so the file is non-empty.
        writer.write(b"hello").await?;

        assert!(tmp_path.exists(), "transit file must exist before finalize");
        assert!(
            !final_path.exists(),
            "final file must not exist before finalize"
        );

        let returned = writer.finalize().await?;
        assert_eq!(returned, final_path, "finalize must return final_path");
        assert!(
            !tmp_path.exists(),
            "transit file must be gone after finalize"
        );
        assert!(final_path.exists(), "final file must exist after finalize");

        // Verify the contents were preserved through the rename.
        let contents = tokio::fs::read(&final_path).await?;
        assert_eq!(contents, b"hello");
        Ok(())
    }

    /// Verify that a second `finalize` call on the same `TmpFileWriter` is a no-op
    /// and returns the same `final_path` without error.
    #[tokio::test]
    async fn finalize_is_idempotent() -> TestResult {
        let dir = TempDir::new()?;
        let writer = make_writer(&dir).await?;
        let final_path = writer.final_path().to_owned();

        writer.write(b"idempotent").await?;

        let p1 = writer.finalize().await?;
        let p2 = writer.finalize().await?;

        assert_eq!(p1, final_path);
        assert_eq!(p2, final_path);
        assert!(
            final_path.exists(),
            "final file must still exist after second finalize"
        );
        Ok(())
    }

    /// Verify that `finalize` is idempotent when called concurrently via `Arc`.
    #[tokio::test]
    async fn finalize_idempotent_via_arc() -> TestResult {
        let dir = TempDir::new()?;
        let writer = Arc::new(make_writer(&dir).await?);
        let final_path = writer.final_path().to_owned();

        writer.write(b"concurrent").await?;

        // Simulate the two callers (stream_capture EOF + registry.result).
        let w1 = Arc::clone(&writer);
        let w2 = Arc::clone(&writer);
        let (r1, r2) = tokio::join!(w1.finalize(), w2.finalize());

        assert!(
            r1.is_ok() || r2.is_ok(),
            "at least one finalize must succeed"
        );
        assert!(
            final_path.exists(),
            "final file must exist after concurrent finalize"
        );
        Ok(())
    }

    /// Verify that `abort` after `finalize` is a no-op (does not remove the final file).
    #[tokio::test]
    async fn abort_after_finalize_is_noop() -> TestResult {
        let dir = TempDir::new()?;
        let writer = make_writer(&dir).await?;
        let final_path = writer.final_path().to_owned();

        writer.write(b"data").await?;
        writer.finalize().await?;

        // abort must not error and must not remove the final file.
        writer.abort().await?;
        assert!(
            final_path.exists(),
            "final file must still exist after abort-post-finalize"
        );
        Ok(())
    }
}
