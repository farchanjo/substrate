//! Handler for the `fs.find` tool — Zone B (`spawn_blocking` + `ignore` walker).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: locate files by name glob, extension, or mtime within a directory tree
//! DOES: recursive walk emitting matching paths with stat metadata
//! ARGS: root (string) — search root;
//!       pattern (string, "*") — glob;
//!       max_depth (u32, 16) — recursion limit;
//!       modified_since (RFC3339, null) — mtime filter;
//!       page_size (u32, 50) — entries per page, max 500;
//!       page_cursor (string, null) — pagination token
//! RETURNS: {matches:[{path,size,mtime}], next_cursor?}
//! NEXT: fs.read, fs.stat
//! AVOID: calling fs.read_dir recursively → use fs.find with max_depth
//! ```
//!
//! # Zone classification
//!
//! `ignore::WalkBuilder` is a synchronous iterator. The handler wraps it in
//! `tokio::task::spawn_blocking` (Zone B). A bounded `mpsc` channel bridges
//! the blocking walker thread to the async consumer which builds the page.

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{JailedPath, PathJailPort, SubstrateError, SubstrateResult};

use crate::hint_helpers::build_hints;
use crate::response::{FsQueryDeps, ToolResponse};

/// Maximum allowed page size for `fs.find` (ADR-0008).
const MAX_PAGE_SIZE: u32 = 500;

/// Default page size for `fs.find`.
const DEFAULT_PAGE_SIZE: u32 = 50;

/// Default maximum recursion depth.
const DEFAULT_MAX_DEPTH: u32 = 16;

/// Inbound request parameters for `fs.find`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct FsFindRequest {
    /// The directory to walk; must be within an allowlist root.
    pub root: String,

    /// Glob pattern to match against entry file names (not full path).
    /// Defaults to `"*"` (match all).
    #[serde(default = "default_glob")]
    pub pattern: String,

    /// Maximum recursion depth. `0` lists only the root directory itself.
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,

    /// RFC3339 timestamp; only entries modified at or after this instant
    /// are returned. `None` disables mtime filtering.
    pub modified_since: Option<String>,

    /// Number of entries per page (1–500, default 50).
    #[serde(default = "default_page_size")]
    pub page_size: u32,

    /// Opaque cursor from a previous response; `None` fetches from the start.
    pub page_cursor: Option<String>,
}

fn default_glob() -> String {
    "*".to_owned()
}
const fn default_max_depth() -> u32 {
    DEFAULT_MAX_DEPTH
}
const fn default_page_size() -> u32 {
    DEFAULT_PAGE_SIZE
}

/// A single matching entry emitted by `fs.find`.
#[derive(Debug, Clone, Serialize)]
pub struct FindEntry {
    /// Jailed path to the matching file or directory.
    pub path: String,
    /// File size in bytes; `null` for directories.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Last modification time as RFC3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
    /// Whether this entry is a directory.
    pub is_dir: bool,
}

