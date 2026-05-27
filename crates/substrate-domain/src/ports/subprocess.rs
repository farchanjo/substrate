//! `SubprocessPort` — inbound port for the subprocess bounded context per ADR-0052.
//!
//! Implemented by the `substrate-subprocess` adapter crate (behind the `subprocess`
//! Cargo feature). The composition root wires an `Arc<dyn SubprocessPort>` when
//! the feature is active, or a `NoopSubprocessPort` Null Object when disabled.
//!
//! Cancellation: this port uses the same `CancelSignal` abstraction as
//! `FsIndexPort` — a thin domain trait backed by `tokio_util::sync::CancellationToken`
//! in the adapter. This keeps `substrate-domain` free of tokio-util.
//!
//! References: ADR-0052, ADR-0053, ADR-0054.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::errors::SubstrateResult;
use crate::subprocess::errors::SubprocessError;
use crate::subprocess::handle::SubprocessHandle;
use crate::subprocess::pagination::{SubprocessSearchRequest, SubprocessSearchResult};
use crate::subprocess::request::SubprocessRequest;
use crate::subprocess::state::SubprocessState;
use crate::value_objects::pagination::PageSize;
use crate::value_objects::{ClientId, JobId};

// Re-export CancelSignal so callers of this port do not need to import fs_index.
pub use crate::ports::fs_index::CancelSignal;

/// Inbound port for spawning and managing subprocesses per ADR-0052.
///
/// Adapter implementations live in `substrate-subprocess` (gated behind the
/// `subprocess` Cargo feature). Domain code and MCP tool handlers depend only
/// on this trait.
///
/// All `async fn` methods are cancel-safe at the `await` boundary per ADR-0037:
/// adapters MUST check the `CancelSignal` / `CancellationToken` at each `await`
/// point using `tokio::select! biased` with the work arm first.
#[async_trait]
pub trait SubprocessPort: Send + Sync {
    /// Spawns a new child process for the given request and returns the initial handle.
    ///
    /// Performs all security checks (Layer 1–5 per ADR-0052), triggers elicitation
    /// if `request.elicitation_confirmed` is `false`, and registers a `JobEntry`
    /// with `Bucket::E` in the `JobRegistry` before returning.
    ///
    /// `cancel` is the job-scoped signal derived from the server's root
    /// `CancellationToken`. The adapter uses it inside `tokio::select! biased` to
    /// interrupt the spawn path cooperatively per ADR-0037.
    ///
    /// # Errors
    ///
    /// - `SubprocessError::BinaryNotAllowed` — binary not in allowlist.
    /// - `SubprocessError::CwdOutsideAllowlist` — cwd outside allowed paths.
    /// - `SubprocessError::ElicitationRequired` — confirmation not provided.
    /// - `SubprocessError::QuotaExceeded` — per-client or global quota reached.
    /// - `SubprocessError::SpawnFailed` — OS `fork`/`exec` returned an error.
    async fn spawn(
        &self,
        req: SubprocessRequest,
        cancel: &dyn CancelSignal,
    ) -> Result<SubprocessHandle, SubprocessError>;

    /// Returns a paginated list of subprocess handles visible to `client_id`.
    ///
    /// Cross-client visibility is forbidden: each client sees only its own
    /// subprocess jobs. Pagination uses base64-opaque cursors per ADR-0008.
    ///
    /// `state_filter` when `Some`, restricts results to handles in the listed
    /// states. `None` returns all states.
    ///
    /// # Errors
    ///
    /// - `SubstrateError::InvalidArgument` — malformed cursor.
    async fn list(
        &self,
        client_id: &ClientId,
        state_filter: Option<&[SubprocessState]>,
        page_cursor: Option<&str>,
        page_size: PageSize,
    ) -> SubstrateResult<(Vec<SubprocessHandle>, Option<String>)>;

    /// Cancels a running subprocess by triggering the cascade kill chain.
    ///
    /// When `force` is `true`, SIGKILL is sent immediately without waiting for
    /// `cascade_drain_secs`. When `false`, SIGTERM is sent first with a drain window.
    ///
    /// Idempotent: a second call on a terminal job returns `Ok(current_state)`.
    ///
    /// # Errors
    ///
    /// - `SubstrateError::JobNotFound` — no subprocess with the given `job_id`.
    async fn cancel(&self, job_id: &JobId, force: bool) -> SubstrateResult<SubprocessState>;

