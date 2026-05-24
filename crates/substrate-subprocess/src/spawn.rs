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
use std::sync::atomic::AtomicU64;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use substrate_domain::subprocess::{StdinKind, SubprocessError, SubprocessRequest};
use substrate_domain::value_objects::{JobId, ProcessGroup};

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
    /// Cleaned up explicitly in the cancel path per ADR-0033 + ADR-0014
    /// (panic=abort means Drop is not guaranteed).
    pub tmp_files: Mutex<Vec<PathBuf>>,

    /// Platform watchdog pipe (macOS: write end; Linux: zero-cost no-op).
    pub watchdog: WatchdogPipe,

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
/// 5. Returns a [`ChildHandle`] for use by reader tasks and the cascade kill chain.
///
/// # Errors
///
/// - [`SubprocessError::InvalidRequest`] — field validation failed.
/// - [`SubprocessError::SpawnFailed`] — OS `fork`/`exec` returned an error.
///
/// References: ADR-0052 §"Subprocess Sandbox", ADR-0053 §"Process Group Leadership".
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

    Ok(ChildHandle {
        job_id,
        process_group,
        cancel,
        tmp_files: Mutex::new(Vec::new()),
        watchdog,
        child: Mutex::new(Some(child)),
        stdout_ring: Arc::new(Mutex::new(RingBuffer::new(aggregate_buffer_bytes))),
        stderr_ring: Arc::new(Mutex::new(RingBuffer::new(aggregate_buffer_bytes))),
        stdout_bytes_total: Arc::new(AtomicU64::new(0)),
        stderr_bytes_total: Arc::new(AtomicU64::new(0)),
        stream_chunks_dropped: Arc::new(AtomicU64::new(0)),
        stdout_seq: Arc::new(AtomicU64::new(0)),
        stderr_seq: Arc::new(AtomicU64::new(0)),
    })
}
