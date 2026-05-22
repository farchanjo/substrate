//! Helpers for constructing the `Hints` map returned with every tool response.
//!
//! The key groups follow ADR-0007 (tool-card keys) and ADR-0042/ADR-0043
//! (capability diagnostic keys).  Only non-default keys are set; `serde`
//! `skip_serializing_if = "Option::is_none"` ensures absent keys never
//! reach the wire.

use substrate_domain::{Capabilities, Hints};

/// Builds a `Hints` map for a successful read-side fs operation.
///
/// `simd_tier_used` and `walker_tier_used` are diagnostic-only; clients MUST
/// NOT branch on them (ADR-0042, ADR-0043).
#[must_use]
pub fn build_hints(
    next_action: Option<&'static str>,
    alternative: Option<&'static str>,
    error_recovery: Option<&'static str>,
    caps: &Capabilities,
    record_walker_tier: bool,
) -> Hints {
    Hints {
        next_action_suggested: next_action.map(ToOwned::to_owned),
        alternative_tool: alternative.map(ToOwned::to_owned),
        error_recovery: error_recovery.map(ToOwned::to_owned),
        confirm_destructive: Some(false),
        simd_tier_used: Some(format!("{:?}", caps.simd_tier).to_lowercase()),
        walker_tier_used: if record_walker_tier {
            Some(format!("{:?}", caps.walker_tier).to_lowercase())
        } else {
            None
        },
        ..Hints::default()
    }
}

/// Builds a minimal `Hints` map for error responses.
#[must_use]
pub fn build_error_hints(recovery: &str) -> Hints {
    Hints {
        error_recovery: Some(recovery.to_owned()),
        confirm_destructive: Some(false),
        ..Hints::default()
    }
}