    /// Returns the terminal result for a completed subprocess.
    ///
    /// When `wait_ms > 0`, long-polls up to `wait_ms` milliseconds for the
    /// subprocess to reach a terminal state. The server-side cap is
    /// `jobs.result_max_wait_ms`.
    ///
    /// **ADR-0059 — handler-side substitution:** the MCP tool handler substitutes
    /// the configured `jobs.quotas.result_default_wait_ms` (default 5 000 ms) when
    /// the caller omits `wait_ms` entirely. An explicit caller-supplied `wait_ms = 0`
    /// is preserved as a fast-return (non-blocking poll). This port trait always
    /// receives the already-substituted value; the substitution and the boot guard
    /// that rejects an invalid wait window reside exclusively in the handler layer.
    /// See [ADR-0059](../../../docs/arch/adr/0059-universal-wait-timeout-enforcement.md).
    ///
    /// When `include_aggregates` is `false`, the `stdout_aggregate` and
    /// `stderr_aggregate` fields in the result are empty to reduce response size.
    ///
    /// # Errors
    ///
    /// - `SubstrateError::JobNotFound` — no subprocess with the given `job_id`.
    /// - `SubstrateError::Timeout` — subprocess still running after `wait_ms`.
    async fn result(
        &self,
        job_id: &JobId,
        wait_ms: u32,
        include_aggregates: bool,
    ) -> SubstrateResult<SubprocessResult>;

    /// Sends a POSIX signal to a subprocess by `job_id`.
    ///
    /// `target` controls whether the signal is delivered to the direct child
    /// process only or to the entire process group (for cascade kills).
    ///
    /// Destructive signals (`SIGKILL`, `SIGTERM`, `SIGSTOP`) require elicitation
    /// confirmation per ADR-0052 §"subprocess.signal".
    ///
    /// # Errors
    ///
    /// - `SubstrateError::JobNotFound` — no subprocess with the given `job_id`.
    /// - `SubprocessError::ElicitationRequired` — destructive signal without confirmation.
    async fn signal(
        &self,
        job_id: &JobId,
        signal_name: SubprocessSignalName,
        target: SignalTarget,
    ) -> SubstrateResult<()>;

    /// Searches subprocess output lines by regex pattern with pagination.
    ///
    /// Applies the compiled regex from `req.pattern` to the captured stdout and/or
    /// stderr line buffers for the identified job, returning `SearchMatch` entries
    /// ordered and paginated according to `req.pagination`.
    ///
    /// When `req.case_insensitive` is `true`, ASCII case is ignored during matching.
    ///
    /// `req.validate()` MUST be called before invoking the adapter; the adapter
    /// MAY call it again as a defense-in-depth measure.
    ///
    /// # Errors
    ///
    /// - `SubprocessError::InvalidRequest` — pattern length or pagination out of range.
    /// - `SubstrateError::JobNotFound` — no subprocess with the given `job_id`.
    async fn search(
        &self,
        req: SubprocessSearchRequest,
    ) -> Result<SubprocessSearchResult, SubprocessError>;
}

// ---- Supporting types -------------------------------------------------------

