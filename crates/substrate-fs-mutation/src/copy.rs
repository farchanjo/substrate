//! `fs.copy` — copy a file from one jailed path to another.
//!
//! # Async zone: B
//!
//! Uses `tokio::fs::copy` (which wraps `sendfile(2)` / `copyfile(2)` on
//! Linux / macOS). The copy is transactional: data is written to
//! `<dst>.tmp.<uuid7>` first, then atomically renamed to `dst`.
//!
//! # Security
//!
//! Both `src` and `dst` are validated through the path jail. An `overwrite`
//! flag guards against silent clobbers (default: `false`).
//!
//! # Dry-run
//!
//! When `dry_run = true`, returns a preview with both paths and the expected
//! byte count (from `src` `metadata`) without touching disk.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers;
use crate::preflight;
use crate::response::{FsMutationDeps, ToolResponse};
use crate::tmp_path::TmpPath;

// ---- Request -----------------------------------------------------------------

/// Input parameters for `fs.copy`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FsCopyRequest {
    /// Source file path (must exist and be within the allowlist).
    pub src: String,

    /// Destination file path (parent must be within the allowlist).
    pub dst: String,

    /// When `false` (default), fail if `dst` already exists.
    #[serde(default)]
    pub overwrite: bool,

    /// When `true` (default), return a preview without modifying disk.
    #[serde(default = "default_true")]
    pub dry_run: bool,
}

const fn default_true() -> bool {
    true
}

// ---- Handler -----------------------------------------------------------------

/// Handles an `fs.copy` tool call.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, preflight checks,
/// or `tokio::fs` operations.
#[instrument(skip(deps), fields(src = %req.src, dst = %req.dst, dry_run = req.dry_run))]
pub async fn handle_fs_copy(
    req: FsCopyRequest,
    deps: &FsMutationDeps,
    allowlist_root: &JailedPath,
) -> SubstrateResult<ToolResponse> {
    // Layer 1+2: jail both paths.
    let jailed_src = deps.jail.jail(allowlist_root, Path::new(&req.src))?;
    let jailed_dst = jail_dst_path(&req.dst, deps, allowlist_root)?;

    // Overwrite guard.
    if !req.overwrite && jailed_dst.as_path().exists() {
        return Err(SubstrateError::InvalidArgument {
            offending_field: "dst".into(),
            reason: "Destination already exists and overwrite is false.".into(),
            correlation_id: None,
        });
    }

    // Source size for dry-run preview and disk-space preflight.
    let src_len = tokio::fs::metadata(jailed_src.as_path())
        .await
        .map_or(0, |m| m.len());

    if req.dry_run {
        return Ok(dry_run_response(&jailed_src, &jailed_dst, src_len));
    }

    // Preflight disk-space.
    let dst_parent = jailed_dst.as_path().parent().unwrap_or_else(|| Path::new("."));
    preflight::check_disk_space(dst_parent, src_len).await?;

    // Zone A transactional copy: tokio::fs::copy then atomic rename.
    let tmp = TmpPath::new_for(jailed_dst.as_path());
    tokio::fs::copy(jailed_src.as_path(), tmp.tmp_path())
        .await
        .map_err(|e| map_io_error(e, tmp.tmp_path()))?;
    tmp.commit()
        .await
        .map_err(|e| map_io_error(e, jailed_dst.as_path()))?;

    #[cfg(feature = "fs-index")]
    crate::write_through::on_upsert(&deps.index, &jailed_dst);

    let content = format!("Copied {jailed_src} → {jailed_dst} ({src_len} bytes)");
    let sc = serde_json::json!({
        "src": jailed_src.as_path(),
        "dst": jailed_dst.as_path(),
        "bytes_copied": src_len,
    });
    Ok(ToolResponse::with_hints(
        content,
        sc,
        hints_helpers::mutation_success_hints("fs.stat"),
    ))
}

// ---- Helpers -----------------------------------------------------------------

/// Validates the destination path, permitting a non-existent file as long as
/// its parent directory is within the allowlist.
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

fn dry_run_response(src: &JailedPath, dst: &JailedPath, src_len: u64) -> ToolResponse {
    let content = format!("Dry run: would copy {src} → {dst} ({src_len} bytes)");
    let sc = serde_json::json!({
        "src": src.as_path(),
        "dst": dst.as_path(),
        "expected_bytes": src_len,
        "dry_run": true,
    });
    ToolResponse::with_hints(content, sc, hints_helpers::dry_run_hints("fs.copy"))
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
        };
        (dir, root, deps)
    }

    #[tokio::test]
    async fn copies_file_successfully() {
        let (dir, root, deps) = make_test_env();
        let src = dir.path().join("src.txt");
        std::fs::write(&src, b"content").expect("seed");
        let dst = dir.path().join("dst.txt");

        let req = FsCopyRequest {
            src: src.display().to_string(),
            dst: dst.display().to_string(),
            overwrite: false,
            dry_run: false,
        };
        handle_fs_copy(req, &deps, &root).await.expect("copy");
        assert_eq!(std::fs::read(&dst).expect("read dst"), b"content");
        assert!(src.exists(), "source must still exist");
    }

    #[tokio::test]
    async fn dry_run_does_not_copy() {
        let (dir, root, deps) = make_test_env();
        let src = dir.path().join("src.txt");
        std::fs::write(&src, b"content").expect("seed");
        let dst = dir.path().join("dst.txt");

        let req = FsCopyRequest {
            src: src.display().to_string(),
            dst: dst.display().to_string(),
            overwrite: false,
            dry_run: true,
        };
        let resp = handle_fs_copy(req, &deps, &root).await.expect("dry run");
        assert_eq!(resp.hints.confirm_destructive, Some(true));
        assert!(!dst.exists());
    }

    #[tokio::test]
    async fn overwrite_false_rejects_existing_dst() {
        let (dir, root, deps) = make_test_env();
        let src = dir.path().join("src.txt");
        std::fs::write(&src, b"new").expect("seed src");
        let dst = dir.path().join("dst.txt");
        std::fs::write(&dst, b"old").expect("seed dst");

        let req = FsCopyRequest {
            src: src.display().to_string(),
            dst: dst.display().to_string(),
            overwrite: false,
            dry_run: false,
        };
        let err = handle_fs_copy(req, &deps, &root).await.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
        assert_eq!(std::fs::read(&dst).expect("read dst"), b"old");
    }
}
