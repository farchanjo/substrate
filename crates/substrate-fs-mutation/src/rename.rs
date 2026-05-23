//! `fs.rename` — atomically rename or move a file or directory.
//!
//! # Async zone: A
//!
//! Uses `tokio::fs::rename` (maps to `rename(2)` on POSIX). Atomic on the
//! same filesystem. Cross-filesystem moves require a copy+remove sequence —
//! that pattern is not supported here; callers should use `fs.copy` + `fs.remove`.
//!
//! # Security: elicitation gate (ADR-0004 Layer 4)
//!
//! `fs.rename` is a destructive operation — it can silently overwrite an
//! existing destination. The handler enforces two gates:
//!
//! 1. **Dry-run gate** — `dry_run_acknowledged` must be `true` before any disk
//!    state is modified. First call with `false` returns a preview.
//! 2. **Elicitation gate** — `confirmed` must be `true`. A `false` value
//!    returns [`SubstrateError::ConfirmationRequired`] so the composition root
//!    emits an MCP elicitation request to the human operator.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::elicitation;
use crate::hints_helpers;
use crate::response::{FsMutationDeps, ToolResponse};

// ---- Request -----------------------------------------------------------------

/// Input parameters for `fs.rename`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FsRenameRequest {
    /// Source path (must exist and be within the allowlist).
    pub src: String,

    /// Destination path (parent must be within the allowlist).
    pub dst: String,

    /// When `false` (default), fail if `dst` already exists.
    #[serde(default)]
    pub overwrite: bool,

    /// Must be explicitly set to `true` before the operation executes.
    /// First call with `false` returns a dry-run preview.
    #[serde(default)]
    pub dry_run_acknowledged: bool,

    /// Explicit human-confirmation token. Returns
    /// [`SubstrateError::ConfirmationRequired`] when `false`.
    #[serde(default)]
    pub confirmed: bool,
}

// ---- Handler -----------------------------------------------------------------

/// Handles an `fs.rename` tool call.
///
/// # Errors
///
/// - [`SubstrateError::DryRunRequired`] — `dry_run_acknowledged` is `false`.
/// - [`SubstrateError::ConfirmationRequired`] — `confirmed` is `false`.
/// - [`SubstrateError::InvalidArgument`] — `dst` already exists and `overwrite`
///   is `false`.
/// - Other [`SubstrateError`] variants from jail validation or `tokio::fs::rename`.
#[instrument(skip(deps), fields(src = %req.src, dst = %req.dst))]
pub async fn handle_fs_rename(
    req: FsRenameRequest,
    deps: &FsMutationDeps,
    allowlist_root: &JailedPath,
) -> SubstrateResult<ToolResponse> {
    // Layer 1+2: jail both paths.
    let jailed_src = deps.jail.jail(allowlist_root, Path::new(&req.src))?;
    let jailed_dst = jail_dst_path(&req.dst, deps, allowlist_root)?;

    // Layer 3: dry-run gate.
    elicitation::require_dry_run_acknowledged(req.dry_run_acknowledged)?;

    // Layer 4: elicitation gate.
    elicitation::require_confirmation(req.confirmed)?;

    // Overwrite guard.
    if !req.overwrite && jailed_dst.as_path().exists() {
        return Err(SubstrateError::InvalidArgument {
            offending_field: "dst".into(),
            reason: "Destination already exists and overwrite is false.".into(),
            correlation_id: None,
        });
    }

    // Zone A: atomic rename.
    tokio::fs::rename(jailed_src.as_path(), jailed_dst.as_path())
        .await
        .map_err(|e| map_io_error(e, jailed_src.as_path()))?;

    #[cfg(feature = "fs-index")]
    crate::write_through::on_rename(&deps.index, &jailed_src, &jailed_dst);

    let content = format!("Renamed {jailed_src} → {jailed_dst}");
    let sc = serde_json::json!({
        "src": jailed_src.as_path(),
        "dst": jailed_dst.as_path(),
    });
    Ok(ToolResponse::with_hints(
        content,
        sc,
        hints_helpers::mutation_success_hints("fs.stat"),
    ))
}

// ---- Helpers -----------------------------------------------------------------

