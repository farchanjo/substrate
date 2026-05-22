//! Hints-map builder helpers for fs-mutation tool handlers.
//!
//! Each helper constructs a [`Hints`] value appropriate for the given mutation
//! outcome. Handlers call these at the end of the success path before building
//! the [`ToolResponse`].

use substrate_domain::Hints;

/// Builds a `Hints` value for a successful non-destructive mutation
/// (mkdir, write, copy, rename, symlink, touch).
#[must_use]
pub fn mutation_success_hints(next_action: impl Into<String>) -> Hints {
    Hints {
        next_action_suggested: Some(next_action.into()),
        ..Hints::default()
    }
}

/// Builds a `Hints` value for a dry-run preview response.
///
/// Sets `confirm_destructive = true` to signal the MCP host that an
/// elicitation form should be rendered before re-submitting.
#[must_use]
pub fn dry_run_hints(tool_name: impl Into<String>) -> Hints {
    let tool = tool_name.into();
    Hints {
        confirm_destructive: Some(true),
        next_action_suggested: Some(format!(
            "Review the dry-run preview, then re-call {tool} with dry_run=false and confirmed=true."
        )),
        ..Hints::default()
    }
}

/// Builds a `Hints` value for a destructive operation that completed
/// successfully after elicitation confirmation.
#[must_use]
pub fn destructive_success_hints() -> Hints {
    Hints {
        next_action_suggested: Some("Call fs.read_dir or fs.stat to verify the result.".into()),
        ..Hints::default()
    }
}
