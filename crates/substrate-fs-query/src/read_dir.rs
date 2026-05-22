//! Handler for the `fs.read_dir` tool — Zone A (`tokio::fs::read_dir`).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: list immediate children of a directory with kind, size, and mtime
//! DOES: single-level directory listing with optional pagination
//! ARGS: path (string) — directory to list;
//!       page_size (u32, 100) — entries per page, max 5000;
//!       page_cursor (string, null) — pagination token
//! RETURNS: {entries:[{name,path,is_dir,size_bytes?,mtime?}], next_cursor?}
//! NEXT: fs.stat, fs.read
//! AVOID: repeated fs.read_dir for deep traversal → use fs.find
//! ```
//!
//! # Zone classification
//!
//! `tokio::fs::read_dir` is async-native (Zone A). Per-entry metadata is
//! fetched with `tokio::fs::symlink_metadata` (also Zone A).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{JailedPath, PathJailPort, SubstrateError, SubstrateResult};

use crate::hint_helpers::build_hints;
use crate::response::{FsQueryDeps, ToolResponse};

/// Maximum page size for `fs.read_dir`.
const MAX_PAGE_SIZE: u32 = 5_000;

/// Default page size for `fs.read_dir`.
const DEFAULT_PAGE_SIZE: u32 = 100;

/// Inbound request for `fs.read_dir`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct FsReadDirRequest {
    /// The directory to list; must be within an allowlist root.
    pub path: String,

    /// Maximum number of entries per page.
    #[serde(default = "default_page_size")]
    pub page_size: u32,

    /// Opaque cursor from a previous response; `None` fetches from the start.
    pub page_cursor: Option<String>,
}

const fn default_page_size() -> u32 {
    DEFAULT_PAGE_SIZE
}

/// A single directory entry returned by `fs.read_dir`.
#[derive(Debug, Clone, Serialize)]
pub struct DirEntryInfo {
    /// File name component (no parent path).
    pub name: String,
    /// Full jailed path.
    pub path: String,
    /// `true` when this entry is a directory.
    pub is_dir: bool,
    /// `true` when this entry is a symbolic link.
    pub is_symlink: bool,
    /// Size in bytes for regular files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Last modification time as RFC3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
}