fn jail_dst_path(
    raw: &str,
    deps: &FsMutationDeps,
    root: &JailedPath,
) -> SubstrateResult<JailedPath> {
    let target = Path::new(raw);
    if target.exists() {
        return deps.jail.jail(root, target);
    }
    let parent = target
        .parent()
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "dst".into(),
            reason: "Destination path has no parent directory.".into(),
            correlation_id: None,
        })?;
    let jailed_parent = deps.jail.jail(root, parent)?;
    let file_name = target
        .file_name()
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "dst".into(),
            reason: "Destination path has no file name component.".into(),
            correlation_id: None,
        })?;
    Ok(JailedPath::new_jailed(
        jailed_parent.as_path().join(file_name),
    ))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "std::io::Error is the conventional error-mapping pattern; taking by value avoids lifetime annotation at call sites"
)]
fn map_io_error(e: std::io::Error, path: &Path) -> SubstrateError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
            path: path.display().to_string(),
            correlation_id: None,
        },
        std::io::ErrorKind::NotFound => SubstrateError::NotFound {
            resource: path.display().to_string(),
            correlation_id: None,
        },
        _ => SubstrateError::IoError {
            path: path.display().to_string(),
            correlation_id: None,
        },
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
    async fn dry_run_gate_blocks_without_acknowledgement() {
        let (dir, root, deps) = make_test_env();
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"data").expect("seed");
        let req = FsRenameRequest {
            src: src.display().to_string(),
            dst: dir.path().join("b.txt").display().to_string(),
            overwrite: false,
            dry_run_acknowledged: false,
            confirmed: true,
        };
        let err = handle_fs_rename(req, &deps, &root).await.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_DRY_RUN_REQUIRED");
        assert!(src.exists(), "source must still exist");
    }

    #[tokio::test]
    async fn elicitation_gate_blocks_without_confirmation() {
        let (dir, root, deps) = make_test_env();
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"data").expect("seed");
        let req = FsRenameRequest {
            src: src.display().to_string(),
            dst: dir.path().join("b.txt").display().to_string(),
            overwrite: false,
            dry_run_acknowledged: true,
            confirmed: false,
        };
        let err = handle_fs_rename(req, &deps, &root).await.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_CONFIRMATION_REQUIRED");
        assert!(src.exists(), "source must still exist");
    }

    #[tokio::test]
    async fn renames_file_with_both_gates_open() {
        let (dir, root, deps) = make_test_env();
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"data").expect("seed");
        let dst = dir.path().join("b.txt");
        let req = FsRenameRequest {
            src: src.display().to_string(),
            dst: dst.display().to_string(),
            overwrite: false,
            dry_run_acknowledged: true,
            confirmed: true,
        };
        handle_fs_rename(req, &deps, &root).await.expect("rename");
        assert!(!src.exists());
        assert!(dst.exists());
    }

    #[tokio::test]
    async fn rename_dst_outside_allowlist_is_rejected() {
        let (dir, root, deps) = make_test_env();
        let src = dir.path().join("file.txt");
        std::fs::write(&src, b"data").expect("seed");
        let req = FsRenameRequest {
            src: src.display().to_string(),
            dst: "/tmp/__substrate_rename_escape_test".into(),
            overwrite: true,
            dry_run_acknowledged: true,
            confirmed: true,
        };
        let err = handle_fs_rename(req, &deps, &root).await.unwrap_err();
        assert!(
            err.code() == "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"
                || err.code() == "SUBSTRATE_NOT_FOUND",
            "unexpected code: {}",
            err.code()
        );
        assert!(src.exists(), "source must still exist");
    }

    #[tokio::test]
    async fn overwrite_true_replaces_existing_dst() {
        let (dir, root, deps) = make_test_env();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&src, b"new content").expect("seed src");
        std::fs::write(&dst, b"old content").expect("seed dst");
        let req = FsRenameRequest {
            src: src.display().to_string(),
            dst: dst.display().to_string(),
            overwrite: true,
            dry_run_acknowledged: true,
            confirmed: true,
        };
        handle_fs_rename(req, &deps, &root).await.expect("rename with overwrite");
        assert!(!src.exists());
        let content = std::fs::read_to_string(&dst).expect("read dst");
        assert_eq!(content, "new content");
    }
}
