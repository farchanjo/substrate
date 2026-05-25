//! Progress notification abstraction for the async job control-plane.
//!
//! Implementations push [`ProgressEvent`] values toward MCP clients over the
//! STDIO transport. The [`NoopProgressNotifier`] is used when a client submits
//! a tool call without a `progressToken` (no push channel desired).
//!
//! Per ADR-0040, events are only emitted while the job is in state `Running`.
//! Events after a terminal state transition are silently discarded by the caller.

use async_trait::async_trait;

use crate::jobs::progress::ProgressEvent;
use crate::ports::job_registry::JobResult;
use crate::value_objects::JobId;

/// Push-channel abstraction for job progress and completion notifications.
///
/// `substrate-mcp-server` supplies a concrete implementation that writes
/// `notifications/progress` frames onto the STDIO transport per ADR-0040.
/// In tests and headless contexts, [`NoopProgressNotifier`] is used.
///
/// Both methods are fire-and-forget from the caller's perspective: they MUST NOT
/// block the tokio executor and MUST be cancel-safe (no state modified after
/// the first `await` unless it can be rolled back).
#[async_trait]
pub trait ProgressNotifier: Send + Sync + std::fmt::Debug {
    /// Emits a progress update for a running job.
    ///
    /// Called after the throttle gate passes (250 ms OR 1% delta per ADR-0040).
    /// Implementations MUST be non-blocking; backpressure is handled at the
    /// bounded mpsc layer before this method is called.
    async fn notify_progress(&self, event: ProgressEvent);

    /// Emits the final terminal notification for a job.
    ///
    /// Called exactly once, after the job enters a terminal state and the result
    /// watch channel is set. The `progress` field in the synthetic event is set
    /// to 100 for `Succeeded`; the caller fills `job_state` from `result`.
    async fn notify_complete(&self, job_id: &JobId, result: &JobResult);
}

/// A no-op [`ProgressNotifier`] for use when no client push channel is available.
///
/// Used when the submitting client did not supply a `progressToken` in the MCP
/// tool call, meaning it has opted out of push notifications.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopProgressNotifier;

#[async_trait]
impl ProgressNotifier for NoopProgressNotifier {
    async fn notify_progress(&self, _event: ProgressEvent) {
        // Intentional no-op: client opted out of progress push.
    }

    async fn notify_complete(&self, _job_id: &JobId, _result: &JobResult) {
        // Intentional no-op: client opted out of completion push.
    }
}
