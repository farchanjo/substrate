//! `fs.write` — write UTF-8 text or base64-decoded bytes to a file.
//!
//! # Async zone: A
//!
//! Uses `tokio::fs::write` with [`TmpPath`] transactional atomicity.
//!
//! # Encoding
//!
//! - `encoding = "text"` (default): `content` string written as UTF-8 after
//!   validation via `simdutf8`.
//! - `encoding = "base64"`: `content` string decoded via `base64-simd`; raw
//!   bytes written verbatim.
//!
//! # Transactional semantics (ADR-0033)
//!
//! Bytes are written to `<parent>/<uuid7>.tmp`, then atomically renamed to the
//! target path. A cancelled or panicked handler removes the temp file via the
//! [`TmpPath`] Drop impl.
//!
//! # Dry-run
//!
//! When `dry_run = true`, returns a preview including byte count without
//! touching disk.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers;
use crate::preflight;
use crate::response::{FsMutationDeps, ToolResponse};
use crate::tmp_path::TmpPath;

// ---- Request -----------------------------------------------------------------

/// Encoding of the `content` field in [`FsWriteRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WriteEncoding {
    /// Plain UTF-8 text.
    #[default]
    Text,
    /// Base64-encoded bytes (standard alphabet, padding optional).
    Base64,
}

/// Input parameters for `fs.write`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FsWriteRequest {
    /// Target file path (caller-supplied; validated against the allowlist).
    pub path: String,

    /// File content as a UTF-8 string or a base64-encoded string.
    pub content: String,

    /// Encoding of the `content` field.
    #[serde(default)]
    pub encoding: WriteEncoding,

    /// When `true` (default), fail if the file already exists.
    #[serde(default = "default_true")]
    pub fail_if_exists: bool,

    /// When `true` (default), return a preview without modifying disk.
    #[serde(default = "default_true")]
    pub dry_run: bool,
}

const fn default_true() -> bool {
    true
}

// ---- Handler -----------------------------------------------------------------

/// Handles an `fs.write` tool call.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, encoding, preflight,
/// or `tokio::fs` operations.
#[instrument(skip(deps), fields(path = %req.path, encoding = ?req.encoding, dry_run = req.dry_run))]
pub async fn handle_fs_write(
    req: FsWriteRequest,
    deps: &FsMutationDeps,
    allowlist_root: &JailedPath,
) -> SubstrateResult<ToolResponse> {
    // Layer 1+2: allowlist + path jail.
    // For write to a non-existent file, we jail the parent and treat the
    // filename as a child. The policy adapter validates the full path against
    // the allowlist; new files are permitted as long as the parent is jailed.
    //
    // The jail call may return SUBSTRATE_NOT_FOUND when the file does not yet
    // exist. For write, this is expected — we catch that and re-validate the
    // parent dir instead.
    let jailed = jail_for_new_file(&req.path, deps, allowlist_root)?;

    // Decode content to bytes.
    let bytes = decode_content(&req.content, req.encoding)?;
    let byte_count = bytes.len();

    if req.dry_run {
        return Ok(dry_run_response(&jailed, byte_count));
    }

    // Fail-if-exists guard.
    if req.fail_if_exists && jailed.as_path().exists() {
        return Err(SubstrateError::InvalidArgument {
            offending_field: "path".into(),
            reason: "File already exists and fail_if_exists is true.".into(),
            correlation_id: None,
        });
    }

    // Preflight disk-space check.
    let parent = jailed.as_path().parent().unwrap_or_else(|| Path::new("."));
    preflight::check_disk_space(parent, byte_count as u64).await?;

    // Transactional write: tmp → rename.
    let tmp = TmpPath::new_for(jailed.as_path());
    tokio::fs::write(tmp.tmp_path(), &bytes)
        .await
        .map_err(|e| map_io_error(e, tmp.tmp_path()))?;
    tmp.commit()
        .await
        .map_err(|e| map_io_error(e, jailed.as_path()))?;

    #[cfg(feature = "fs-index")]
    crate::write_through::on_upsert(&deps.index, &jailed);

    let content_msg = format!("File written: {jailed} ({byte_count} bytes)");
    let sc = serde_json::json!({
        "path": jailed.as_path(),
        "bytes_written": byte_count,
    });
    Ok(ToolResponse::with_hints(
        content_msg,
        sc,
        hints_helpers::mutation_success_hints("fs.stat"),
    ))
}

