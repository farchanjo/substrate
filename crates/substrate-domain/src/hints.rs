//! `Hints` — the structured hints map returned alongside every tool result.
//!
//! Extends the base 5 keys from ADR-0007 with 6 job keys (ADR-0040) and
//! 2 capability diagnostic keys (ADR-0042 / ADR-0043). All fields are `Option`
//! so absent keys are never serialised.

use serde::{Deserialize, Serialize};

/// Structured guidance hints included in every tool's `structuredContent`.
///
/// Agents use these hints for follow-up action selection, quota awareness,
/// and error recovery without parsing free-text responses.
///
/// # Key groups
///
/// - **Tool-card keys** (ADR-0007): `next_action_suggested`, `alternative_tool`,
///   `confirm_destructive`, `quota_status`, `error_recovery`.
/// - **Job keys** (ADR-0040): `job_id`, `job_state`, `job_progress_pct`,
///   `polling_endpoint`, `estimated_completion_ms`, `sequence_number`.
/// - **Capability diagnostic keys** (ADR-0042 / ADR-0043): `simd_tier_used`,
///   `walker_tier_used`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Hints {
    // ---- Tool-card keys (ADR-0007) ------------------------------------------
    /// Suggested follow-up tool or action for the agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_action_suggested: Option<String>,

    /// An alternative tool to consider when this tool is unavailable or fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alternative_tool: Option<String>,

    /// Set to `true` when the tool requires explicit user confirmation before
    /// proceeding with a destructive operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm_destructive: Option<bool>,

    /// Machine-readable quota status string (e.g., `"4/16 jobs active"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_status: Option<String>,

    /// Actionable error-recovery hint, potentially overriding the generic
    /// `recovery_hint` from the error catalog for context-specific guidance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_recovery: Option<String>,

    // ---- Job keys (ADR-0040) ------------------------------------------------
    /// `UUIDv7` (Crockford base32) of the created or reused async job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,

    /// Current `JobState` value of the dispatched async job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_state: Option<String>,

    /// Completion percentage `0..=100`; `None` for terminal or not-yet-started jobs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_progress_pct: Option<u8>,

    /// The control-plane tool to poll for job status or result.
    /// One of `"job.status"` or `"job.result"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polling_endpoint: Option<String>,

    /// Best-effort estimate of remaining wall-clock time in milliseconds.
    /// `None` when the estimate is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_completion_ms: Option<u64>,

    /// Last known `sequence_number` for this job's progress stream.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,

    // ---- Capability diagnostic keys (ADR-0042 / ADR-0043) -------------------
    /// The SIMD tier that accelerated the critical path for this invocation.
    /// One of the `#SimdTier` string values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simd_tier_used: Option<String>,

    /// The directory-walker tier selected for this invocation.
    /// One of the `#WalkerTier` string values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub walker_tier_used: Option<String>,
}
