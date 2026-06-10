//! `JobRegistryPort` — inbound port for the async job control-plane per ADR-0040.
//!
//! Implemented by `InMemoryJobRegistry` in `substrate-jobs` (adapter crate).
//! Domain code (and MCP tool handlers) depend only on this trait.

use std::time::Duration;

use async_trait::async_trait;
use futures::future::BoxFuture;

use crate::errors::{SubstrateError, SubstrateResult};
use crate::jobs::bucket::JobBucket;
use crate::jobs::entry::JobEntry;
use crate::jobs::state::JobState;
use crate::value_objects::pagination::PageSize;
use crate::value_objects::{ClientId, IdempotencyKey, JobId, PageCursor};

/// Inbound port for submitting and managing async jobs per ADR-0040.
///
/// All methods are cancel-safe: they check the `CancellationToken` at the
/// start and at each await point using `tokio::select!` with `biased` per ADR-0037.
/// The adapter implementation is responsible for token propagation.
#[async_trait]
pub trait JobRegistryPort: Send + Sync {
    /// Submits a new job and returns its `JobId`.
    ///
    /// If an `idempotency_key` is provided and a matching dedup entry exists
    /// (same client, tool, key, and args hash), the existing job ID is returned
    /// without spawning a new worker.
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_QUOTA_EXCEEDED` when any concurrent-job quota is saturated.
    async fn submit(&self, request: JobSubmitRequest) -> SubstrateResult<JobId>;

    /// Returns a point-in-time snapshot of the job's state.
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_JOB_NOT_FOUND` when the job has expired or never existed.
    async fn status(&self, id: &JobId) -> SubstrateResult<JobEntry>;

    /// Returns the final `ToolOutput` for a completed job.
    ///
    /// When `wait` is `Some(d)`, long-polls up to `d` for the job to reach a
    /// terminal state. The server-side cap is `jobs.result_max_wait_ms`.
    ///
    /// **ADR-0059 — handler-side substitution:** the MCP tool handler substitutes
    /// the configured `jobs.quotas.result_default_wait_ms` (default 5 000 ms) when
    /// the caller omits `wait_ms` entirely. An explicit `wait_ms = 0` is preserved
    /// as a fast-return (non-blocking poll). This port trait always receives the
    /// already-substituted value as `Some(d)` or `None` (fast-return). The
    /// substitution logic and the boot guard that rejects an invalid wait window
    /// live exclusively in the handler layer; the port and the registry adapter are
    /// unaware of the default. See [ADR-0059](../../../docs/arch/adr/0059-universal-wait-timeout-enforcement.md).
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_JOB_NOT_FOUND` — job expired or never existed.
    /// - `SUBSTRATE_RESULT_WAIT_EXCEEDED` — requested wait exceeds the server cap.
    /// - `SUBSTRATE_JOB_CANCELLED` — job reached the cancelled terminal state.
    /// - `SUBSTRATE_JOB_TIMED_OUT` — job exceeded its per-tool timeout.
    async fn result(&self, id: &JobId, wait: Option<Duration>) -> SubstrateResult<JobResult>;

    /// Cancels the job by triggering its child `CancellationToken`.
    ///
    /// Idempotent: a second call on a terminal job returns `Ok(current_state)`.
    /// Returns synchronously after token cancellation is triggered; does not wait
    /// for the worker to acknowledge cancellation.
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_JOB_NOT_FOUND` — job expired or never existed.
    async fn cancel(&self, id: &JobId) -> SubstrateResult<JobState>;

    /// Returns a paginated list of jobs visible to the requesting client.
    ///
    /// Cross-client visibility is forbidden: each client sees only its own jobs.
    /// Pagination uses base64-opaque cursors per ADR-0008 (max 500).
    ///
    /// `page_size` is a validated [`PageSize`] value object per ADR-0060. The
    /// `job_list` and `tasks/list` MCP surfaces do not expose `page_size` on the
    /// wire, so the handler substitutes [`PageSize::default`] (50); the adapter
    /// caps the effective page at 500 (ADR-0008).
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_INVALID_ARGUMENT` — malformed cursor.
    async fn list(
        &self,
        client_id: &ClientId,
        cursor: Option<PageCursor>,
        page_size: PageSize,
    ) -> SubstrateResult<JobPage>;
}

/// Submission parameters for a new async job.
///
/// The `execute` field carries the actual tool work as a `BoxFuture`. The registry
/// spawns it as a `tokio` task and drives it to completion, wiring the
/// `CancellationToken` via `tokio::select! biased` per ADR-0037. Callers must box
/// the future with `Box::pin(async move { ... })` before constructing this struct.
pub struct JobSubmitRequest {
    /// The MCP client that is submitting the job.
    pub client_id: ClientId,

    /// Fully-qualified MCP tool name (e.g., `archive_tar_create`).
    pub tool: String,

    /// Static dispatch bucket for the tool.
    pub bucket: JobBucket,

    /// Client-supplied idempotency key for deduplication.
    pub idempotency_key: Option<IdempotencyKey>,

    /// Serialised tool arguments (used as part of the dedup key computation).
    pub args_json: serde_json::Value,

    /// The tool work to execute asynchronously inside the job worker.
    ///
    /// The registry spawns this future via `tokio::spawn`, wrapped in a
    /// `tokio::select! biased` block so the job's child `CancellationToken`
    /// can interrupt it cooperatively per ADR-0037. On success the value
    /// is stored in the job's result watch channel as `JobResult::Succeeded(v)`;
    /// on error as `JobResult::Failed(e)`.
    pub execute: BoxFuture<'static, SubstrateResult<serde_json::Value>>,
}

/// A page of job entries returned by [`JobRegistryPort::list`].
#[derive(Debug)]
pub struct JobPage {
    /// Jobs visible to the requesting client in this page.
    pub jobs: Vec<JobEntry>,

    /// Opaque cursor for the next page; `None` when this is the last page.
    pub next_cursor: Option<PageCursor>,
}

/// Terminal result of an async job, returned by [`JobRegistryPort::result`].
#[derive(Debug)]
pub enum JobResult {
    /// The job completed successfully; carries the serialised tool output.
    Succeeded(serde_json::Value),

    /// The job terminated with a domain error.
    Failed(SubstrateError),

    /// The job was cancelled by the client or during graceful shutdown.
    Cancelled,

    /// The job exceeded its configured per-tool timeout.
    TimedOut,
}
