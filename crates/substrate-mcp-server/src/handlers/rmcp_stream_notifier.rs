//! [`RmcpStreamNotifier`] — concrete [`StreamChunkObserver`] that pushes
//! subprocess stdout/stderr chunks as `notifications/progress` frames to the
//! connected MCP client over the rmcp STDIO transport per ADR-0054.
//!
//! # Design
//!
//! Reuses the late-bound peer slot owned by [`RmcpPeerNotifier`] so the same
//! `Peer<RoleServer>` instance carries both job-progress events (from
//! `InMemoryJobRegistry`) and subprocess stream chunks (from the per-job
//! dispatcher task in `SubprocessRegistry`).
//!
//! # Wire format
//!
//! rmcp 1.7's `ProgressNotificationParam` exposes only `progress_token`,
//! `progress`, `total`, and `message`. Stream-extension fields specified in
//! ADR-0054 (stream, chunk_base64, chunk_bytes, chunk_seq, byte_offset,
//! job_state) are packed as a JSON object inside `message` until rmcp adds
//! native `_meta` support for the progress notification payload. Clients parse
//! `message` as JSON to retrieve the stream chunk.
//!
//! `progress_token` carries the `JobId` per ADR-0040 triple-equality.
//!
//! # Cancellation safety
//!
//! `on_chunk` and `on_terminal` are fire-and-forget: rmcp's `notify_progress`
//! is a non-blocking send; channel-closed errors are logged and discarded
//! identical to `RmcpPeerNotifier`.

use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{NumberOrString, ProgressNotificationParam, ProgressToken};
use serde_json::json;
use substrate_domain::ports::stream_observer::StreamChunkObserver;
use substrate_domain::subprocess::state::SubprocessState;
use substrate_domain::subprocess::stream::StreamChunk;
use substrate_domain::value_objects::JobId;
use tracing::{debug, warn};

use crate::handlers::rmcp_progress_notifier::RmcpPeerNotifier;

/// Standard base64 encode (RFC 4648 §4). Mirrors the helper in
/// `subprocess_tools.rs` to avoid a runtime dep on `base64` for a small payload.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let combined = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHA[((combined >> 18) & 0x3F) as usize]);
        out.push(ALPHA[((combined >> 12) & 0x3F) as usize]);
        out.push(if chunk.len() >= 2 {
            ALPHA[((combined >> 6) & 0x3F) as usize]
        } else {
            b'='
        });
        out.push(if chunk.len() == 3 {
            ALPHA[(combined & 0x3F) as usize]
        } else {
            b'='
        });
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Pushes subprocess stdout/stderr chunks via `notifications/progress`.
///
/// Shares the late-bound peer with [`RmcpPeerNotifier`] so both observers
/// reuse a single `Peer<RoleServer>` instance after the `initialize` handshake.
pub(crate) struct RmcpStreamNotifier {
    peer_notifier: Arc<RmcpPeerNotifier>,
}

impl RmcpStreamNotifier {
    /// Constructs the stream notifier sharing the supplied peer-bound notifier.
    #[must_use]
    pub(crate) const fn new(peer_notifier: Arc<RmcpPeerNotifier>) -> Self {
        Self { peer_notifier }
    }
}

impl std::fmt::Debug for RmcpStreamNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RmcpStreamNotifier").finish_non_exhaustive()
    }
}

/// Mirrors `RmcpPeerNotifier::job_id_to_progress_token` (kept local to avoid
/// exposing it from the sibling module).
fn job_id_to_progress_token(job_id: &JobId) -> ProgressToken {
    ProgressToken(NumberOrString::String(job_id.to_string().into()))
}

#[async_trait]
impl StreamChunkObserver for RmcpStreamNotifier {
    async fn on_chunk(&self, chunk: &StreamChunk) {
        let Some(peer) = self.peer_notifier.peer_handle() else {
            debug!(
                job_id = %chunk.job_id,
                "on_chunk: no peer bound — event discarded"
            );
            return;
        };

        let chunk_base64 = base64_encode(&chunk.chunk);
        let chunk_bytes = chunk.chunk.len();
        let payload = json!({
            "stream": chunk.stream,
            "chunk_base64": chunk_base64,
            "chunk_bytes": chunk_bytes,
            "chunk_seq": chunk.seq,
            "byte_offset": chunk.byte_offset,
        });

        let params = ProgressNotificationParam {
            progress_token: job_id_to_progress_token(&chunk.job_id),
            progress: 0.0_f64,
            total: None,
            message: Some(payload.to_string()),
        };

        if let Err(e) = peer.notify_progress(params).await {
            warn!(
                job_id = %chunk.job_id,
                error = %e,
                "on_chunk: rmcp send error — stream chunk dropped"
            );
        }
    }

    async fn on_terminal(&self, job_id: &JobId, state: SubprocessState) {
        let Some(peer) = self.peer_notifier.peer_handle() else {
            debug!(
                %job_id,
                "on_terminal: no peer bound — event discarded"
            );
            return;
        };

        let payload = json!({
            "chunk_base64": "",
            "chunk_bytes": 0,
            "job_state": state,
        });

        let params = ProgressNotificationParam {
            progress_token: job_id_to_progress_token(job_id),
            progress: 100.0_f64,
            total: Some(100.0_f64),
            message: Some(payload.to_string()),
        };

        if let Err(e) = peer.notify_progress(params).await {
            warn!(
                %job_id,
                error = %e,
                "on_terminal: rmcp send error — terminal sentinel dropped"
            );
        }
    }
}
