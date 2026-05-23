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

use crate::elicitation;
use crate::hints_helpers;
use crate::response::{FsMutationDeps, ToolResponse};

// ---- Jail helper -------------------------------------------------------------

/// Validates a path that may not yet exist.
///
/// When the target does not exist the kernel jail rejects it with
/// `SUBSTRATE_NOT_FOUND` because `canonicalize` cannot resolve a missing path.
/// This helper walks up the ancestor chain to find the deepest existing ancestor,
/// jails that ancestor, and then reconstructs the full target path by appending
/// the non-existent suffix. This is needed for `fs.mkdir` with `parents = true`
/// when multiple intermediate directories (e.g. `src/new_module`) also do not
/// exist yet — peeling only one parent level is insufficient in those cases.
///
/// Security: only the existing ancestor is jailed (canonicalized + allowlist
/// checked). The non-existent suffix is appended raw; `..` components are
/// rejected by the dispatcher-level `pre_validate_field_for_traversal` guard
/// before this function is reached (ADR-0035).
fn jail_for_new_path(
    raw: &str,
    deps: &FsMutationDeps,
    root: &JailedPath,
) -> SubstrateResult<JailedPath> {
    use std::path::PathBuf;

    let target = Path::new(raw);

    // If the target already exists (e.g. `parents = true` on an existing tree),
    // the standard jail path works and resolves symlinks safely.
    if target.exists() {
        return deps.jail.jail(root, target);
    }

    // Walk up ancestors to find the deepest one that exists on disk.
    // Build the suffix (relative path from that ancestor to the target) as we go.
    let mut suffix: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cursor: &Path = target;

    let existing_ancestor = loop {
        // Push the current node's last component onto the suffix stack.
        if let Some(name) = cursor.file_name() {
            suffix.push(name);
        }

        let parent = match cursor.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => {
                // Reached the filesystem root without finding an existing ancestor.
                // Let the jail handle the original path (returns NOT_FOUND or
                // PATH_OUTSIDE_ALLOWLIST).
                break None;
            },
        };

        if parent.exists() {
            break Some(parent);
        }

        cursor = parent;
    };

    let Some(ancestor) = existing_ancestor else {
        // Fallback: let the jail reject naturally.
        return deps.jail.jail(root, target);
    };

    // Jail the existing ancestor (canonicalization + allowlist check).
    let jailed_ancestor = deps.jail.jail(root, ancestor)?;

    // Reconstruct: jailed_ancestor + suffix components in top-down order.
    // `suffix` was built bottom-up, so reverse before appending.
    suffix.reverse();
    let mut full: PathBuf = jailed_ancestor.as_path().to_path_buf();
    for comp in &suffix {
        full.push(comp);
    }

    Ok(JailedPath::new_jailed(full))
}

// ---- Request -----------------------------------------------------------------

/// Input parameters for `fs.mkdir`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsMkdirRequest {
    /// Target directory path (caller-supplied; validated against the allowlist).
    pub path: String,

    /// When `true` (default), create intermediate parent directories if absent.
    #[serde(default = "default_true")]
    pub parents: bool,

    /// When `true` (default), return a preview without modifying disk.
    #[serde(default = "default_true")]
    pub dry_run: bool,

    /// Explicit elicitation confirmation gate required when `dry_run=false`.
    /// Returns [`SubstrateError::DryRunRequired`] when `false` and `dry_run=false`.
    /// Defaults to `false` so that callers must opt in explicitly.
    #[serde(default)]
    pub elicitation_confirmed: bool,
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

    // Elicitation gate: when proceeding past dry-run, require explicit confirmation.
    // `elicitation_confirmed=false` + `dry_run=false` → DRY_RUN_REQUIRED so that
    // callers are guided to review the dry-run plan before committing.
    elicitation::require_dry_run_acknowledged(req.elicitation_confirmed)?;

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
            elicitation_confirmed: false,
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
            elicitation_confirmed: true,
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
            elicitation_confirmed: false,
        };
        let err = handle_fs_mkdir(req, &deps, &root).await.unwrap_err();
        assert!(
            err.code() == "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST" || err.code() == "SUBSTRATE_NOT_FOUND",
            "unexpected code: {}",
            err.code()
        );
    }
}
