//! `JobEntry` — in-memory aggregate root snapshot per ADR-0040.
//!
//! Mirrors `#JobEntry` in `docs/arch/schemas/job.cue`.
//! State transitions are serialised through a `parking_lot::Mutex<JobState>`
//! in the `substrate-jobs` adapter; the domain type is a plain data struct.
//! Mutation methods live in the registry adapter, not here.
//!
//! The optional `subprocess` field was added in Wave 2 Phase 2a to carry the
//! `SubprocessHandle` for Bucket E jobs (ADR-0040 §"2026-05-24 amendment",
//! ADR-0052 §"`JobEntry` with `SubprocessHandle` variant").

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::jobs::bucket::JobBucket;
use crate::jobs::state::JobState;
use crate::subprocess::handle::SubprocessHandle;
use crate::value_objects::{ClientId, CorrelationId, IdempotencyKey, JobId};

/// An immutable snapshot of a job aggregate root stored in the `JobRegistry`.
///
/// The adapter crate (`substrate-jobs`) is responsible for maintaining state
/// transitions. Domain code that receives a `JobEntry` treats it as a read-only
/// value object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobEntry {
    /// Canonical job identifier — equals `progress_token` and `correlation_id`.
    pub id: JobId,

    /// The MCP client that submitted this job.
    pub client_id: ClientId,

    /// Fully-qualified MCP tool name (e.g., `archive_tar_create`).
    pub tool: String,

    /// Static dispatch bucket assigned at submission time.
    pub bucket: JobBucket,

    /// Current position in the job state machine.
    pub state: JobState,

    /// Last-known completion percentage emitted by the worker (`0..=100`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_pct: Option<u8>,

    /// Last human-readable status note from the worker (max 120 chars).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Equals `id` per ADR-0040 triple-equality invariant.
    pub correlation_id: CorrelationId,

    /// Client-supplied deduplication token; optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<IdempotencyKey>,

    /// Timestamp when the job transitioned to `Running`.
    pub started_at: OffsetDateTime,

    /// Timestamp of the most recent state transition.
    pub updated_at: OffsetDateTime,

    /// Timestamp when the job entered a terminal state; absent while pending/running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<OffsetDateTime>,

    /// Count of progress events lost due to bounded mpsc channel backpressure.
    ///
    /// An `AuditEvent` is emitted for each drop per ADR-0040.
    pub progress_events_dropped: u64,

    /// For Bucket E jobs: the subprocess aggregate root associated with this entry.
    ///
    /// `None` for all non-subprocess job buckets (A, B, C, D). Populated by the
    /// `substrate-subprocess` adapter immediately after a successful `spawn` and
    /// updated by the cascade kill chain per ADR-0053.
    ///
    /// References: ADR-0040 §"2026-05-24 amendment — Bucket E", ADR-0052.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub subprocess: Option<SubprocessHandle>,
}
