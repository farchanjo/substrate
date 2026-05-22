//! `fs.remove` — delete a file, empty directory, or directory tree.
//!
//! # Async zone: B
//!
//! Uses `tokio::task::spawn_blocking` because `std::fs::remove_file`,
//! `std::fs::remove_dir`, and `std::fs::remove_dir_all` are synchronous and
//! potentially blocking on NFS.
//!
//! # Security: elicitation gate (ADR-0004 Layer 4)
//!
//! `fs.remove` is an irreversible, destructive operation. The handler enforces
//! two gates:
//!
//! 1. **Dry-run gate** — `dry_run_acknowledged` must be `true` before any disk
//!    state is modified. First-call with `false` returns a preview.
//! 2. **Elicitation gate** — `confirmed` must be `true`. A `false` value
//!    returns [`SubstrateError::ConfirmationRequired`] so the composition root
//!    emits an MCP elicitation request to the human operator.
//!
//! # Recursive remove
//!
//! When `recursive: true`, the handler calls `std::fs::remove_dir_all` on the
//! jailed path (wrapped in `spawn_blocking`). Per-entry path-jail validation
//! is NOT required for the recursive walk because `remove_dir_all` operates
//! entirely within the jailed root — the jail was already verified at the
//! top-level path before any I/O begins.
//!
//! When `recursive: false` (default), a non-empty directory returns
//! `SUBSTRATE_INVALID_ARGUMENT`.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::elicitation;
use crate::hints_helpers;
use crate::response::{FsMutationDeps, ToolResponse};

// ---- Request -----------------------------------------------------------------

/// Input parameters for `fs.remove`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FsRemoveRequest {
    /// Path of the file or directory to remove (within the allowlist).
    pub path: String,

    /// Must be explicitly set to `true` to proceed past the dry-run gate.
    #[serde(default)]
    pub dry_run_acknowledged: bool,

    /// Explicit human-confirmation token. Returns
    /// [`SubstrateError::ConfirmationRequired`] when `false`.
    #[serde(default)]
    pub confirmed: bool,

    /// When `true`, recursively removes the directory tree via
    /// `std::fs::remove_dir_all`. Defaults to `false`; a non-empty directory
    /// with `recursive: false` returns `SUBSTRATE_INVALID_ARGUMENT`.
    #[serde(default)]
    pub recursive: bool,
}

// ---- Handler -----------------------------------------------------------------

/// Handles an `fs.remove` tool call.
///
/// # Errors
///
/// - [`SubstrateError::DryRunRequired`] — `dry_run_acknowledged` is `false`.
/// - [`SubstrateError::ConfirmationRequired`] — `confirmed` is `false`.
/// - [`SubstrateError::InvalidArgument`] — path is a non-empty directory and
///   `recursive` is `false`.
/// - Other [`SubstrateError`] variants from jail validation or I/O.
#[instrument(skip(deps), fields(path = %req.path, recursive = req.recursive))]
pub async fn handle_fs_remove(
    req: FsRemoveRequest,
    deps: &FsMutationDeps,
    allowlist_root: &JailedPath,
) -> SubstrateResult<ToolResponse> {
    // Layer 1+2: allowlist + path jail.
    let jailed = deps.jail.jail(allowlist_root, Path::new(&req.path))?;

    // Layer 3: dry-run gate.
    elicitation::require_dry_run_acknowledged(req.dry_run_acknowledged)?;

    // Layer 4: elicitation gate.
    elicitation::require_confirmation(req.confirmed)?;

    let is_dir = jailed.as_path().is_dir();

    if is_dir && req.recursive {
        // Recursive removal: remove the entire directory tree.
        // The jailed path was already validated above; `remove_dir_all` cannot
        // escape the root because it only descends into subdirectories of `jailed`.
        let jailed_path = jailed.as_path().to_path_buf();
        tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&jailed_path))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking join error in fs.remove (recursive): {e}"),
                correlation_id: None,
            })?
            .map_err(|e| map_io_error(e, jailed.as_path()))?;
    } else if is_dir {
        // Non-recursive: directory must be empty.
        let path = jailed.as_path().to_path_buf();
        let is_empty = tokio::task::spawn_blocking(move || {
            std::fs::read_dir(&path).is_ok_and(|mut d| d.next().is_none())
        })
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking error in fs.remove: {e}"),
            correlation_id: None,
        })?;

        if !is_empty {
            return Err(SubstrateError::InvalidArgument {
                offending_field: "path".into(),
                reason: "Directory is not empty. Set recursive=true to remove the entire tree, \
                         or empty the directory first."
                    .into(),
                correlation_id: None,
            });
        }

        let jailed_path = jailed.as_path().to_path_buf();
        tokio::task::spawn_blocking(move || std::fs::remove_dir(&jailed_path))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking join error in fs.remove: {e}"),
                correlation_id: None,
            })?
            .map_err(|e| map_io_error(e, jailed.as_path()))?;
    } else {
        // File (or symlink) removal.
        let jailed_path = jailed.as_path().to_path_buf();
        tokio::task::spawn_blocking(move || std::fs::remove_file(&jailed_path))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking join error in fs.remove: {e}"),
                correlation_id: None,
            })?
            .map_err(|e| map_io_error(e, jailed.as_path()))?;
    }

    #[cfg(feature = "fs-index")]
    crate::write_through::on_remove(&deps.index, &jailed);

    let content = format!("Removed: {jailed}");
    let sc = serde_json::json!({ "path": jailed.as_path(), "removed": true });
    Ok(ToolResponse::with_hints(
        content,
        sc,
        hints_helpers::destructive_success_hints(),
    ))
}

