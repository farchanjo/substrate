//! Helpers for constructing the `Hints` map returned with every process tool
//! response.
//!
//! Key groups follow ADR-0007 (tool-card keys). Only non-default keys are set;
//! `serde` `skip_serializing_if = "Option::is_none"` ensures absent keys never
//! reach the wire.

use substrate_domain::Hints;

/// Builds a `Hints` map for successful read-side process operations
/// (`proc.list`, `proc.tree`).
#[must_use]
pub fn build_read_hints(
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

/// Builds a `Hints` map for `proc.signal` responses where the operation
/// was previewed via dry-run.
#[must_use]
pub fn build_dry_run_hints(pid: u32, signal_name: &str) -> Hints {
    Hints {
        next_action_suggested: Some(format!(
            "Re-invoke proc.signal with dry_run=false and elicitation_confirmed=true to deliver {signal_name} to PID {pid}."
        )),
        confirm_destructive: Some(true),
        ..Hints::default()
    }
}

/// Builds a `Hints` map for `proc.signal` when elicitation is required.
#[must_use]
pub fn build_elicitation_hints(pid: u32, signal_name: &str) -> Hints {
    Hints {
        next_action_suggested: Some(format!(
            "Obtain explicit user confirmation, then retry proc.signal with elicitation_confirmed=true for {signal_name} on PID {pid}."
        )),
        confirm_destructive: Some(true),
        error_recovery: Some(
            "Present the elicitation form to the user and wait for confirmation token.".to_owned(),
        ),
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
