//! `StateTransitionObserver` — push-channel for `SubprocessState` lifecycle events.
//!
//! Per ADR-0056 §"Observable Port: `StateTransitionObserver`": a separate sibling of
//! `StreamChunkObserver` (data plane) carrying control-plane state events.
//! Implementations may forward to MCP `notifications/progress` (terminal sentinel),
//! audit log, or metrics counters.

use async_trait::async_trait;

use crate::subprocess::state::SubprocessState;
use crate::value_objects::JobId;

/// Observer notified on every `SubprocessState` transition per ADR-0056.
///
/// Implementations MUST be `Send + Sync` and MUST NOT block; the supervisor
/// task calls `on_state_change` fire-and-forget from an async context.
#[async_trait]
pub trait StateTransitionObserver: Send + Sync + std::fmt::Debug {
    /// Called by the supervisor for every state transition.
    ///
    /// Fire-and-forget; MUST NOT block. Callers drop the future without awaiting
    /// a result when the observer is non-critical (e.g. metrics counters).
    async fn on_state_change(&self, job_id: &JobId, old: SubprocessState, new: SubprocessState);
}

/// No-op implementation for unit tests and contexts that require an observer
/// but do not need to act on state events.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopStateTransitionObserver;

#[async_trait]
impl StateTransitionObserver for NoopStateTransitionObserver {
    async fn on_state_change(
        &self,
        _job_id: &JobId,
        _old: SubprocessState,
        _new: SubprocessState,
    ) {
    }
}