// ---- Helpers -----------------------------------------------------------------

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
        };
        (dir, root, deps)
    }

    #[tokio::test]
    async fn dry_run_gate_blocks() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("victim.txt");
        std::fs::write(&f, b"data").expect("seed");
        let req = FsRemoveRequest {
            path: f.display().to_string(),
            dry_run_acknowledged: false,
            confirmed: true,
            recursive: false,
        };
        let err = handle_fs_remove(req, &deps, &root).await.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_DRY_RUN_REQUIRED");
        assert!(f.exists());
    }

    #[tokio::test]
    async fn elicitation_gate_blocks() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("victim.txt");
        std::fs::write(&f, b"data").expect("seed");
        let req = FsRemoveRequest {
            path: f.display().to_string(),
            dry_run_acknowledged: true,
            confirmed: false,
            recursive: false,
        };
        let err = handle_fs_remove(req, &deps, &root).await.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_CONFIRMATION_REQUIRED");
        assert!(f.exists());
    }

    #[tokio::test]
    async fn removes_file_with_all_gates_open() {
        let (dir, root, deps) = make_test_env();
        let f = dir.path().join("victim.txt");
        std::fs::write(&f, b"data").expect("seed");
        let req = FsRemoveRequest {
            path: f.display().to_string(),
            dry_run_acknowledged: true,
            confirmed: true,
            recursive: false,
        };
        handle_fs_remove(req, &deps, &root).await.expect("remove");
        assert!(!f.exists());
    }

    #[tokio::test]
    async fn rejects_non_empty_directory_without_recursive() {
        let (dir, root, deps) = make_test_env();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).expect("mkdir");
        std::fs::write(sub.join("file.txt"), b"data").expect("seed");
        let req = FsRemoveRequest {
            path: sub.display().to_string(),
            dry_run_acknowledged: true,
            confirmed: true,
            recursive: false,
        };
        let err = handle_fs_remove(req, &deps, &root).await.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
        assert!(sub.exists());
    }

    #[tokio::test]
    async fn removes_non_empty_directory_with_recursive_true() {
        let (dir, root, deps) = make_test_env();
        let sub = dir.path().join("tree");
        let nested = sub.join("nested").join("deep");
        std::fs::create_dir_all(&nested).expect("mkdir -p");
        std::fs::write(nested.join("file.txt"), b"data").expect("seed");
        std::fs::write(sub.join("top.txt"), b"top").expect("seed top");

        let req = FsRemoveRequest {
            path: sub.display().to_string(),
            dry_run_acknowledged: true,
            confirmed: true,
            recursive: true,
        };
        handle_fs_remove(req, &deps, &root)
            .await
            .expect("recursive remove must succeed");
        assert!(!sub.exists(), "directory tree must be fully removed");
    }

    #[tokio::test]
    async fn removes_empty_directory_without_recursive() {
        let (dir, root, deps) = make_test_env();
        let sub = dir.path().join("empty_dir");
        std::fs::create_dir(&sub).expect("mkdir");

        let req = FsRemoveRequest {
            path: sub.display().to_string(),
            dry_run_acknowledged: true,
            confirmed: true,
            recursive: false,
        };
        handle_fs_remove(req, &deps, &root)
            .await
            .expect("empty dir remove must succeed");
        assert!(!sub.exists());
    }
}
