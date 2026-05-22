//! `fs.symlink` — create a symbolic link.
//!
//! # Async zone: A
//!
//! Uses `tokio::fs::symlink` (maps to `symlink(2)` on POSIX).
//!
//! # Security
//!
//! Both `link_path` (the new symlink entry) and `link_target` (what it points
//! to) are validated against the allowlist via the path jail. This prevents
//! symlink-based escape attacks where an agent creates a symlink inside the
//! allowlist pointing to a path outside it (ADR-0004, ADR-0035).
//!
//! # Platform
//!
//! `tokio::fs::symlink` is a POSIX-only function. On Windows this crate is not
//! supported. The workspace `rust-version = "1.95"` and `edition = "2024"`
//! targets are POSIX-only for this BC.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers;
use crate::response::{FsMutationDeps, ToolResponse};

// ---- Request -----------------------------------------------------------------

/// Input parameters for `fs.symlink`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FsSymlinkRequest {
    /// The path where the symlink entry is created (parent must be in allowlist).
    pub link_path: String,

    /// The path the symlink points to (must be within the allowlist).
    pub link_target: String,

    /// When `true` (default), return a preview without modifying disk.
    #[serde(default = "default_true")]
    pub dry_run: bool,
}

const fn default_true() -> bool {
    true
}

// ---- Handler -----------------------------------------------------------------

/// Handles an `fs.symlink` tool call.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation or `tokio::fs`.
#[instrument(skip(deps), fields(link_path = %req.link_path, link_target = %req.link_target))]
pub async fn handle_fs_symlink(
    req: FsSymlinkRequest,
    deps: &FsMutationDeps,
    allowlist_root: &JailedPath,
) -> SubstrateResult<ToolResponse> {
    // Layer 1+2: validate both paths.
    // link_target must exist and be within the allowlist.
    let jailed_target = deps
        .jail
        .jail(allowlist_root, Path::new(&req.link_target))?;

    // link_path must have its parent within the allowlist (the link itself
    // does not yet exist).
    let jailed_link = jail_new_path(&req.link_path, deps, allowlist_root)?;

    if req.dry_run {
        return Ok(dry_run_response(&jailed_link, &jailed_target));
    }

    // Zone A: symlink creation.
    tokio::fs::symlink(jailed_target.as_path(), jailed_link.as_path())
        .await
        .map_err(|e| map_io_error(e, jailed_link.as_path()))?;

    #[cfg(feature = "fs-index")]
    crate::write_through::on_upsert(&deps.index, &jailed_link);

    let content = format!("Symlink created: {jailed_link} → {jailed_target}");
    let sc = serde_json::json!({
        "link_path": jailed_link.as_path(),
        "link_target": jailed_target.as_path(),
    });
    Ok(ToolResponse::with_hints(
        content,
        sc,
        hints_helpers::mutation_success_hints("fs.stat"),
    ))
}

// ---- Helpers -----------------------------------------------------------------

/// Validates the path for a new (not-yet-existing) symlink entry.
fn jail_new_path(
    raw: &str,
    deps: &FsMutationDeps,
    root: &JailedPath,
) -> SubstrateResult<JailedPath> {
    let target = Path::new(raw);
    if target.exists() {
        return Err(SubstrateError::InvalidArgument {
            offending_field: "link_path".into(),
            reason: "link_path already exists.".into(),
            correlation_id: None,
        });
    }
    let parent = target
        .parent()
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "link_path".into(),
            reason: "link_path has no parent directory.".into(),
            correlation_id: None,
        })?;
    let jailed_parent = deps.jail.jail(root, parent)?;
    let file_name = target
        .file_name()
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "link_path".into(),
            reason: "link_path has no file name component.".into(),
            correlation_id: None,
        })?;
    Ok(JailedPath::new_jailed(
        jailed_parent.as_path().join(file_name),
    ))
}

fn dry_run_response(link: &JailedPath, target: &JailedPath) -> ToolResponse {
    let content = format!("Dry run: would create symlink {link} → {target}");
    let sc = serde_json::json!({
        "link_path": link.as_path(),
        "link_target": target.as_path(),
        "dry_run": true,
    });
    ToolResponse::with_hints(content, sc, hints_helpers::dry_run_hints("fs.symlink"))
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
        std::io::ErrorKind::AlreadyExists => SubstrateError::InvalidArgument {
            offending_field: "link_path".into(),
            reason: "Link path already exists.".into(),
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
    async fn creates_symlink() {
        let (dir, root, deps) = make_test_env();
        let target_file = dir.path().join("real.txt");
        std::fs::write(&target_file, b"data").expect("seed");
        let link = dir.path().join("link.txt");

        let req = FsSymlinkRequest {
            link_path: link.display().to_string(),
            link_target: target_file.display().to_string(),
            dry_run: false,
        };
        handle_fs_symlink(req, &deps, &root).await.expect("symlink");
        assert!(link.exists());
        assert!(link.is_symlink());
    }

    #[tokio::test]
    async fn dry_run_does_not_create_symlink() {
        let (dir, root, deps) = make_test_env();
        let target_file = dir.path().join("real.txt");
        std::fs::write(&target_file, b"data").expect("seed");
        let link = dir.path().join("link.txt");

        let req = FsSymlinkRequest {
            link_path: link.display().to_string(),
            link_target: target_file.display().to_string(),
            dry_run: true,
        };
        let resp = handle_fs_symlink(req, &deps, &root).await.expect("dry run");
        assert_eq!(resp.hints.confirm_destructive, Some(true));
        assert!(!link.exists());
    }

    #[tokio::test]
    async fn rejects_target_outside_allowlist() {
        let (dir, root, deps) = make_test_env();
        let link = dir.path().join("link.txt");
        let req = FsSymlinkRequest {
            link_path: link.display().to_string(),
            link_target: "/etc/passwd".into(),
            dry_run: false,
        };
        let err = handle_fs_symlink(req, &deps, &root).await.unwrap_err();
        assert!(
            err.code() == "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST" || err.code() == "SUBSTRATE_NOT_FOUND",
            "unexpected code: {}",
            err.code()
        );
    }
}
