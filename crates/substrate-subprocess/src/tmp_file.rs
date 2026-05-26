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
//! ## Log rotation (ADR-0056)
//!
//! When a subprocess runs for hours (e.g., a dev server), its captured output can
//! grow unbounded. The optional [`LogRotationPolicy`] enables size-based rotation:
//! once the current transit file reaches `max_bytes_per_file`, [`TmpFileWriter::rotate_if_needed`]
//! atomically shifts older logs (`<base>.log.1` → `<base>.log.2`, …), renames the
//! current file to `<base>.log.1`, and opens a fresh `<base>.log` for continued writes.
//! Files beyond `keep_files` are unlinked. The rotation is purely best-effort from the
//! capture loop's perspective; an error is logged but never aborts the stream reader.
//!
//! References: ADR-0033 §"Transactional Write Pattern", ADR-0054 §"`TmpFile` Branch",
//! ADR-0056 §"Log Rotation".

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::io::AsyncWriteExt as _;
use tokio::sync::Mutex;
use tracing::info;

use substrate_domain::subprocess::errors::SubprocessError;
use substrate_domain::subprocess::stream::Stream;
use substrate_domain::value_objects::JobId;

/// Size-based log rotation policy per ADR-0056.
///
/// Attached to a [`TmpFileWriter`] via [`TmpFileWriter::with_rotation`].  When the
/// cumulative bytes written to the current transit file exceeds `max_bytes_per_file`,
/// the next call to [`TmpFileWriter::rotate_if_needed`] will atomically shift older
/// numbered files and open a fresh log.
///
/// `keep_files` controls how many numbered archives are retained (`<base>.log.1` …
/// `<base>.log.<keep_files>`).  Files beyond `keep_files` are unlinked. The minimum
/// useful value is `1`.
#[derive(Debug, Clone)]
pub struct LogRotationPolicy {
    /// Byte threshold that triggers a rotation.
    pub max_bytes_per_file: u64,
    /// Number of numbered archive files to keep (`.log.1` … `.log.<keep_files>`).
    pub keep_files: u8,
}

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
    /// Optional size-based rotation policy per ADR-0056.  `None` = no rotation.
    rotation: Option<LogRotationPolicy>,
}

/// Returns `<base_path>.<n>` — the path for the Nth numbered rotation archive.
///
/// For example, if `base` is `/tmp/.substrate-subprocess-stream-abc.stdout`, then
/// `numbered(base, 1)` returns `/tmp/.substrate-subprocess-stream-abc.stdout.1`.
fn numbered(base: &Path, n: u32) -> PathBuf {
    let mut s = base.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
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
            rotation: None,
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

    /// Attaches a size-based log-rotation policy per ADR-0056.
    ///
    /// Builder-style; consumes and returns `self` so it can be chained after [`create`](Self::create).
    ///
    /// # Parameters
    ///
    /// - `max_bytes_per_file` — byte threshold that triggers rotation.
    /// - `keep_files` — number of numbered archives to retain (`.log.1` … `.log.<keep_files>`).
    ///   Values of `0` are treated as `1` (at least one archive is kept).
    #[must_use]
    pub fn with_rotation(mut self, max_bytes_per_file: u64, keep_files: u8) -> Self {
        self.rotation = Some(LogRotationPolicy {
            max_bytes_per_file,
            keep_files: keep_files.max(1),
        });
        self
    }

    /// Atomically rotates the transit file if the byte threshold from the attached
    /// [`LogRotationPolicy`] is exceeded.
    ///
    /// Returns `Ok(true)` when a rotation was performed, `Ok(false)` when no policy
    /// is set or the threshold has not been reached.  On error the current transit
    /// file is left intact (best-effort) and the error is returned so the caller can
    /// log it and continue.
    ///
    /// ## Rotation algorithm (ADR-0056)
    ///
    /// Let `base` = `final_path` (e.g., `.substrate-subprocess-stream-<id>.stdout`).
    ///
    /// 1. Lock the file mutex and flush + close the current FD.
    /// 2. Shift numbered archives in reverse:
    ///    for N = `keep_files−1` … 1: rename `<base>.log.N` → `<base>.log.N+1` (ok() — missing is fine).
    /// 3. Unlink `<base>.log.<keep_files+1>` if present (overflow from the shift).
    /// 4. Rename current transit file → `<base>.log.1`.
    /// 5. Reopen a fresh transit file at the original `tmp_path`.
    /// 6. Reset `bytes_written` to 0.
    ///
    /// All individual renames are atomic filesystem operations.  A crash between
    /// steps leaves at most one stale numbered file; no data is lost because the
    /// previous content is preserved in the renamed archive.
    ///
    /// # Errors
    ///
    /// Returns `io::Error` if flush, a critical rename, or the reopen fails.
    pub async fn rotate_if_needed(&self) -> io::Result<bool> {
        let Some(policy) = self.rotation.as_ref() else {
            return Ok(false);
        };
        if self.bytes_written.load(Ordering::Relaxed) < policy.max_bytes_per_file {
            return Ok(false);
        }

        let keep = policy.keep_files;

        // ── 1. Flush + close the current FD ─────────────────────────────────
        {
            let mut guard = self.file.lock().await;
            if let Some(mut f) = guard.take() {
                f.flush().await.map_err(|e| {
                    io::Error::other(format!(
                        "TmpFileWriter::rotate_if_needed: flush failed for {}: {e}",
                        self.tmp_path.display()
                    ))
                })?;
                // `f` is dropped here; FD closed.
            }
            // guard still holds None — we reopen below.

            // ── 2. Shift numbered archives in reverse order ──────────────────
            // Base path for numbered archives is the *final* path (human-visible name).
            let base = &self.final_path;

            // Optionally unlink the oldest file that would be pushed beyond `keep_files`.
            let overflow_n = u32::from(keep) + 1;
            let overflow_path = numbered(base, overflow_n);
            if overflow_path.exists() {
                tokio::fs::remove_file(&overflow_path).await.map_err(|e| {
                    io::Error::other(format!(
                        "TmpFileWriter::rotate_if_needed: unlink overflow {}: {e}",
                        overflow_path.display()
                    ))
                })?;
            }

            // Shift: .log.(keep-1) → .log.keep  …  .log.1 → .log.2
            // Only range keep-1 down to 1 needs shifting (1-indexed archives).
            if keep >= 2 {
                let mut n = u32::from(keep) - 1;
                while n >= 1 {
                    let from = numbered(base, n);
                    let to = numbered(base, n + 1);
                    // ok() — a missing source is fine; that slot simply hasn't been created yet.
                    tokio::fs::rename(&from, &to).await.ok();
                    n -= 1;
                }
            }

            // ── 3. Rename current transit file → <base>.log.1 ───────────────
            let archive_1 = numbered(base, 1);
            tokio::fs::rename(&self.tmp_path, &archive_1)
                .await
                .map_err(|e| {
                    io::Error::other(format!(
                        "TmpFileWriter::rotate_if_needed: rename {} -> {}: {e}",
                        self.tmp_path.display(),
                        archive_1.display()
                    ))
                })?;

            // ── 4. Reopen a fresh transit file at `tmp_path` ────────────────
            let new_file = {
                use tokio::fs::OpenOptions;
                let mut opts = OpenOptions::new();
                opts.create(true).write(true).truncate(true);
                #[cfg(unix)]
                opts.mode(0o600);
                opts.open(&self.tmp_path).await.map_err(|e| {
                    io::Error::other(format!(
                        "TmpFileWriter::rotate_if_needed: reopen {} failed: {e}",
                        self.tmp_path.display()
                    ))
                })?
            };
            *guard = Some(new_file);
        }

        // ── 5. Reset byte counter ────────────────────────────────────────────
        self.bytes_written.store(0, Ordering::Relaxed);

        info!(
            target: "substrate_audit",
            event = "SUBSTRATE_SUBPROCESS_TMP_ROTATED",
            transit_path = %self.tmp_path.display(),
            final_path = %self.final_path.display(),
            keep_files = keep,
            "TmpFileWriter log rotation completed"
        );

        Ok(true)
    }
}

