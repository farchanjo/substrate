//! Helpers for constructing the `Hints` map for system-info tool responses.
//!
//! All system-info tools are Zone A (sync inline) read-only operations per
//! ADR-0040. No `job_id` is ever set. `confirm_destructive` is always `false`.

use substrate_domain::Hints;

/// Builds a `Hints` map for a successful system-info read.
///
/// `next_action` and `alternative` are optional follow-up guidance strings.
#[must_use]
pub fn build_info_hints(
    next_action: Option<&'static str>,
    alternative: Option<&'static str>,
) -> Hints {
    Hints {
        next_action_suggested: next_action.map(ToOwned::to_owned),
        alternative_tool: alternative.map(ToOwned::to_owned),
        confirm_destructive: Some(false),
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