/// Terminal result of a subprocess, returned by [`SubprocessPort::result`].
///
/// See ADR-0054 §"Result Shape" and ADR-0033 amendment 2026-05-24.
///
/// When `capture_kind == TmpFile`, the adapter atomically renames the transit
/// file (`<tmp_root>/.substrate-subprocess-stream-<job_id>.<stream>.tmp.<uuid7>`)
/// to the final path (`<tmp_root>/.substrate-subprocess-stream-<job_id>.<stream>`)
/// on terminal `Succeeded`, then populates [`stdout_tmp_path`] and
/// [`stderr_tmp_path`]. For non-`Succeeded` terminal states the files are cleaned
/// up by the cascade kill chain and both fields remain `None`.
///
/// [`stdout_tmp_path`]: SubprocessResult::stdout_tmp_path
/// [`stderr_tmp_path`]: SubprocessResult::stderr_tmp_path
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubprocessResult {
    /// Terminal state of the subprocess.
    pub terminal_state: SubprocessState,

    /// OS exit code.
    ///
    /// `None` when the process was killed via `SIGKILL` (exit code is undefined
    /// on POSIX) or when `terminal_state` is `Cancelled` before the process exited.
    pub exit_code: Option<i32>,

    /// Aggregated stdout bytes up to `aggregate_buffer_bytes` per ADR-0054.
    ///
    /// Empty when `include_aggregates = false` in the `result` call.
    /// The adapter base64-encodes this for the wire; the domain holds raw bytes.
    pub stdout_aggregate: Vec<u8>,

    /// Aggregated stderr bytes up to `aggregate_buffer_bytes` per ADR-0054.
    ///
    /// Empty when `include_aggregates = false` in the `result` call.
    pub stderr_aggregate: Vec<u8>,

    /// `true` when the stdout ring buffer overflowed and oldest bytes were discarded.
    pub stdout_aggregate_truncated: bool,

    /// `true` when the stderr ring buffer overflowed and oldest bytes were discarded.
    pub stderr_aggregate_truncated: bool,

    /// Filesystem path of the final stdout capture file.
    ///
    /// Populated only when `capture_kind == TmpFile` AND the subprocess reached
    /// `Succeeded`. The file contains the complete stdout byte stream; the ring
    /// buffer aggregate is also populated as a safety net per ADR-0054 amendment
    /// 2026-05-24.
    ///
    /// `None` for `Stream`, `InMemory`, or any non-`Succeeded` terminal state.
    ///
    /// References: ADR-0033 §"Transactional Write Pattern", ADR-0054 §"`TmpFile` Branch".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_tmp_path: Option<std::path::PathBuf>,

    /// Filesystem path of the final stderr capture file.
    ///
    /// Populated only when `capture_kind == TmpFile` AND the subprocess reached
    /// `Succeeded`. The file contains the complete stderr byte stream; the ring
    /// buffer aggregate is also populated as a safety net per ADR-0054 amendment
    /// 2026-05-24.
    ///
    /// `None` for `Stream`, `InMemory`, or any non-`Succeeded` terminal state.
    ///
    /// References: ADR-0033 §"Transactional Write Pattern", ADR-0054 §"`TmpFile` Branch".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_tmp_path: Option<std::path::PathBuf>,

    /// Total number of stream chunks dropped due to mpsc backpressure per ADR-0054.
    pub stream_chunks_dropped: u64,

    /// Wall-clock duration from `started_at` to terminal state entry in milliseconds.
    pub duration_ms: u64,

    /// Total bytes emitted by stdout over the lifetime of the process.
    pub stdout_bytes_total: u64,

    /// Total bytes emitted by stderr over the lifetime of the process.
    pub stderr_bytes_total: u64,

    /// Timestamp when the subprocess transitioned to the terminal state.
    #[serde(with = "time::serde::rfc3339")]
    pub terminal_at: OffsetDateTime,

    // ---- Pagination fields (ADR-0057) ----------------------------------------
    //
    // Populated when `SubprocessResultRequest.pagination` is `Some`. All six
    // fields are `None` when pagination was not requested, keeping backward
    // compatibility with callers that do not supply a pagination cursor.
    /// Paginated stdout lines for this result page.
    ///
    /// `None` when the caller did not include a `pagination` cursor in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_lines: Option<Vec<String>>,

    /// Total number of stdout lines available across all pages.
    ///
    /// `None` when the caller did not include a `pagination` cursor in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_total_lines: Option<u64>,

    /// Offset to pass as `pagination.offset` to fetch the next stdout page.
    ///
    /// `None` when there are no more stdout lines or pagination was not requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_next_offset: Option<u64>,

    /// Paginated stderr lines for this result page.
    ///
    /// `None` when the caller did not include a `pagination` cursor in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_lines: Option<Vec<String>>,

    /// Total number of stderr lines available across all pages.
    ///
    /// `None` when the caller did not include a `pagination` cursor in the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_total_lines: Option<u64>,

    /// Offset to pass as `pagination.offset` to fetch the next stderr page.
    ///
    /// `None` when there are no more stderr lines or pagination was not requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_next_offset: Option<u64>,
}

/// POSIX signal names available for `subprocess.signal`.
///
/// Only these signals are permitted via the MCP tool surface; raw integer signals
/// are not accepted. Destructive signals (`SIGKILL`, `SIGTERM`, `SIGSTOP`) require
/// elicitation confirmation per ADR-0052.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SubprocessSignalName {
    /// Request graceful shutdown (default cascade signal per ADR-0053).
    Sigterm,
    /// Request interrupt (Ctrl-C equivalent).
    Sigint,
    /// Force kill — cannot be caught or ignored.
    Sigkill,
    /// Pause execution.
    Sigstop,
    /// Resume paused execution.
    Sigcont,
    /// Hang up (terminal disconnect).
    Sighup,
    /// User-defined signal 1.
    Sigusr1,
    /// User-defined signal 2.
    Sigusr2,
}

impl std::fmt::Display for SubprocessSignalName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Sigterm => "SIGTERM",
            Self::Sigint => "SIGINT",
            Self::Sigkill => "SIGKILL",
            Self::Sigstop => "SIGSTOP",
            Self::Sigcont => "SIGCONT",
            Self::Sighup => "SIGHUP",
            Self::Sigusr1 => "SIGUSR1",
            Self::Sigusr2 => "SIGUSR2",
        };
        f.write_str(s)
    }
}

/// Controls whether a signal is delivered to the direct child or the entire group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalTarget {
    /// Deliver to the child PID only (`kill(pid, sig)`).
    Process,
    /// Deliver to the entire process group (`killpg(pgid, sig)`) per ADR-0053.
    ProcessGroup,
}