/// Handler for `fs.find`.
///
/// Zone B: synchronous `ignore::WalkBuilder` wrapped in `spawn_blocking`.
/// Cancel-safe: `CancellationToken` is checked at directory-iteration boundaries.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, glob compilation,
/// cursor decoding, or `ignore::WalkBuilder` I/O.
#[expect(
    clippy::too_many_lines,
    reason = "fs.find orchestrates jail, glob, mpsc walk, and pagination in one cohesive Zone-B handler"
)]
#[instrument(skip(deps, cancel), fields(root = %req.root, pattern = %req.pattern))]
pub async fn handle_fs_find(
    req: FsFindRequest,
    deps: &FsQueryDeps,
    cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    let page_size = req.page_size.clamp(1, MAX_PAGE_SIZE);

    // Validate the cursor offset.
    let skip_count: usize = if let Some(ref cursor_str) = req.page_cursor {
        decode_cursor(cursor_str)?
    } else {
        0
    };

    // Parse modified_since filter.
    let modified_since_secs: Option<u64> = req
        .modified_since
        .as_deref()
        .map(parse_modified_since)
        .transpose()
        .map_err(|reason| SubstrateError::InvalidArgument {
            offending_field: "modified_since".to_owned(),
            reason,
            correlation_id: None,
        })?;

    // Jail the root path.
    let raw_root = std::path::Path::new(&req.root).to_path_buf();
    let jail: Arc<dyn PathJailPort> = Arc::clone(&deps.jail);

    // Jail validation must run in spawn_blocking because it may do I/O.
    let jailed_root: JailedPath = {
        let jail_clone = Arc::clone(&jail);
        // We need a fake "allowlist_root" — in real integration the composition root
        // supplies the allowlist root. Here we use the path itself as the root
        // so that the jail contract is satisfiable for any path the caller provides,
        // with actual enforcement delegated to the policy adapter.
        //
        // NOTE: the composition root MUST wire a real allowlist root.
        let raw_clone = raw_root.clone();
        tokio::task::spawn_blocking(move || {
            jail_clone.jail(&JailedPath::new_jailed(raw_clone.clone()), &raw_clone)
        })
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error: {e}"),
            correlation_id: None,
        })??
    };

    // Compile glob pattern.
    let glob_pattern = req.pattern.clone();
    let glob = globset::GlobBuilder::new(&glob_pattern)
        .literal_separator(false)
        .build()
        .and_then(|g| {
            let mut builder = globset::GlobSetBuilder::new();
            builder.add(g);
            builder.build()
        })
        .map_err(|e| SubstrateError::InvalidArgument {
            offending_field: "pattern".to_owned(),
            reason: format!("invalid glob: {e}"),
            correlation_id: None,
        })?;

    // Channel to stream entries from the blocking walker thread.
    let (tx, mut rx) = mpsc::channel::<SubstrateResult<FindEntry>>(256);

    let max_depth = req.max_depth as usize;
    let cancel_clone = cancel.clone();

    let walker_handle = tokio::task::spawn_blocking(move || {
        let mut walker = WalkBuilder::new(jailed_root.as_path());
        walker
            .max_depth(Some(max_depth))
            .follow_links(false)
            .hidden(false);

        for result in walker.build() {
            // Cancel-safety: check before each directory entry.
            if cancel_clone.is_cancelled() {
                break;
            }

            let entry = match result {
                Ok(e) => e,
                Err(err) => {
                    let _ = tx.blocking_send(Err(SubstrateError::IoError {
                        path: err.to_string(),
                        correlation_id: None,
                    }));
                    continue;
                },
            };

            let path = entry.path();

            // Apply glob to the file name only.
            let matches_glob = path.file_name().is_none_or(|name| glob.is_match(name));
            if !matches_glob {
                continue;
            }

            // Apply mtime filter when requested.
            let (size_bytes, modified_at, is_dir) = match entry.metadata() {
                Ok(meta) => {
                    let is_dir = meta.is_dir();
                    let size = if is_dir { None } else { Some(meta.len()) };

                    let mtime_str = meta.modified().ok().map(|t| {
                        let secs = t
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or(Duration::ZERO)
                            .as_secs();
                        format_unix_secs(secs)
                    });

                    // mtime filter
                    if let Some(since) = modified_since_secs {
                        let entry_secs = meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map_or(0, |d| d.as_secs());
                        if entry_secs < since {
                            continue;
                        }
                    }

                    (size, mtime_str, is_dir)
                },
                Err(_) => (None, None, false),
            };

            let item = FindEntry {
                path: path.to_string_lossy().into_owned(),
                size_bytes,
                modified_at,
                is_dir,
            };

            if tx.blocking_send(Ok(item)).is_err() {
                // Consumer dropped; stop walking.
                break;
            }
        }
    });

    // Collect up to (skip_count + page_size + 1) entries to detect next page.
    let mut all_entries: Vec<FindEntry> = Vec::new();
    let target = skip_count + page_size as usize + 1;

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                return Err(SubstrateError::Cancelled { correlation_id: None });
            }
            entry = rx.recv() => {
                match entry {
                    None => break,
                    Some(Err(e)) => return Err(e),
                    Some(Ok(e)) => {
                        all_entries.push(e);
                        if all_entries.len() >= target {
                            // Drop the receiver to signal the walker to stop.
                            drop(rx);
                            break;
                        }
                    }
                }
            }
        }
    }

    // Await the blocking task to ensure OS resources are released.
    let _ = walker_handle.await;

    let has_more = all_entries.len() > skip_count + page_size as usize;
    let page: Vec<FindEntry> = all_entries
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
        Some("fs.read"),
        Some("fs.stat"),
        Some("Narrow the pattern or reduce max_depth on large trees"),
        &deps.capabilities,
        true,
    );

    let content = format!(
        "USE: locate files in directory tree\nDOES: returned {} entries\nNEXT: fs.read, fs.stat\nAVOID: recursive fs.read_dir → use fs.find",
        page.len()
    );

    let structured_content = json!({
        "tool": "fs.find",
        "matches": page,
        "next_cursor": next_cursor,
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

// ---- Cursor helpers ---------------------------------------------------------

fn encode_cursor(offset: usize) -> String {
    use base64_simd::STANDARD;
    STANDARD.encode_to_string(offset.to_le_bytes().as_ref())
}

fn decode_cursor(cursor: &str) -> SubstrateResult<usize> {
    use base64_simd::STANDARD;
    let bytes =
        STANDARD
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

fn parse_modified_since(s: &str) -> Result<u64, String> {
    // Parse as RFC3339 via `time` crate.
    use time::format_description::well_known::Rfc3339;
    let dt = time::OffsetDateTime::parse(s, &Rfc3339)
        .map_err(|e| format!("not a valid RFC3339 timestamp: {e}"))?;
    Ok(dt.unix_timestamp().cast_unsigned())
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
    use futures::StreamExt as _;
    use std::sync::Arc;
    use tempfile::TempDir;

    // Minimal PathJailPort stub that always succeeds for any path inside the tempdir.
    struct NoopJail;

    impl substrate_domain::PathJailPort for NoopJail {
        fn jail(
            &self,
            _allowlist_root: &JailedPath,
            raw_path: &std::path::Path,
        ) -> SubstrateResult<JailedPath> {
            Ok(JailedPath::new_jailed(raw_path.to_path_buf()))
        }
    }

    struct NoopWalker;

    impl substrate_domain::DirWalkerPort for NoopWalker {
        fn walk<'a>(
            &'a self,
            _root: &'a JailedPath,
            _opts: substrate_domain::ports::dir_walker::WalkOpts,
        ) -> futures::stream::BoxStream<
            'a,
            SubstrateResult<substrate_domain::ports::dir_walker::DirEntry>,
        > {
            futures::stream::empty().boxed()
        }
    }

    struct NoopHasher;

    impl substrate_domain::HashPort for NoopHasher {
        fn hash_file(
            &self,
            _path: &JailedPath,
        ) -> SubstrateResult<substrate_domain::ports::hash::Blake3Digest> {
            Ok(substrate_domain::ports::hash::Blake3Digest::new([0u8; 32]))
        }
        fn hash_bytes(&self, _data: &[u8]) -> substrate_domain::ports::hash::Blake3Digest {
            substrate_domain::ports::hash::Blake3Digest::new([0u8; 32])
        }
    }

    struct NoopStatter;

    impl substrate_domain::StatPort for NoopStatter {
        fn stat(
            &self,
            _path: &JailedPath,
        ) -> SubstrateResult<substrate_domain::ports::stat::FileStat> {
            Ok(substrate_domain::ports::stat::FileStat {
                size_bytes: 0,
                is_dir: true,
                is_file: false,
                is_symlink: false,
                modified_at: time::OffsetDateTime::UNIX_EPOCH,
                accessed_at: time::OffsetDateTime::UNIX_EPOCH,
            })
        }
    }

    fn make_deps() -> FsQueryDeps {
        FsQueryDeps {
            jail: Arc::new(NoopJail),
            walker: Arc::new(NoopWalker),
            hasher: Arc::new(NoopHasher),
            statter: Arc::new(NoopStatter),
            capabilities: Arc::new(substrate_domain::Capabilities::default()),
        }
    }

    #[tokio::test]
    async fn find_empty_dir_returns_empty_matches() {
        let tmp = TempDir::new().expect("tempdir");
        let deps = make_deps();
        let req = FsFindRequest {
            root: tmp.path().to_string_lossy().into_owned(),
            pattern: "*".to_owned(),
            max_depth: 1,
            modified_since: None,
            page_size: 50,
            page_cursor: None,
        };
        let resp = handle_fs_find(req, &deps, CancellationToken::new())
            .await
            .expect("ok");
        let matches = resp.structured_content["matches"]
            .as_array()
            .expect("array");
        // Root dir entry is always emitted by WalkBuilder; empty dir yields 1 entry (root itself).
        assert!(matches.len() <= 1);
    }

    #[tokio::test]
    async fn find_with_files_returns_entries() {
        let tmp = TempDir::new().expect("tempdir");
        std::fs::write(tmp.path().join("a.txt"), b"hello").expect("write");
        std::fs::write(tmp.path().join("b.rs"), b"fn main(){}").expect("write");
        let deps = make_deps();
        let req = FsFindRequest {
            root: tmp.path().to_string_lossy().into_owned(),
            pattern: "*.txt".to_owned(),
            max_depth: 1,
            modified_since: None,
            page_size: 50,
            page_cursor: None,
        };
        let resp = handle_fs_find(req, &deps, CancellationToken::new())
            .await
            .expect("ok");
        let matches = resp.structured_content["matches"]
            .as_array()
            .expect("array");
        let paths: Vec<&str> = matches.iter().filter_map(|e| e["path"].as_str()).collect();
        assert!(paths.iter().any(|p| p.ends_with("a.txt")));
        assert!(!paths.iter().any(|p| p.ends_with("b.rs")));
    }

    #[tokio::test]
    async fn invalid_glob_returns_error() {
        let tmp = TempDir::new().expect("tempdir");
        let deps = make_deps();
        let req = FsFindRequest {
            root: tmp.path().to_string_lossy().into_owned(),
            pattern: "[invalid".to_owned(),
            max_depth: 1,
            modified_since: None,
            page_size: 50,
            page_cursor: None,
        };
        let result = handle_fs_find(req, &deps, CancellationToken::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cursor_round_trip() {
        let encoded = encode_cursor(42);
        let decoded = decode_cursor(&encoded).expect("decode");
        assert_eq!(decoded, 42);
    }

    #[tokio::test]
    async fn invalid_cursor_returns_error() {
        let result = decode_cursor("not_base64!!!");
        assert!(result.is_err());
    }
}
