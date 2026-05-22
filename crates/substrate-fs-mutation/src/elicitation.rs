//! Elicitation gate for destructive operations per ADR-0004 (Layer 4).
//!
//! Before executing `fs.remove` or world-writable `fs.set_permissions`, the
//! handler checks whether the caller has supplied an explicit confirmation
//! token. If not, [`require_confirmation`] returns
//! [`SubstrateError::ConfirmationRequired`] so the composition root can emit
//! an MCP elicitation request to the human operator.
//!
//! The confirmation token is a simple boolean flag carried in the request
//! struct. In a future wave this will be replaced by a signed nonce exchanged
//! via the MCP elicitation form-mode flow (ADR-0013, preferred version
//! 2025-11-25).

use substrate_domain::{SubstrateError, SubstrateResult};

/// Asserts that the caller has supplied explicit confirmation for a destructive op.
///
/// Pass `confirmed = true` only after the human operator approved the
/// elicitation form rendered by the MCP host. Passing `false` returns
/// [`SubstrateError::ConfirmationRequired`], which the composition root
/// converts into an MCP elicitation request.
///
/// # Errors
///
/// - [`SubstrateError::ConfirmationRequired`] — `confirmed` is `false`.
pub const fn require_confirmation(confirmed: bool) -> SubstrateResult<()> {
    if confirmed {
        Ok(())
    } else {
        Err(SubstrateError::ConfirmationRequired {
            correlation_id: None,
        })
    }
}

/// Asserts that the caller has acknowledged the dry-run requirement.
///
/// Returns [`SubstrateError::DryRunRequired`] when `dry_run_acknowledged` is
/// `false`, prompting the caller to first issue the same request with
/// `dry_run: true` to preview the mutation.
///
/// # Errors
///
/// - [`SubstrateError::DryRunRequired`] — `dry_run_acknowledged` is `false`.
pub const fn require_dry_run_acknowledged(dry_run_acknowledged: bool) -> SubstrateResult<()> {
    if dry_run_acknowledged {
        Ok(())
    } else {
        Err(SubstrateError::DryRunRequired {
            correlation_id: None,
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use super::*;

    #[test]
    fn require_confirmation_true_passes() {
        assert!(require_confirmation(true).is_ok());
    }

    #[test]
    fn require_confirmation_false_errs() {
        let err = require_confirmation(false).unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_CONFIRMATION_REQUIRED");
    }

    #[test]
    fn require_dry_run_acknowledged_true_passes() {
        assert!(require_dry_run_acknowledged(true).is_ok());
    }

    #[test]
    fn require_dry_run_acknowledged_false_errs() {
        let err = require_dry_run_acknowledged(false).unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_DRY_RUN_REQUIRED");
    }
}
