//! `fs.mkdir` — create a directory (and optionally its parents).
//!
//! # Async zone: A
//!
//! Uses `tokio::fs::create_dir_all` directly on the async executor.
//! No blocking syscalls; kernel metadata updates are fast.
//!
//! # Security
//!
//! The target path is validated through [`PathJailPort`](substrate_domain::PathJailPort)
//! before any filesystem call is made (ADR-0004 layers 1 + 2).
//!
//! # Dry-run
//!
//! When `dry_run = true` (the default), the handler returns a preview without
//! touching disk. The preview reports the path that would be created.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers;
use crate::response::{FsMutationDeps, ToolResponse};

// ---- Jail helper -------------------------------------------------------------

/// Validates a path that may not yet exist.
///
/// When the target does not exist, the kernel jail rejects it with
/// `SUBSTRATE_NOT_FOUND` because `canonicalize` cannot resolve a missing path.
/// This helper jails the *parent* directory (which must exist and be within the
/// allowlist) and then reconstructs the full target path.
fn jail_for_new_path(
    raw: &str,
    deps: &FsMutationDeps,
    root: &JailedPath,
) -> SubstrateResult<JailedPath> {
    let target = Path::new(raw);
    // If the target already exists (e.g. `parents = true` on an existing tree),
    // the standard jail path works and resolves symlinks safely.
    if target.exists() {
        return deps.jail.jail(root, target);
    }
    let parent = target
        .parent()
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "path".into(),
            reason: "Path has no parent directory.".into(),
            correlation_id: None,
        })?;
    let jailed_parent = deps.jail.jail(root, parent)?;
    let file_name = target
        .file_name()
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "path".into(),
            reason: "Path has no file name component.".into(),
            correlation_id: None,
        })?;
    Ok(JailedPath::new_jailed(
        jailed_parent.as_path().join(file_name),
    ))
}

// ---- Request -----------------------------------------------------------------

/// Input parameters for `fs.mkdir`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FsMkdirRequest {
    /// Target directory path (caller-supplied; validated against the allowlist).
    pub path: String,

    /// When `true` (default), create intermediate parent directories if absent.
    #[serde(default = "default_true")]
    pub parents: bool,

    /// When `true` (default), return a preview without modifying disk.
    #[serde(default = "default_true")]
    pub dry_run: bool,
}

const fn default_true() -> bool {
    true
}

// ---- Handler -----------------------------------------------------------------

/// Handles an `fs.mkdir` tool call.
///
/// Validates the path against the allowlist + path jail, then either returns a
/// dry-run preview or creates the directory tree.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation or `tokio::fs`.
#[instrument(skip(deps), fields(path = %req.path, dry_run = req.dry_run))]
pub async fn handle_fs_mkdir(
    req: FsMkdirRequest,
    deps: &FsMutationDeps,
    allowlist_root: &JailedPath,
) -> SubstrateResult<ToolResponse> {
    // Layer 1+2: allowlist + path jail.
    // The target directory may not yet exist, so we use the parent-jail
    // strategy for new paths (jails the parent and reconstructs the full path).
    let jailed = jail_for_new_path(&req.path, deps, allowlist_root)?;

    if req.dry_run {
        return Ok(dry_run_response(&jailed));
    }

    // Zone A: async-native directory creation.
    if req.parents {
        tokio::fs::create_dir_all(jailed.as_path()).await
    } else {
        tokio::fs::create_dir(jailed.as_path()).await
    }
    .map_err(|e| map_io_error(e, jailed.as_path()))?;

    #[cfg(feature = "fs-index")]
    crate::write_through::on_upsert(&deps.index, &jailed);

    let content = format!("Directory created: {jailed}");
    let sc = serde_json::json!({ "path": jailed.as_path(), "created": true });
    Ok(ToolResponse::with_hints(
        content,
        sc,
        hints_helpers::mutation_success_hints("fs.read_dir"),
    ))
}

// ---- Helpers -----------------------------------------------------------------

fn dry_run_response(jailed: &JailedPath) -> ToolResponse {
    let content = format!("Dry run: would create directory {jailed}");
    let sc = serde_json::json!({ "path": jailed.as_path(), "dry_run": true });
    ToolResponse::with_hints(content, sc, hints_helpers::dry_run_hints("fs.mkdir"))
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
            offending_field: "path".into(),
            reason: "Directory already exists.".into(),
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
    async fn dry_run_does_not_create_directory() {
        let (dir, root, deps) = make_test_env();
        let target = dir.path().join("sub");
        let req = FsMkdirRequest {
            path: target.display().to_string(),
            parents: true,
            dry_run: true,
        };
        let resp = handle_fs_mkdir(req, &deps, &root).await.expect("dry run");
        assert_eq!(resp.hints.confirm_destructive, Some(true));
        assert!(!target.exists(), "dir must not exist after dry run");
    }

    #[tokio::test]
    async fn creates_directory_when_not_dry_run() {
        let (dir, root, deps) = make_test_env();
        let target = dir.path().join("new_dir");
        let req = FsMkdirRequest {
            path: target.display().to_string(),
            parents: true,
            dry_run: false,
        };
        handle_fs_mkdir(req, &deps, &root).await.expect("mkdir");
        assert!(target.is_dir(), "directory must be created");
    }

    #[tokio::test]
    async fn rejects_path_outside_allowlist() {
        let (_dir, root, deps) = make_test_env();
        let req = FsMkdirRequest {
            path: "/outside/jail".into(),
            parents: false,
            dry_run: false,
        };
        let err = handle_fs_mkdir(req, &deps, &root).await.unwrap_err();
        assert!(
            err.code() == "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST" || err.code() == "SUBSTRATE_NOT_FOUND",
            "unexpected code: {}",
            err.code()
        );
    }
}
