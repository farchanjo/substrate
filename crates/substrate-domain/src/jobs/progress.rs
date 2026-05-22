//! `ProgressEvent` — push-channel payload for async job progress notifications.
//!
//! Mirrors `#ProgressEvent` in `docs/arch/schemas/job.cue`.
//! Events are throttled: suppressed unless 250 ms have elapsed since the last
//! emission OR the progress delta >= 1 percentage point per ADR-0040.
//! `sequence_number` is sourced from a per-job `AtomicU64` for dropped-event detection.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::value_objects::job_id::JobId;

/// A single progress notification emitted by a running job per ADR-0040.
///
/// Delivered via MCP 2025-11-25 `notifications/progress`. Clients MUST use
/// `sequence_number` to detect dropped or reordered events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEvent {
    /// The job ID, equal to the MCP `progressToken` per ADR-0040 triple-equality.
    pub progress_token: JobId,

    /// Completion percentage in the range `0..=100`.
    pub progress: u8,

    /// Denominator for `progress`; defaults to `100`.
    pub total: u32,

    /// Optional human-readable status note (max 120 characters per CUE schema).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Monotonically increasing per-job counter sourced from an `AtomicU64`.
    ///
    /// Gaps indicate dropped events; reordering is not expected but must be
    /// handled defensively by clients.
    pub sequence_number: u64,

    /// RFC 3339 timestamp at which this event was constructed by the worker.
    pub emitted_at: OffsetDateTime,
}
