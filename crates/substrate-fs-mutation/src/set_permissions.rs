//! `fs.set_permissions` — apply a POSIX permission bitmask to a path.
//!
//! # Async zone: B
//!
//! Uses `tokio::task::spawn_blocking` wrapping `nix::sys::stat::chmod`.
//!
//! # Security: elicitation gate for world-writable targets (ADR-0004 Layer 4)
//!
//! When the requested mode includes world-writable bits (`0o002`), the handler
//! requires elicitation confirmation before proceeding. Dry-run gate is enforced
//! for all invocations per ADR-0004 Layer 3.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::elicitation;
use crate::hints_helpers;
use crate::response::{FsMutationDeps, ToolResponse};

// ---- Constants ---------------------------------------------------------------

/// POSIX world-writable bit mask.
const WORLD_WRITABLE_MASK: u32 = 0o002;

// ---- Request -----------------------------------------------------------------

/// Input parameters for `fs.set_permissions`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FsSetPermissionsRequest {
    /// Target path (must be within the allowlist).
    pub path: String,

    /// POSIX permission bitmask (e.g., `0o755` represented as decimal `493`).
    pub mode: u32,

    /// Must be explicitly set to `true` to proceed past the dry-run gate.
    #[serde(default)]
    pub dry_run_acknowledged: bool,

    /// Required when `mode` sets world-writable bits. Provides elicitation
    /// confirmation per ADR-0004 Layer 4.
    #[serde(default)]
    pub confirmed: bool,
}

// ---- Handler -----------------------------------------------------------------

/// Handles an `fs.set_permissions` tool call.
///
/// # Errors
///
/// - [`SubstrateError::DryRunRequired`] — `dry_run_acknowledged` is `false`.
/// - [`SubstrateError::ConfirmationRequired`] — world-writable mode and `confirmed` is `false`.
/// - Other [`SubstrateError`] variants from jail validation or `nix`.
#[instrument(skip(deps), fields(path = %req.path, mode = req.mode))]
pub async fn handle_fs_set_permissions(
    req: FsSetPermissionsRequest,
    deps: &FsMutationDeps,
    allowlist_root: &JailedPath,
) -> SubstrateResult<ToolResponse> {
    // Layer 1+2: allowlist + path jail.
    let jailed = deps.jail.jail(allowlist_root, Path::new(&req.path))?;

    // Layer 3: dry-run gate.
    elicitation::require_dry_run_acknowledged(req.dry_run_acknowledged)?;

    // Layer 4: elicitation gate for world-writable.
    if req.mode & WORLD_WRITABLE_MASK != 0 {
        elicitation::require_confirmation(req.confirmed)?;
    }

    // Zone B: blocking chmod via nix.
    let path = jailed.as_path().to_path_buf();
    let mode = req.mode;
    tokio::task::spawn_blocking(move || apply_chmod(&path, mode))
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error in fs.set_permissions: {e}"),
            correlation_id: None,
        })??;

    let content = format!("Permissions set on {jailed}: mode {mode:#o}");
    let sc = serde_json::json!({
        "path": jailed.as_path(),
        "mode": mode,
        "mode_octal": format!("{mode:#o}"),
    });
    Ok(ToolResponse::with_hints(
        content,
        sc,
        hints_helpers::mutation_success_hints("fs.stat"),
    ))
}

// ---- Helpers -----------------------------------------------------------------

fn apply_chmod(path: &Path, mode: u32) -> SubstrateResult<()> {
    use nix::fcntl::AT_FDCWD;
    use nix::sys::stat::{FchmodatFlags, Mode, fchmodat};

    // nix::sys::stat::Mode uses u16 internally (matching st_mode lower 12 bits).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Unix chmod mode is a 12-bit value (octal 0000–7777); the upper 20 bits of u32 are never set by callers"
    )]
    let nix_mode = Mode::from_bits_truncate(mode as u16);
    fchmodat(AT_FDCWD, path, nix_mode, FchmodatFlags::FollowSymlink).map_err(|_e| {
        SubstrateError::PermissionDenied {
            path: path.display().to_string(),
            correlation_id: None,
        }
    })
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
    use std::sync::Arc;

    use substrate_domain::{Capabilities, JailedPath, PortFactory};
    use substrate_policy::{Allowlist, PathJailFactory};
    use tempfile::TempDir;

    use super::*;
    use crate::response::FsMutationDeps;

    fn make_test_env() -> (TempDir, JailedPath, FsMutationDeps) {
        let dir = TempDir::new().expect("tempdir");
        let canonical = dir.path().canonicalize().expect("canonicalize");
        let root = JailedPath::new_jailed(canonical.clone());
        let allowlist = Allowlist::new(vec![canonical]).expect("allowlist");
        let caps = Arc::new(Capabilities::default());
        let factory = PathJailFactory::new(allowlist, false);
        let jail = factory.build(&caps);
        let deps = FsMutationDeps {
            jail,
            capabilities: caps,
        };
        (dir, root, deps)
    }

    #[tokio::test]
    async fn dry_run_gate_blocks_chmod() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("target.txt");
        std::fs::write(&f, b"data").expect("seed");
        let req = FsSetPermissionsRequest {
            path: f.display().to_string(),
            mode: 0o644,
            dry_run_acknowledged: false,
            confirmed: false,
        };
        let err = handle_fs_set_permissions(req, &deps, &root)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_DRY_RUN_REQUIRED");
    }

    #[tokio::test]
    async fn world_writable_requires_confirmation() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("target.txt");
        std::fs::write(&f, b"data").expect("seed");
        let req = FsSetPermissionsRequest {
            path: f.display().to_string(),
            mode: 0o777,
            dry_run_acknowledged: true,
            confirmed: false, // missing confirmation for world-writable
        };
        let err = handle_fs_set_permissions(req, &deps, &root)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_CONFIRMATION_REQUIRED");
    }

    #[tokio::test]
    async fn applies_non_world_writable_mode_without_confirmation() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("target.txt");
        std::fs::write(&f, b"data").expect("seed");
        let req = FsSetPermissionsRequest {
            path: f.display().to_string(),
            mode: 0o644,
            dry_run_acknowledged: true,
            confirmed: false,
        };
        handle_fs_set_permissions(req, &deps, &root)
            .await
            .expect("chmod 0o644");
    }
}
