//! Handler for the `fs.stat` tool — Zone B (`spawn_blocking` + `StatPort`).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: retrieve metadata for a single path: size, kind, owner, timestamps
//! DOES: lstat semantics (does not follow symlinks)
//! ARGS: path (string) — file or directory to stat
//! RETURNS: {path, size_bytes, is_dir, is_file, is_symlink, modified_at, accessed_at}
//! NEXT: fs.read, fs.hash
//! AVOID: calling fs.stat in a loop for directory entries → use fs.read_dir
//! ```
//!
//! # Zone classification
//!
//! `StatPort::stat` is a synchronous call. The handler dispatches it via
//! `tokio::task::spawn_blocking` (Zone B per ADR-0003).

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{JailedPath, PathJailPort, StatPort, SubstrateError, SubstrateResult};

use crate::hint_helpers::build_hints;
use crate::response::{FsQueryDeps, ToolResponse};

/// Inbound request for `fs.stat`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct FsStatRequest {
    /// The path to stat; must be within an allowlist root.
    pub path: String,
}

/// Handler for `fs.stat`.
///
/// Zone B: `StatPort::stat` is synchronous; dispatched via `spawn_blocking`.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation or `StatPort::stat`.
#[instrument(skip(deps, _cancel), fields(path = %req.path))]
pub async fn handle_fs_stat(
    req: FsStatRequest,
    deps: &FsQueryDeps,
    _cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    // Jail the path.
    let raw = std::path::Path::new(&req.path).to_path_buf();
    let jail: Arc<dyn PathJailPort> = Arc::clone(&deps.jail);
    let raw_clone = raw.clone();
    let jailed: JailedPath = tokio::task::spawn_blocking(move || {
        jail.jail(&JailedPath::new_jailed(raw_clone.clone()), &raw_clone)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: None,
    })??;

    // Zone B: dispatch synchronous StatPort call.
    let statter: Arc<dyn StatPort> = Arc::clone(&deps.statter);
    let file_stat = tokio::task::spawn_blocking(move || statter.stat(&jailed))
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error: {e}"),
            correlation_id: None,
        })??;

    let hints = build_hints(
        Some("fs.read"),
        Some("fs.hash"),
        Some("Use fs.read_dir for bulk metadata of directory children"),
        &deps.capabilities,
        false,
    );

    let kind = if file_stat.is_symlink {
        "symlink"
    } else if file_stat.is_dir {
        "directory"
    } else if file_stat.is_file {
        "file"
    } else {
        "special"
    };

    let content = format!(
        "USE: retrieve single-path metadata\nDOES: stat of {kind} at '{}'\nNEXT: fs.read, fs.hash\nAVOID: stat in a loop → use fs.read_dir",
        req.path
    );

    let structured_content = json!({
        "tool": "fs.stat",
        "path": req.path,
        "size_bytes": file_stat.size_bytes,
        "is_dir": file_stat.is_dir,
        "is_file": file_stat.is_file,
        "is_symlink": file_stat.is_symlink,
        "kind": kind,
        "modified_at": file_stat.modified_at.to_string(),
        "accessed_at": file_stat.accessed_at.to_string(),
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

// ---- Tests ------------------------------------------------------------------

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
    use std::sync::Arc;
    use tempfile::TempDir;

    struct NoopJail;
    impl substrate_domain::PathJailPort for NoopJail {
        fn jail(&self, _: &JailedPath, raw: &std::path::Path) -> SubstrateResult<JailedPath> {
            Ok(JailedPath::new_jailed(raw.to_path_buf()))
        }
    }

    fn make_deps() -> FsQueryDeps {
        FsQueryDeps {
            jail: Arc::new(NoopJail),
            walker: Arc::new(crate::walker::legacy::LegacyWalker::new()),
            hasher: Arc::new(crate::hash_factory::Blake3Hasher::new()),
            statter: Arc::new(crate::stat_factory::PortableStatter::new()),
            capabilities: Arc::new(substrate_domain::Capabilities::default()),
        }
    }

    #[tokio::test]
    async fn stat_regular_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("file.txt");
        std::fs::write(&path, b"hello").unwrap();
        let deps = make_deps();
        let resp = handle_fs_stat(
            FsStatRequest {
                path: path.to_string_lossy().into_owned(),
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(resp.structured_content["is_file"], true);
        assert_eq!(resp.structured_content["is_dir"], false);
        assert_eq!(resp.structured_content["size_bytes"], 5u64);
    }

    #[tokio::test]
    async fn stat_directory() {
        let tmp = TempDir::new().unwrap();
        let deps = make_deps();
        let resp = handle_fs_stat(
            FsStatRequest {
                path: tmp.path().to_string_lossy().into_owned(),
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(resp.structured_content["is_dir"], true);
    }

    #[tokio::test]
    async fn stat_missing_returns_not_found() {
        let deps = make_deps();
        let err = handle_fs_stat(
            FsStatRequest {
                path: "/tmp/__substrate_no_such_path_xyz".to_owned(),
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SubstrateError::NotFound { .. }));
    }
}
