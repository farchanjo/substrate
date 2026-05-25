//! Supervised child spawn: builds and launches a `tokio::process::Command`,
//! installing the pre-exec hook and watchdog pipe per ADR-0053.
//!
//! [`ChildHandle`] is the in-process live representation of a running subprocess.
//! It holds the OS-level process group, cancellation token, ring-buffer aggregates,
//! stream drop counter, and the `tokio::process::Child` under a `Mutex` so that
//! `.wait()` can be called from the cascade kill path.
//!
//! References: ADR-0052 §"`SubprocessHandle`", ADR-0053, ADR-0054.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU8};

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use substrate_domain::subprocess::request::CaptureKind;
use substrate_domain::subprocess::state::SubprocessState;
use substrate_domain::subprocess::{StdinKind, SubprocessError, SubprocessRequest};
use substrate_domain::value_objects::{JobId, ProcessGroup};

// ---------------------------------------------------------------------------
// SubprocessState ↔ u8 conversion helpers (private to this crate).
// AtomicU8 is used in ChildHandle.state for lock-free reads by snapshot_handle.
// ---------------------------------------------------------------------------

/// Maps a [`SubprocessState`] to its stable u8 discriminant.
///
/// Values are stable internal constants — never persisted or sent over wire.
/// Existing 0-6 assignments are frozen; new ADR-0056 variants occupy 7-9.
#[inline]
pub(crate) const fn state_to_u8(s: SubprocessState) -> u8 {
    match s {
        SubprocessState::Pending => 0,
        SubprocessState::Running => 1,
        SubprocessState::Succeeded => 2,
        SubprocessState::Failed => 3,
        SubprocessState::Cancelled => 4,
        SubprocessState::TimedOut => 5,
        SubprocessState::Killed => 6,
        // ADR-0056 additions — must not overlap with 0-6.
        SubprocessState::Starting => 7,
        SubprocessState::Ready => 8,
        SubprocessState::Restarting => 9,
    }
}

/// Maps a u8 discriminant back to a [`SubprocessState`].
///
/// An unrecognised byte (written by a future variant before upgrade) falls back
/// to `Running` so that the process is treated as still-live rather than silently
/// terminal — the conservative safe choice.
#[inline]
pub(crate) const fn u8_to_state(v: u8) -> SubprocessState {
    match v {
        0 => SubprocessState::Pending,
        1 => SubprocessState::Running,
        2 => SubprocessState::Succeeded,
        3 => SubprocessState::Failed,
        4 => SubprocessState::Cancelled,
        5 => SubprocessState::TimedOut,
        6 => SubprocessState::Killed,
        // ADR-0056 additions.
        7 => SubprocessState::Starting,
        8 => SubprocessState::Ready,
        9 => SubprocessState::Restarting,
        _ => SubprocessState::Running,
    }
}

use crate::tmp_file::TmpFileWriter;
use crate::watchdog::WatchdogPipe;

/// The maximum decoded size of a single stream chunk, per ADR-0054.
pub const CHUNK_CAPACITY: usize = 4096;

/// Bounded mpsc channel capacity per stream, per ADR-0054.
pub const MPSC_CAPACITY: usize = 64;

/// In-process live representation of a spawned subprocess.
///
/// Created by [`spawn_supervised`] immediately after the OS child is started.
/// Shared across the reader tasks (stdout/stderr capture), the cascade kill chain,
/// and the registry's result path via `Arc<ChildHandle>`.
///
/// References: ADR-0052 §"`SubprocessHandle`", ADR-0053, ADR-0054.
#[derive(Debug)]
pub struct ChildHandle {
    /// Correlating job identifier. Triple-equality with `progressToken` and
    /// `correlation_id` per ADR-0040.
    pub job_id: JobId,

    /// OS process group descriptor produced by `setsid()` per ADR-0053.
    ///
    /// `pgid` == `pid` because the child called `setsid()` in `pre_exec`.
    pub process_group: ProcessGroup,

    /// Job-scoped cancellation token. A child of the server root token.
    ///
    /// Cancelled when: the MCP client cancels the job, server SIGTERM fires,
    /// or `timeout_secs` elapses. The cascade kill chain observes this token.
    pub cancel: CancellationToken,