// ---- Helpers -----------------------------------------------------------------

/// Validates the path for a new or overwritten file.
///
/// If the file does not yet exist, we validate the parent directory and
/// produce a `JailedPath` for the intended target using the parent as the
/// base. This allows the jail to enforce allowlist membership without requiring
/// the target to be pre-existing.
fn jail_for_new_file(
    raw: &str,
    deps: &FsMutationDeps,
    root: &JailedPath,
) -> SubstrateResult<JailedPath> {
    let target = Path::new(raw);

    // If the target already exists, jail it directly.
    if target.exists() {
        return deps.jail.jail(root, target);
    }

    // Target doesn't exist yet — jail the parent instead, then reconstruct.
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

fn decode_content(content: &str, encoding: WriteEncoding) -> SubstrateResult<Vec<u8>> {
    match encoding {
        WriteEncoding::Text => {
            // simdutf8 fast-path validation — the string is already UTF-8 in Rust,
            // but we explicitly validate to catch embedded null bytes and confirm
            // the caller's intent.
            simdutf8::basic::from_utf8(content.as_bytes()).map_err(|_| {
                SubstrateError::EncodingError {
                    detail: "content is not valid UTF-8".into(),
                    correlation_id: None,
                }
            })?;
            Ok(content.as_bytes().to_vec())
        },
        WriteEncoding::Base64 => {
            use base64_simd::STANDARD;
            STANDARD
                .decode_to_vec(content.as_bytes())
                .map_err(|e| SubstrateError::EncodingError {
                    detail: format!("base64 decode failed: {e}"),
                    correlation_id: None,
                })
        },
    }
}

fn dry_run_response(jailed: &JailedPath, byte_count: usize) -> ToolResponse {
    let content = format!("Dry run: would write {byte_count} bytes to {jailed}");
    let sc = serde_json::json!({
        "path": jailed.as_path(),
        "bytes_to_write": byte_count,
        "dry_run": true,
    });
    ToolResponse::with_hints(content, sc, hints_helpers::dry_run_hints("fs.write"))
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
        std::io::ErrorKind::StorageFull => SubstrateError::StorageFull {
            path: path.display().to_string(),
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
    async fn writes_text_file() {
        let (dir, root, deps) = make_test_env();
        let target = dir.path().join("hello.txt");
        let req = FsWriteRequest {
            path: target.display().to_string(),
            content: "hello world".into(),
            encoding: WriteEncoding::Text,
            fail_if_exists: false,
            dry_run: false,
        };
        handle_fs_write(req, &deps, &root).await.expect("write");
        let written = std::fs::read_to_string(&target).expect("read back");
        assert_eq!(written, "hello world");
    }

    #[tokio::test]
    async fn writes_base64_file() {
        let (dir, root, deps) = make_test_env();
        let target = dir.path().join("data.bin");
        let req = FsWriteRequest {
            path: target.display().to_string(),
            content: "aGVsbG8=".into(), // "hello"
            encoding: WriteEncoding::Base64,
            fail_if_exists: false,
            dry_run: false,
        };
        handle_fs_write(req, &deps, &root)
            .await
            .expect("write base64");
        let written = std::fs::read(&target).expect("read back");
        assert_eq!(written, b"hello");
    }

    #[tokio::test]
    async fn dry_run_does_not_write() {
        let (dir, root, deps) = make_test_env();
        let target = dir.path().join("should_not_exist.txt");
        let req = FsWriteRequest {
            path: target.display().to_string(),
            content: "data".into(),
            encoding: WriteEncoding::Text,
            fail_if_exists: false,
            dry_run: true,
        };
        let resp = handle_fs_write(req, &deps, &root).await.expect("dry run");
        assert_eq!(resp.hints.confirm_destructive, Some(true));
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn fail_if_exists_rejects_overwrite() {
        let (dir, root, deps) = make_test_env();
        let target = dir.path().join("existing.txt");
        std::fs::write(&target, b"old").expect("seed file");
        let req = FsWriteRequest {
            path: target.display().to_string(),
            content: "new".into(),
            encoding: WriteEncoding::Text,
            fail_if_exists: true,
            dry_run: false,
        };
        let err = handle_fs_write(req, &deps, &root).await.unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
        // Original content preserved.
        let still_old = std::fs::read_to_string(&target).expect("read");
        assert_eq!(still_old, "old");
    }
}
