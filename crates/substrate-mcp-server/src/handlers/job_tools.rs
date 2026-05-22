//! MCP tool handlers for the async job control-plane per ADR-0040.
//!
//! These thin wrappers are preserved for backwards compatibility with the Wave B
//! scaffold. The canonical dispatch path is now in `dispatcher.rs`:
//! `ToolDispatcher::handle_job_status`, `handle_job_result`, `handle_job_cancel`,
//! and `handle_job_list` inline the logic directly on `Arc<dyn JobRegistryPort>`.
//!
//! The free functions below are retained so that integration tests can invoke
//! individual job control-plane operations without going through the full
//! `ToolDispatcher::dispatch` match arm.

#![allow(clippy::redundant_pub_crate, reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates")]

// Retained for integration-test access; not yet called by the production path.
#![expect(
    dead_code,
    reason = "integration tests will call these helpers directly in Wave G"
)]

use std::time::Duration;

use substrate_domain::{
    JobRegistryPort, SubstrateResult,
    jobs::entry::JobEntry,
    jobs::state::JobState,
    ports::job_registry::{JobPage, JobResult},
    value_objects::{ClientId, JobId, PageCursor},
};

/// Handles `job_status` — returns a point-in-time `JobEntry` snapshot.
pub(crate) async fn handle_job_status(
    registry: &dyn JobRegistryPort,
    job_id: &JobId,
) -> SubstrateResult<JobEntry> {
    tracing::debug!(%job_id, "job_status called");
    registry.status(job_id).await
}

/// Handles `job_result` — returns the terminal result, optionally long-polling.
///
/// `wait_ms` is capped at `jobs.quotas.result_max_wait_ms` by the registry
/// implementation.
pub(crate) async fn handle_job_result(
    registry: &dyn JobRegistryPort,
    job_id: &JobId,
    wait_ms: Option<u64>,
) -> SubstrateResult<JobResult> {
    let wait = wait_ms.map(Duration::from_millis);
    tracing::debug!(%job_id, ?wait, "job_result called");
    registry.result(job_id, wait).await
}

/// Handles `job_cancel` — triggers cancellation token for the job.
///
/// Idempotent: a second call on a terminal job returns `Ok(current_state)`.
pub(crate) async fn handle_job_cancel(
    registry: &dyn JobRegistryPort,
    job_id: &JobId,
) -> SubstrateResult<JobState> {
    tracing::debug!(%job_id, "job_cancel called");
    registry.cancel(job_id).await
}

/// Handles `job_list` — paginated list of jobs visible to the caller.
///
/// Cross-client visibility is forbidden: the registry enforces `client_id`
/// scoping per ADR-0040.
pub(crate) async fn handle_job_list(
    registry: &dyn JobRegistryPort,
    client_id: &ClientId,
    cursor: Option<PageCursor>,
) -> SubstrateResult<JobPage> {
    tracing::debug!(%client_id, "job_list called");
    registry.list(client_id, cursor).await
}