    /// Temporary file paths registered during this invocation.
    ///
    /// Both the transit (`.tmp.<uuid7>`) and the final paths are pushed here at
    /// spawn time so the cascade cleanup chain handles interrupted renames too.
    /// The transit entry is removed from this Vec by `ChildHandle::unregister_tmp_path`
    /// after a successful [`TmpFileWriter::finalize`]; the final path entry stays
    /// until the client reads the result and explicitly requests cleanup.
    ///
    /// Cleaned up explicitly in the cancel path per ADR-0033 + ADR-0014
    /// (panic=abort means Drop is not guaranteed).
    pub tmp_files: Mutex<Vec<PathBuf>>,

    /// Platform watchdog pipe (macOS: write end; Linux: zero-cost no-op).
    pub watchdog: WatchdogPipe,

    /// Live lifecycle state stored atomically for lock-free reads by `snapshot_handle`.
    ///
    /// Initialized to `Running` at spawn time. Updated by the dispatcher task
    /// (after `wait_exit` resolves) and by the cancel path (after cascade kill).
    /// Reads use `Ordering::SeqCst` to guarantee visibility across tasks.
    ///
    /// Encoding: see `state_to_u8` / `u8_to_state` in this module.
    pub state: Arc<AtomicU8>,

    /// The `tokio::process::Child` under a mutex so that `wait()` can be called
    /// from the cascade kill chain without racing with the reader tasks.
    pub child: Mutex<Option<tokio::process::Child>>,

    /// Stdout ring buffer for `subprocess.result` aggregates per ADR-0054.
    ///
    /// Populated directly by the reader task (independent of the mpsc channel,
    /// so dropped mpsc chunks still enter the ring buffer).
    pub stdout_ring: Arc<Mutex<RingBuffer>>,

    /// Stderr ring buffer for `subprocess.result` aggregates per ADR-0054.
    pub stderr_ring: Arc<Mutex<RingBuffer>>,

    /// Total stdout bytes written to the ring buffer (monotonically increasing).
    pub stdout_bytes_total: Arc<AtomicU64>,

    /// Total stderr bytes written to the ring buffer (monotonically increasing).
    pub stderr_bytes_total: Arc<AtomicU64>,

    /// Cumulative count of stream chunks dropped due to mpsc backpressure.
    ///
    /// Surfaced in the terminal job result per ADR-0054.
    pub stream_chunks_dropped: Arc<AtomicU64>,

    /// Monotonic sequence counter for stdout stream chunks per ADR-0054.
    pub stdout_seq: Arc<AtomicU64>,

    /// Monotonic sequence counter for stderr stream chunks per ADR-0054.
    pub stderr_seq: Arc<AtomicU64>,

    /// The capture kind requested for this job per ADR-0054.
    ///
    /// Used by the registry's `result()` method to decide whether to finalize
    /// tmp files and populate [`SubprocessResult::stdout_tmp_path`] /
    /// [`SubprocessResult::stderr_tmp_path`].
    pub capture_kind: CaptureKind,

    /// Stdout tmp file writer; `Some` only when `capture_kind == TmpFile`.
    ///
    /// The writer is wrapped in `Arc` so it can be shared with the reader task
    /// and the registry result path without cloning the file handle.
    ///
    /// References: ADR-0033 §"Transactional Write Pattern", ADR-0054 §"`TmpFile` Branch".
    pub stdout_tmp_writer: Option<Arc<TmpFileWriter>>,

    /// Stderr tmp file writer; `Some` only when `capture_kind == TmpFile`.
    ///
    /// References: ADR-0033 §"Transactional Write Pattern", ADR-0054 §"`TmpFile` Branch".
    pub stderr_tmp_writer: Option<Arc<TmpFileWriter>>,
}

impl ChildHandle {
    /// Waits for the child process to exit and returns its exit status.
    ///
    /// Locks the `child` mutex, takes the `Child` out of the `Option`, and calls
    /// `Child::wait()`. Returns `None` if the child was already waited on.
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if `Child::wait()` fails.
    pub async fn wait_exit(&self) -> std::io::Result<Option<std::process::ExitStatus>> {
        // Take the child out of the mutex before awaiting to avoid holding
        // the MutexGuard across .await (clippy::significant_drop_tightening).
        let child_opt = {
            let mut guard = self.child.lock().await;
            guard.take()
        };
        let Some(mut child) = child_opt else {
            return Ok(None);
        };
        child.wait().await.map(Some)
    }