#[cfg(test)]
mod rotation_tests {
    use tempfile::TempDir;

    use substrate_domain::subprocess::stream::Stream;
    use substrate_domain::value_objects::JobId;

    use super::TmpFileWriter;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// `rotate_if_needed` must return `Ok(false)` and leave the transit file unchanged
    /// when the bytes written are below the `max_bytes_per_file` threshold.
    #[tokio::test]
    async fn rotate_if_needed_no_op_under_threshold() -> TestResult {
        let dir = TempDir::new()?;
        let job_id = JobId::now_v7();
        let writer = TmpFileWriter::create(dir.path(), &job_id, Stream::Stdout)
            .await?
            .with_rotation(1024, 2);

        let payload = vec![0u8; 100];
        writer.write(&payload).await?;

        let rotated = writer.rotate_if_needed().await?;
        assert!(!rotated, "should not rotate when under threshold");
        assert!(
            writer.tmp_path().exists(),
            "transit file must still exist when no rotation occurred"
        );
        assert_eq!(
            writer.bytes_written(),
            100,
            "bytes_written must not reset on no-op"
        );
        Ok(())
    }

    /// `rotate_if_needed` must return `Ok(true)`, rename the current transit file to
    /// `<final>.1`, and reopen a fresh transit file when the threshold is crossed.
    /// After rotation `bytes_written` must be reset to 0.
    #[tokio::test]
    async fn rotate_if_needed_rotates_on_threshold() -> TestResult {
        let dir = TempDir::new()?;
        let job_id = JobId::now_v7();
        let writer = TmpFileWriter::create(dir.path(), &job_id, Stream::Stdout)
            .await?
            .with_rotation(1024, 2); // threshold 1 KiB, keep 2 archives

        // Write 2 KiB — exceeds the 1 KiB threshold.
        let payload = vec![b'x'; 2048];
        writer.write(&payload).await?;

        assert!(
            writer.bytes_written() >= 1024,
            "pre-condition: bytes_written must meet or exceed threshold"
        );

        let rotated = writer.rotate_if_needed().await?;
        assert!(rotated, "rotation must occur when threshold is crossed");

        // After rotation:
        // - `<final_path>.1` (the archive) must exist.
        let archive_1 = {
            use std::ffi::OsString;
            let mut s: OsString = writer.final_path().as_os_str().to_owned();
            s.push(".1");
            std::path::PathBuf::from(s)
        };
        assert!(
            archive_1.exists(),
            "archive <final_path>.1 must exist after rotation"
        );

        // - Fresh transit file must be present (reopened for continued writes).
        assert!(
            writer.tmp_path().exists(),
            "fresh transit file must be reopened after rotation"
        );

        // - bytes_written must be reset.
        assert_eq!(
            writer.bytes_written(),
            0,
            "bytes_written must reset to 0 after rotation"
        );

        // Sanity: the archive contains the original 2 KiB payload.
        let contents = tokio::fs::read(&archive_1).await?;
        assert_eq!(
            contents.len(),
            2048,
            "archive must contain original payload"
        );
        Ok(())
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
