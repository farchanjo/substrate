//! `fs.set_permissions` — apply a POSIX permission bitmask to a path.
//!
//! # Async zone: B
//!
//! Uses `tokio::task::spawn_blocking` wrapping `nix::sys::stat::chmod`.
//!
//! # Security: elicitation gate for privileged-mode targets (ADR-0004 Layer 4)
//!
//! When the requested mode includes any of the following bits, the handler
//! requires elicitation confirmation before proceeding:
//!
//! - **setuid** (`0o4000`) — executing user acquires file owner identity.
//! - **setgid** (`0o2000`) — executing user acquires file group identity.
//! - **world-writable** (`0o002`) — any user may overwrite the file.
//!
//! Setting `0o4755` on a binary inside the allowlist without confirmation
//! would create an unconfirmed privilege-escalation path.
//!
//! Dry-run gate is enforced for all invocations per ADR-0004 Layer 3.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::elicitation;
use crate::hints_helpers;
use crate::response::{FsMutationDeps, ToolResponse};

// ---- Constants ---------------------------------------------------------------

/// Mask of POSIX mode bits that require elicitation confirmation before being
/// applied:
///
/// - `0o4000` — setuid: process runs with file-owner privileges.
/// - `0o2000` — setgid: process runs with file-group privileges.
/// - `0o002`  — world-writable: any user may overwrite the file.
const ELICITATION_MASK: u32 = 0o4000 | 0o2000 | 0o002;

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

    /// Required when `mode` sets setuid (`0o4000`), setgid (`0o2000`), or
    /// world-writable (`0o002`) bits. Provides elicitation confirmation per
    /// ADR-0004 Layer 4.
    #[serde(default)]
    pub confirmed: bool,
}

// ---- Handler -----------------------------------------------------------------

/// Handles an `fs.set_permissions` tool call.
///
/// # Errors
///
/// - [`SubstrateError::DryRunRequired`] — `dry_run_acknowledged` is `false`.
/// - [`SubstrateError::ConfirmationRequired`] — `mode` includes setuid
///   (`0o4000`), setgid (`0o2000`), or world-writable (`0o002`) bits and
///   `confirmed` is `false`.
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

    // Layer 4: elicitation gate — setuid, setgid, or world-writable bits.
    if req.mode & ELICITATION_MASK != 0 {
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
            #[cfg(feature = "fs-index")]
            index: substrate_fs_index::FsIndexFactory::new().build(&Capabilities::default()),
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

    /// Verifies that `fchmodat` with `FollowSymlink` updates the target file's
    /// mode, not the symlink's mode (symlinks have no independent mode on POSIX).
    /// After `chmod` on a symlink, the target's mode must be updated.
    #[tokio::test]
    async fn set_permissions_on_symlink_affects_target_via_fchmodat() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, root, deps) = make_test_env();
        let target = dir.path().join("real.txt");
        let link   = dir.path().join("link.txt");
        std::fs::write(&target, b"data").expect("seed target");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        let req = FsSetPermissionsRequest {
            path: link.display().to_string(),
            mode: 0o600,
            dry_run_acknowledged: true,
            confirmed: false,
        };
        handle_fs_set_permissions(req, &deps, &root)
            .await
            .expect("chmod on symlink via fchmodat must succeed");

        // Stat the TARGET (not the link) — fchmodat(FollowSymlink) follows links.
        let target_meta = std::fs::metadata(&target).expect("stat target");
        let mode = target_meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "target file mode must be 0o600 after chmod via symlink");

        // lstat the LINK itself — symlink mode is fixed at 0o777 on macOS/Linux.
        let link_lstat = std::fs::symlink_metadata(&link).expect("lstat link");
        assert!(link_lstat.file_type().is_symlink(), "link must still be a symlink");
    }

    #[tokio::test]
    async fn setuid_bit_requires_confirmation() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("target.txt");
        std::fs::write(&f, b"data").expect("seed");
        let req = FsSetPermissionsRequest {
            path: f.display().to_string(),
            mode: 0o4755, // setuid + rwxr-xr-x
            dry_run_acknowledged: true,
            confirmed: false,
        };
        let err = handle_fs_set_permissions(req, &deps, &root)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_CONFIRMATION_REQUIRED");
    }

    #[tokio::test]
    async fn setgid_bit_requires_confirmation() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("target.txt");
        std::fs::write(&f, b"data").expect("seed");
        let req = FsSetPermissionsRequest {
            path: f.display().to_string(),
            mode: 0o2755, // setgid + rwxr-xr-x
            dry_run_acknowledged: true,
            confirmed: false,
        };
        let err = handle_fs_set_permissions(req, &deps, &root)
            .await
            .unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_CONFIRMATION_REQUIRED");
    }

    #[tokio::test]
    async fn setuid_passes_with_confirmation() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("target.txt");
        std::fs::write(&f, b"data").expect("seed");
        let req = FsSetPermissionsRequest {
            path: f.display().to_string(),
            mode: 0o4755,
            dry_run_acknowledged: true,
            confirmed: true,
        };
        handle_fs_set_permissions(req, &deps, &root)
            .await
            .expect("chmod 0o4755 with confirmation");
    }

    #[tokio::test]
    async fn rejects_path_outside_allowlist() {
        let (_dir, root, deps) = make_test_env();
        let req = FsSetPermissionsRequest {
            path: "/etc/passwd".into(),
            mode: 0o644,
            dry_run_acknowledged: true,
            confirmed: false,
        };
        let err = handle_fs_set_permissions(req, &deps, &root)
            .await
            .unwrap_err();
        assert!(
            err.code() == "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"
                || err.code() == "SUBSTRATE_NOT_FOUND"
                || err.code() == "SUBSTRATE_PERMISSION_DENIED",
            "unexpected code: {}",
            err.code()
        );
    }
}