/// Handler for `fs.read_dir`.
///
/// Zone A: `tokio::fs::read_dir` with per-entry `symlink_metadata`.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, cursor decoding,
/// or `tokio::fs::read_dir` I/O.
#[instrument(skip(deps, _cancel), fields(path = %req.path))]
pub async fn handle_fs_read_dir(
    req: FsReadDirRequest,
    deps: &FsQueryDeps,
    _cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    let page_size = req.page_size.clamp(1, MAX_PAGE_SIZE);
    let skip_count: usize = if let Some(ref cursor_str) = req.page_cursor {
        decode_cursor(cursor_str)?
    } else {
        0
    };

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

    // Zone A: async read_dir.
    let mut read_dir = tokio::fs::read_dir(jailed.as_path())
        .await
        .map_err(|e| map_io_err(e, &req.path))?;

    // Collect all entries (async iterator).
    let mut all_entries: Vec<DirEntryInfo> = Vec::new();
    let target = skip_count + page_size as usize + 1;

    loop {
        let entry = match read_dir.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(e) => return Err(map_io_err(e, &req.path)),
        };

        let name = entry.file_name().to_string_lossy().into_owned();
        let entry_path = entry.path();

        // Fetch symlink-aware metadata.
        let (is_dir, is_symlink, size_bytes, modified_at) = tokio::fs::symlink_metadata(
            &entry_path,
        )
        .await
        .map_or((false, false, None, None), |meta| {
            let is_sym = meta.is_symlink();
            let is_dir = meta.is_dir();
            let size = if meta.is_file() {
                Some(meta.len())
            } else {
                None
            };
            let mtime = meta.modified().ok().map(|t| {
                use std::time::UNIX_EPOCH;
                let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                format_unix_secs(secs)
            });
            (is_dir, is_sym, size, mtime)
        });

        all_entries.push(DirEntryInfo {
            name,
            path: entry_path.to_string_lossy().into_owned(),
            is_dir,
            is_symlink,
            size_bytes,
            modified_at,
        });

        if all_entries.len() >= target {
            break;
        }
    }

    let has_more = all_entries.len() > skip_count + page_size as usize;
    let page: Vec<DirEntryInfo> = all_entries
        .into_iter()
        .skip(skip_count)
        .take(page_size as usize)
        .collect();

    let next_cursor = if has_more {
        Some(encode_cursor(skip_count + page_size as usize))
    } else {
        None
    };

    let hints = build_hints(
        Some("fs.stat"),
        Some("fs.read"),
        Some("Use fs.find for deep traversal instead of recursive fs.read_dir"),
        &deps.capabilities,
        false,
    );

    let count = page.len();
    let content = format!(
        "USE: list directory children\nDOES: returned {count} entries\nNEXT: fs.stat, fs.read\nAVOID: recursive read_dir → use fs.find"
    );

    let structured_content = json!({
        "tool": "fs.read_dir",
        "path": req.path,
        "entries": page,
        "next_cursor": next_cursor,
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

fn encode_cursor(offset: usize) -> String {
    base64_simd::STANDARD.encode_to_string(offset.to_le_bytes().as_ref())
}

fn decode_cursor(cursor: &str) -> SubstrateResult<usize> {
    let bytes = base64_simd::STANDARD
        .decode_to_vec(cursor.as_bytes())
        .map_err(|_| SubstrateError::InvalidArgument {
            offending_field: "page_cursor".to_owned(),
            reason: "malformed cursor (invalid base64)".to_owned(),
            correlation_id: None,
        })?;
    let arr: [u8; 8] = bytes
        .try_into()
        .map_err(|_| SubstrateError::InvalidArgument {
            offending_field: "page_cursor".to_owned(),
            reason: "malformed cursor (wrong length)".to_owned(),
            correlation_id: None,
        })?;
    Ok(usize::from_le_bytes(arr))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "std::io::Error is the conventional error-mapping pattern; taking by value avoids lifetime annotation at call sites"
)]
fn map_io_err(e: std::io::Error, path: &str) -> SubstrateError {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::NotFound => SubstrateError::NotFound {
            resource: path.to_owned(),
            correlation_id: None,
        },
        ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
            path: path.to_owned(),
            correlation_id: None,
        },
        _ => SubstrateError::IoError {
            path: path.to_owned(),
            correlation_id: None,
        },
    }
}

fn format_unix_secs(secs: u64) -> String {
    use time::format_description::well_known::Rfc3339;
    #[expect(
        clippy::cast_possible_wrap,
        reason = "unix timestamps in the valid range fit in i64"
    )]
    let ts = time::OffsetDateTime::from_unix_timestamp(secs as i64)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    ts.format(&Rfc3339).unwrap_or_else(|_| secs.to_string())
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
    async fn read_dir_empty() {
        let tmp = TempDir::new().unwrap();
        let deps = make_deps();
        let req = FsReadDirRequest {
            path: tmp.path().to_string_lossy().into_owned(),
            page_size: 100,
            page_cursor: None,
        };
        let resp = handle_fs_read_dir(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        let entries = resp.structured_content["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[tokio::test]
    async fn read_dir_lists_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"b").unwrap();
        let deps = make_deps();
        let req = FsReadDirRequest {
            path: tmp.path().to_string_lossy().into_owned(),
            page_size: 100,
            page_cursor: None,
        };
        let resp = handle_fs_read_dir(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        let entries = resp.structured_content["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn read_dir_not_found() {
        let deps = make_deps();
        let req = FsReadDirRequest {
            path: "/tmp/__substrate_no_dir_xyz".to_owned(),
            page_size: 100,
            page_cursor: None,
        };
        let err = handle_fs_read_dir(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, SubstrateError::NotFound { .. }));
    }

    #[tokio::test]
    async fn pagination_cursor_advances() {
        let tmp = TempDir::new().unwrap();
        for i in 0..5u8 {
            std::fs::write(tmp.path().join(format!("f{i}.txt")), [i]).unwrap();
        }
        let deps = make_deps();
        let req = FsReadDirRequest {
            path: tmp.path().to_string_lossy().into_owned(),
            page_size: 2,
            page_cursor: None,
        };
        let resp = handle_fs_read_dir(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            resp.structured_content["entries"].as_array().unwrap().len(),
            2
        );
        let cursor = resp.structured_content["next_cursor"].as_str();
        assert!(
            cursor.is_some(),
            "expected next_cursor for 5 entries paged at 2"
        );
    }
}
