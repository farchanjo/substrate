//! [`RmcpPeerNotifier`] — concrete [`ProgressNotifier`] that pushes
//! `notifications/progress` events to the MCP client over the rmcp STDIO
//! transport per ADR-0040.
//!
//! # Design
//!
//! The peer becomes available only after the `initialize` handshake. The
//! notifier therefore holds an [`ArcSwapOption`] around `Peer<RoleServer>`.
//! Before `set_peer` is called (pre-initialize) all events are silently
//! discarded, identical to `NoopProgressNotifier`.
//!
//! After `set_peer` is called by the `initialize` handler the peer is
//! published into the slot and subsequent notifications are forwarded over the
//! transport.
//!
//! # Cancellation safety
//!
//! Both `notify_progress` and `notify_complete` are cancel-safe:
//! - `ArcSwapOption::load_full` is lock-free and never blocks.
//! - `Peer::notify_progress` is an rmcp fire-and-forget send. If the channel
//!   has been closed (client disconnected) the `Err` is logged and discarded.
//! - Neither method modifies shared state after the first `.await`.

use std::{fmt, sync::Arc};

use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use rmcp::{
    Peer, RoleServer,
    model::{NumberOrString, ProgressNotificationParam, ProgressToken},
};
use substrate_domain::{
    jobs::progress::ProgressEvent, ports::job_registry::JobResult, value_objects::JobId,
};
use substrate_jobs::ProgressNotifier;
use tracing::{debug, warn};

/// Pushes `notifications/progress` frames to the connected MCP client.
///
/// Holds a late-bound `Peer<RoleServer>` in an [`ArcSwapOption`].  Discards
/// events silently until [`set_peer`] is called from the `initialize` handler.
///
/// [`set_peer`]: RmcpPeerNotifier::set_peer
pub(crate) struct RmcpPeerNotifier {
    peer: ArcSwapOption<Peer<RoleServer>>,
}

impl RmcpPeerNotifier {
    /// Creates a new notifier with no peer bound (pre-initialize state).
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            peer: ArcSwapOption::empty(),
        }
    }

    /// Binds the `Peer<RoleServer>` obtained from the `initialize` handler context.
    ///
    /// Safe to call more than once (e.g. if a client reconnects and re-initializes);
    /// the old peer is atomically replaced.
    pub(crate) fn set_peer(&self, peer: Peer<RoleServer>) {
        self.peer.store(Some(Arc::new(peer)));
    }
}

impl Default for RmcpPeerNotifier {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for RmcpPeerNotifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bound = self.peer.load().is_some();
        f.debug_struct("RmcpPeerNotifier")
            .field("peer_bound", &bound)
            .finish()
    }
}

/// Converts a substrate [`JobId`] into an rmcp [`ProgressToken`] per the
/// ADR-0040 triple-equality invariant (`job_id` == `progress_token` == MCP `request_id`).
fn job_id_to_progress_token(job_id: &JobId) -> ProgressToken {
    ProgressToken(NumberOrString::String(job_id.to_string().into()))
}

#[async_trait]
impl ProgressNotifier for RmcpPeerNotifier {
    /// Pushes a progress update to the connected client.
    ///
    /// Silently drops the event when:
    /// - No peer is bound yet (pre-initialize).
    /// - The rmcp send channel is closed (client disconnected).
    async fn notify_progress(&self, event: ProgressEvent) {
        let Some(peer) = self.peer.load_full() else {
            debug!(
                job_id = %event.progress_token,
                "notify_progress: no peer bound — event discarded"
            );
            return;
        };

        let token = job_id_to_progress_token(&event.progress_token);
        let params = ProgressNotificationParam {
            progress_token: token,
            progress: f64::from(event.progress),
            total: Some(f64::from(event.total)),
            message: event.message.clone(),
        };

        if let Err(e) = peer.notify_progress(params).await {
            warn!(
                job_id = %event.progress_token,
                error = %e,
                "notify_progress: rmcp send error — event dropped"
            );
        }
    }

    /// Pushes a terminal (100% or failed) notification to the connected client.
    ///
    /// Maps the job result to a descriptive message and emits a final
    /// `notifications/progress` frame with `progress = 100` (or `0` for
    /// failed/cancelled/timed-out).
    async fn notify_complete(&self, job_id: &JobId, result: &JobResult) {
        let Some(peer) = self.peer.load_full() else {
            debug!(
                %job_id,
                "notify_complete: no peer bound — event discarded"
            );
            return;
        };

        let token = job_id_to_progress_token(job_id);
        let (progress, message) = match result {
            JobResult::Succeeded(_) => (100.0_f64, "completed".to_owned()),
            JobResult::Failed(e) => (0.0_f64, format!("failed: {e}")),
            JobResult::Cancelled => (0.0_f64, "cancelled".to_owned()),
            JobResult::TimedOut => (0.0_f64, "timed out".to_owned()),
        };

        let params = ProgressNotificationParam {
            progress_token: token,
            progress,
            total: Some(100.0),
            message: Some(message),
        };

        if let Err(e) = peer.notify_progress(params).await {
            warn!(
                %job_id,
                error = %e,
                "notify_complete: rmcp send error — completion dropped"
            );
        }
    }
}