    /// Removes `path` from the registered tmp-file list.
    ///
    /// Called after [`TmpFileWriter::finalize`] succeeds to deregister the
    /// transit path (which no longer exists after the rename). The final path
    /// remains registered so the cascade reaper can handle an interrupted
    /// rename scenario.
    ///
    /// References: ADR-0033 §"Transactional Write Pattern".
    pub async fn unregister_tmp_path(&self, path: &std::path::Path) {
        let mut guard = self.tmp_files.lock().await;
        guard.retain(|p| p != path);
    }
}

/// A fixed-capacity ring buffer retaining the most recent bytes.
///
/// Per ADR-0054 §"Aggregate Retention Ring Buffer": newest-byte-wins eviction.
/// When the ring is full the oldest bytes are overwritten. The `truncated` flag
/// is set on the first overflow and never cleared.
#[derive(Debug)]
pub struct RingBuffer {
    buf: Vec<u8>,
    capacity: usize,
    /// `true` once any byte has been dropped (oldest-byte eviction occurred).
    pub truncated: bool,
}

impl RingBuffer {
    /// Creates a new ring buffer with the given byte capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity.min(1024 * 1024)),
            capacity,
            truncated: false,
        }
    }

    /// Appends `data` to the ring buffer, evicting oldest bytes when full.
    pub fn push(&mut self, data: &[u8]) {
        let space = self.capacity.saturating_sub(self.buf.len());
        if space >= data.len() {
            // Fits without eviction.
            self.buf.extend_from_slice(data);
        } else if data.len() >= self.capacity {
            // Incoming data is larger than or equal to the buffer; keep the last `capacity` bytes.
            let start = data.len() - self.capacity;
            self.buf.clear();
            self.buf.extend_from_slice(&data[start..]);
            self.truncated = true;
        } else {
            // Partial eviction: drop oldest bytes to make room.
            let evict = data.len() - space;
            self.buf.drain(..evict);
            self.buf.extend_from_slice(data);
            self.truncated = true;
        }
    }

    /// Returns the current retained bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }
}

/// Spawns a supervised child process from `req`, wired to `parent_cancel`.
///
/// Steps:
/// 1. Validates the request (domain-level in-memory checks per ADR-0052 §"Layer 5").
/// 2. Builds the `tokio::process::Command` with stdio pipes per ADR-0052.
/// 3. Installs watchdog pipe (macOS) and pre-exec hook (both platforms).
/// 4. Spawns the child and records its PID/PGID in a [`ProcessGroup`].
/// 5. When `req.capture_kind == TmpFile`: creates stdout and stderr [`TmpFileWriter`]
///    instances under `tmp_root` and registers both the transit and final paths in
///    `ChildHandle.tmp_files` for cascade cleanup per ADR-0033.
/// 6. Returns a [`ChildHandle`] for use by reader tasks and the cascade kill chain.
///
/// # Errors
///
/// - [`SubprocessError::InvalidRequest`] — field validation failed, or `TmpFile`
///   capture mode requested but `tmp_root` is `None`.
/// - [`SubprocessError::SpawnFailed`] — OS `fork`/`exec` returned an error, or
///   transit file could not be created.
///
/// References: ADR-0052 §"Subprocess Sandbox", ADR-0053 §"Process Group Leadership",
/// ADR-0033 §"Transactional Write Pattern", ADR-0054 §"`TmpFile` Branch".
#[expect(
    clippy::disallowed_types,
    reason = "substrate-subprocess is the single authorized host of tokio::process::Command \
              per ADR-0052 §\"Supersession of ADR-0044\"."
)]
#[expect(
    clippy::disallowed_methods,
    reason = "substrate-subprocess is the single authorized host of tokio::process::Command::new \
              per ADR-0052."
)]
pub async fn spawn_supervised(
    req: &SubprocessRequest,
    parent_cancel: CancellationToken,
    aggregate_buffer_bytes: usize,
    tmp_root: Option<&std::path::Path>,
) -> Result<ChildHandle, SubprocessError> {
    // Step 1: domain validation (no OS calls).
    req.validate()?;

    // Step 2: build the command.
    let mut cmd = tokio::process::Command::new(&req.binary_path);
    cmd.args(&req.args);
    cmd.current_dir(&req.cwd);

    // Clear environment then re-add only allowed keys.
    cmd.env_clear();
    for key in &req.env_allowlist {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
    // Apply explicit overrides (already validated for banned vars at domain level).
    for (k, v) in &req.env_override {
        cmd.env(k, v);
    }

    // Step 3: configure stdio per ADR-0052 §"STDIO Sanctity".
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    match &req.stdin_kind {
        StdinKind::None => {
            cmd.stdin(Stdio::null());
        },
        StdinKind::Piped => {
            cmd.stdin(Stdio::piped());
        },
        StdinKind::FilePath(path) => {
            let file = std::fs::File::open(path)
                .map_err(|e| SubprocessError::SpawnFailed { source: e })?;
            cmd.stdin(file);
        },
    }

    // Step 4: watchdog (macOS) — must be installed before pre_exec.
    let watchdog = crate::watchdog::install(&mut cmd)
        .map_err(|e| SubprocessError::SpawnFailed { source: e })?;

    // Step 5: pre-exec hook (setsid + prctl).
    crate::pre_exec::configure_pre_exec(&mut cmd);

    // Step 6: spawn.
    let child = cmd
        .spawn()
        .map_err(|e| SubprocessError::SpawnFailed { source: e })?;

    // Step 7: extract PID. After setsid() pgid == pid.
    let raw_pid = child.id().ok_or_else(|| SubprocessError::SpawnFailed {
        source: std::io::Error::other("child.id() returned None immediately after spawn"),
    })?;

    // pid is u32 from tokio; ProcessGroup requires i32 >= 2.
    let pid_i32 = i32::try_from(raw_pid).map_err(|_| SubprocessError::SpawnFailed {
        source: std::io::Error::other(format!("child pid {raw_pid} overflows i32")),
    })?;
    let process_group =
        ProcessGroup::new(pid_i32, pid_i32).map_err(|e| SubprocessError::SpawnFailed {
            source: std::io::Error::other(e.to_string()),
        })?;

    // Step 8: child-scoped cancellation token derived from parent.
    let cancel = parent_cancel.child_token();

    let job_id = JobId::now_v7();

    // Step 9: TmpFile writers (only when capture_kind == TmpFile).
    //
    // For Stream and InMemory: no file I/O, writers stay None.
    // For TmpFile: create transit files, register both transit AND final paths in
    //   tmp_files so the cascade reaper can handle interrupted renames.
    let (stdout_tmp_writer, stderr_tmp_writer, tmp_files_vec) = match req.capture_kind {
        CaptureKind::TmpFile => {
            let root = tmp_root.ok_or_else(|| SubprocessError::InvalidRequest {
                msg: "capture_kind TmpFile requires a configured subprocess.tmp_root; \
                      no tmp_root was provided to spawn_supervised"
                    .to_owned(),
            })?;
            let stdout_writer = TmpFileWriter::create(
                root,
                &job_id,
                substrate_domain::subprocess::stream::Stream::Stdout,
            )
            .await?;
            let stderr_writer = TmpFileWriter::create(
                root,
                &job_id,
                substrate_domain::subprocess::stream::Stream::Stderr,
            )
            .await?;
            // Register both transit AND final paths for cascade cleanup.
            // Cleanup of the transit path after finalize is handled by unregister_tmp_path.
            let registered = vec![
                stdout_writer.tmp_path().to_owned(),
                stdout_writer.final_path().to_owned(),
                stderr_writer.tmp_path().to_owned(),
                stderr_writer.final_path().to_owned(),
            ];
            (
                Some(Arc::new(stdout_writer)),
                Some(Arc::new(stderr_writer)),
                registered,
            )
        },
        CaptureKind::Stream | CaptureKind::InMemory => (None, None, Vec::new()),
    };

    Ok(ChildHandle {
        job_id,
        process_group,
        cancel,
        tmp_files: Mutex::new(tmp_files_vec),
        watchdog,
        state: Arc::new(AtomicU8::new(state_to_u8(SubprocessState::Running))),
        child: Mutex::new(Some(child)),
        stdout_ring: Arc::new(Mutex::new(RingBuffer::new(aggregate_buffer_bytes))),
        stderr_ring: Arc::new(Mutex::new(RingBuffer::new(aggregate_buffer_bytes))),
        stdout_bytes_total: Arc::new(AtomicU64::new(0)),
        stderr_bytes_total: Arc::new(AtomicU64::new(0)),
        stream_chunks_dropped: Arc::new(AtomicU64::new(0)),
        stdout_seq: Arc::new(AtomicU64::new(0)),
        stderr_seq: Arc::new(AtomicU64::new(0)),
        capture_kind: req.capture_kind.clone(),
        stdout_tmp_writer,
        stderr_tmp_writer,
    })
}
