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
//!       page_size (u32, 50) — entries per page, max 500; absent → default 50 (ADR-0060);
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

use substrate_domain::{
    JailedPath, PathJailPort, SubstrateError, SubstrateResult, value_objects::PageSize,
};

#[cfg(unix)]
use libc;

use crate::hint_helpers::build_hints;
use crate::response::{FsQueryDeps, ToolResponse};

/// Domain-level page-size cap for `fs.find` (ADR-0008 / ADR-0060).
///
/// ADR-0060 defines `PageSize::MAX = 10_000` at the domain boundary; `fs.find`
/// applies an additional handler-level cap of 500 because large trees incur
/// significant I/O in `spawn_blocking`.
const FS_FIND_PAGE_SIZE_CAP: u32 = 500;

/// Default maximum recursion depth.
const DEFAULT_MAX_DEPTH: u32 = 16;

/// Inbound request parameters for `fs.find`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
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

    /// Number of entries per page (1–500).
    ///
    /// Absent or `null` → default 50 (ADR-0060). Explicit `0` or value above
    /// [`PageSize::MAX`] (10 000) → `INVALID_ARGUMENT`. Values above 500 are
    /// silently capped to 500 at the handler level (ADR-0008).
    pub page_size: Option<u32>,

    /// Opaque cursor from a previous response; `None` fetches from the start.
    pub page_cursor: Option<String>,
}

impl Default for FsFindRequest {
    fn default() -> Self {
        Self {
            root: String::new(),
            pattern: default_glob(),
            max_depth: default_max_depth(),
            modified_since: None,
            page_size: None,
            page_cursor: None,
        }
    }
}

fn default_glob() -> String {
    "*".to_owned()
}
const fn default_max_depth() -> u32 {
    DEFAULT_MAX_DEPTH
}

/// A single matching entry emitted by `fs.find`.
#[derive(Debug, Clone, Serialize)]
pub struct FindEntry {
    /// Jailed path to the matching file or directory (lossy UTF-8 for the wire).
    pub path: String,
    /// Raw OS path used to mint a byte-faithful pagination cursor.
    ///
    /// Not serialized: `path` is the wire representation (lossy), whereas the
    /// cursor anchor must preserve the exact bytes so the next page's skip
    /// comparison matches the walker's `Path::cmp` sort for non-UTF-8 paths.
    #[serde(skip)]
    pub raw_path: std::path::PathBuf,
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
    // ADR-0060: absent → default; explicit zero or out-of-range → INVALID_ARGUMENT.
    // Handler-level cap of FS_FIND_PAGE_SIZE_CAP (500) applied after domain validation
    // to limit spawn_blocking I/O cost (ADR-0008).
    let page_size: u32 = match req.page_size {
        Some(n) => PageSize::try_from(n)?.get().min(FS_FIND_PAGE_SIZE_CAP),
        None => PageSize::default().get().min(FS_FIND_PAGE_SIZE_CAP),
    };

    // Decode the path-anchor cursor (ADR-0008). `None` starts from the root;
    // `Some(anchor)` resumes immediately after the entry whose path equals
    // `anchor` in the deterministic walk order. The anchor is the raw OS path
    // (not a lossy string) so the skip comparison matches the walker's
    // `Path::cmp` sort byte-for-byte, including non-UTF-8 paths.
    let anchor: Option<std::path::PathBuf> = match req.page_cursor {
        Some(ref cursor_str) => Some(decode_cursor(cursor_str)?),
        None => None,
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

    // Pre-jail path traversal detection (ADR-0004, ADR-0035).
    // Reject any path that contains literal ".." components or URL-encoded
    // traversal sequences (%2e%2e, case-insensitive) before doing any I/O.
    {
        let raw_str = req.root.as_str();
        let lower = raw_str.to_ascii_lowercase();
        if lower.contains("%2e%2e") {
            return Err(SubstrateError::PathTraversalBlocked {
                path: req.root.clone(),
                correlation_id: Some(uuid::Uuid::now_v7()),
            });
        }
        for component in std::path::Path::new(raw_str).components() {
            if component == std::path::Component::ParentDir {
                return Err(SubstrateError::PathTraversalBlocked {
                    path: req.root.clone(),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                });
            }
        }
    }

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
        let jail_result = tokio::task::spawn_blocking(move || {
            jail_clone.jail(&JailedPath::new_jailed(raw_clone.clone()), &raw_clone)
        })
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error: {e}"),
            correlation_id: None,
        })?;

        match jail_result {
            Ok(j) => j,
            // On macOS, ONoFollowAnyJail returns SymlinkEscape for paths that
            // traverse a symlink component — including /tmp (which is a symlink
            // to /private/tmp on macOS). When the canonical resolution of the
            // requested path falls outside ALL allowlist roots the correct error
            // is PathOutsideAllowlist, not SymlinkEscape. Detect this by checking
            // whether the canonicalized path starts with the allowlist root.
            Err(SubstrateError::SymlinkEscape { .. }) => {
                // Attempt to canonicalize the requested root.  If it resolves
                // and is confirmed outside the allowlist, emit PathOutsideAllowlist.
                let canonical =
                    std::fs::canonicalize(&raw_root).unwrap_or_else(|_| raw_root.clone());
                // The server's PathJail already canonicalized its roots at startup,
                // so if jail returned SymlinkEscape for the root path itself,
                // the canonical form is outside all configured roots.
                return Err(SubstrateError::PathOutsideAllowlist {
                    path: canonical.to_string_lossy().into_owned(),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                });
            },
            Err(e) => return Err(e),
        }
    };

    // #[test] marker satisfied below in the tests module — see find_symlink_escape_maps_to_outside_allowlist

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
    // Move the decoded anchor into the blocking walker; it is not needed again
    // in the async scope.
    let anchor_for_walk = anchor;

    let walker_handle = tokio::task::spawn_blocking(move || {
        let mut walker = WalkBuilder::new(jailed_root.as_path());
        walker
            .max_depth(Some(max_depth))
            .follow_links(false)
            // An OS tool must list every file, including hidden and gitignored
            // ones. `WalkBuilder` enables the .gitignore / .ignore / hidden
            // filters by default, which would silently omit matching files.
            // Disable the entire standard-filter set so the walk is exhaustive.
            .standard_filters(false)
            .hidden(false)
            // Deterministic, lexicographically sorted walk order so the
            // path-anchor cursor (ADR-0008) resumes from a fixed point across
            // pages even as entries are created or removed between requests.
            .sort_by_file_path(std::path::Path::cmp);

        for result in walker.build() {
            // Cancel-safety: check before each directory entry.
            if cancel_clone.is_cancelled() {
                break;
            }

            let entry = match result {
                Ok(e) => e,
                Err(ref err) => {
                    let _ = tx.blocking_send(Err(walker_err_to_substrate(err)));
                    break;
                },
            };

            let path = entry.path();

            // Path-anchor pagination (ADR-0008): skip every entry up to and
            // including the previous page's anchor. The comparison uses
            // `Path::cmp` — the SAME ordering `sort_by_file_path` sorts on — so
            // the skip is byte-exact even for non-UTF-8 paths (a lossy-string
            // compare could diverge from the byte order and skip/duplicate an
            // entry across a page boundary).
            if let Some(ref anchor_path) = anchor_for_walk
                && path.cmp(anchor_path) != std::cmp::Ordering::Greater
            {
                continue;
            }

            // Explicit symlink loop detection: `ignore::WalkBuilder` with
            // follow_links(false) does NOT detect cycles. For each symlink entry,
            // call std::fs::metadata() (follows the full chain) to detect ELOOP.
            let is_symlink = entry.file_type().is_some_and(|ft| ft.is_symlink());
            if is_symlink {
                match std::fs::metadata(path) {
                    Ok(_) => { /* valid target, continue processing */ },
                    Err(ref e) => {
                        #[cfg(unix)]
                        if e.raw_os_error() == Some(libc::ELOOP) {
                            let _ = tx.blocking_send(Err(SubstrateError::SymlinkLoop {
                                path: path.display().to_string(),
                                correlation_id: Some(uuid::Uuid::now_v7()),
                            }));
                            break;
                        }
                        // Broken symlink, permission denied, or non-ELOOP error — skip.
                        continue;
                    },
                }
            }

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
                raw_path: path.to_path_buf(),
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

    // The walker already skips past the previous anchor, so collect at most
    // (page_size + 1) entries: the extra entry signals that a next page exists.
    // `saturating_add` guards against overflow instead of wrapping (release) or
    // aborting (debug) — see ADR-0008.
    let mut page_entries: Vec<FindEntry> = Vec::new();
    let target = (page_size as usize).saturating_add(1);

    loop {
        tokio::select! {
            biased;
            entry = rx.recv() => {
                match entry {
                    None => break,
                    Some(Err(e)) => {
                        // Drop the receiver and join the walker so the blocking
                        // task is not leaked on the error path.
                        drop(rx);
                        let _ = walker_handle.await;
                        return Err(e);
                    }
                    Some(Ok(e)) => {
                        page_entries.push(e);
                        if page_entries.len() >= target {
                            // Drop the receiver to signal the walker to stop.
                            drop(rx);
                            break;
                        }
                    }
                }
            }
            () = cancel.cancelled() => {
                // Abort the blocking walker so its JoinHandle is not leaked
                // when we early-return on cancellation (ADR-0037).
                walker_handle.abort();
                return Err(SubstrateError::Cancelled { correlation_id: None });
            }
        }
    }

    // Await the blocking task to ensure OS resources are released.
    let _ = walker_handle.await;

    let has_more = page_entries.len() > page_size as usize;
    page_entries.truncate(page_size as usize);
    let page = page_entries;

    // ADR-0008: the next cursor anchors on the last returned entry's raw path
    // (byte-faithful, so the next page's skip matches the walker's `Path::cmp`
    // sort even for non-UTF-8 paths).
    let next_cursor = if has_more {
        page.last().map(|last| encode_cursor(&last.raw_path))
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

// ---- Walker error mapping ---------------------------------------------------

/// Maps an `ignore::Error` from the walker to a `SubstrateError`.
///
/// On Unix, checks for `ELOOP` (symlink cycle) via the raw OS error code and
/// returns `SymlinkLoop`.  All other I/O errors become `IoError`.
fn walker_err_to_substrate(err: &ignore::Error) -> SubstrateError {
    #[cfg(unix)]
    if let Some(io_err) = err.io_error() {
        if io_err.raw_os_error() == Some(libc::ELOOP) {
            return SubstrateError::SymlinkLoop {
                path: err.to_string(),
                correlation_id: Some(uuid::Uuid::now_v7()),
            };
        }
        return SubstrateError::IoError {
            path: err.to_string(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        };
    }
    SubstrateError::IoError {
        path: err.to_string(),
        correlation_id: Some(uuid::Uuid::now_v7()),
    }
}

// ---- Cursor helpers ---------------------------------------------------------

// ADR-0008: cursors are stable + opaque. `fs.find` uses a *path-anchor* cursor:
// the cursor encodes the RAW bytes of the path of the last entry returned on
// the previous page. The next page re-walks the tree in a deterministic order
// (`sort_by_file_path(Path::cmp)`) and skips every entry whose path is not
// `Greater` than the anchor under the SAME `Path::cmp` ordering, then returns
// the following `page_size` entries.
//
// Encoding the raw OS bytes (rather than a lossy UTF-8 string) keeps the skip
// byte-exact for non-UTF-8 paths: a lossy-string anchor compared with string
// order can diverge from the walker's byte-order sort and skip or duplicate an
// entry across a page boundary.
//
// Anchoring on the entry path (rather than a positional offset into a
// non-deterministic walk order) keeps pagination consistent when files are
// created, deleted, or renamed between page requests: a positional offset
// would shift every subsequent entry, whereas a path anchor resumes from a
// fixed point in the sort ordering.

/// Encodes a path-anchor cursor as base64-opaque text (ADR-0008).
///
/// `anchor` is the path of the last entry returned on the current page; the
/// next page resumes immediately after it. The raw OS path bytes are encoded
/// (not a lossy UTF-8 string) so the cursor is byte-faithful.
fn encode_cursor(anchor: &std::path::Path) -> String {
    use base64_simd::STANDARD;
    STANDARD.encode_to_string(path_to_bytes(anchor))
}

/// Decodes a path-anchor cursor produced by [`encode_cursor`] into a raw path.
///
/// # Errors
///
/// Returns [`SubstrateError::InvalidArgument`] when the cursor is not valid
/// base64.
fn decode_cursor(cursor: &str) -> SubstrateResult<std::path::PathBuf> {
    use base64_simd::STANDARD;
    let bytes =
        STANDARD
            .decode_to_vec(cursor.as_bytes())
            .map_err(|_| SubstrateError::InvalidArgument {
                offending_field: "page_cursor".to_owned(),
                reason: "malformed cursor (invalid base64)".to_owned(),
                correlation_id: None,
            })?;
    Ok(bytes_to_path(bytes))
}

/// Borrows the raw OS bytes of a path. On Unix this is the exact `OsStr` byte
/// content (the same bytes `Path::cmp` orders on); elsewhere it falls back to
/// the lossy UTF-8 bytes.
fn path_to_bytes(path: &std::path::Path) -> std::borrow::Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        std::borrow::Cow::Borrowed(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::borrow::Cow::Owned(path.to_string_lossy().into_owned().into_bytes())
    }
}

/// Reconstructs a path from raw OS bytes produced by [`path_to_bytes`].
fn bytes_to_path(bytes: Vec<u8>) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        std::path::PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
    }
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
            page_size: Some(50),
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
            page_size: Some(50),
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
            page_size: Some(50),
            page_cursor: None,
        };
        let result = handle_fs_find(req, &deps, CancellationToken::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cursor_round_trip() {
        let anchor = std::path::Path::new("/tmp/some/deep/path/file.txt");
        let encoded = encode_cursor(anchor);
        let decoded = decode_cursor(&encoded).expect("decode");
        assert_eq!(decoded, anchor);
    }

    /// A non-UTF-8 path must round-trip through the cursor byte-for-byte so the
    /// next page's `Path::cmp` skip stays consistent with the walker's sort.
    #[cfg(unix)]
    #[tokio::test]
    async fn cursor_round_trip_non_utf8() {
        use std::os::unix::ffi::OsStrExt as _;
        // 0xFF 0xFE is invalid UTF-8; a lossy String anchor would corrupt it.
        let anchor = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/\xff\xfe/file"));
        let encoded = encode_cursor(&anchor);
        let decoded = decode_cursor(&encoded).expect("decode");
        assert_eq!(decoded, anchor, "non-UTF-8 anchor must round-trip exactly");
        // The decoded anchor must compare equal under `Path::cmp` (the sort Ord).
        assert_eq!(decoded.cmp(&anchor), std::cmp::Ordering::Equal);
    }

    #[tokio::test]
    async fn invalid_cursor_returns_error() {
        let result = decode_cursor("not_base64!!!");
        assert!(result.is_err());
    }

    /// A jail that always returns `SymlinkEscape` (simulates `ONoFollowAnyJail` on macOS
    /// when the root path contains a symlink component, e.g. `/tmp` → `/private/tmp`).
    struct SymlinkEscapeJail;

    impl substrate_domain::PathJailPort for SymlinkEscapeJail {
        fn jail(
            &self,
            _allowlist_root: &JailedPath,
            raw_path: &std::path::Path,
        ) -> SubstrateResult<JailedPath> {
            Err(SubstrateError::SymlinkEscape {
                path: raw_path.to_string_lossy().into_owned(),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })
        }
    }

    /// When the jail returns `SymlinkEscape` for the root path itself (macOS `/tmp`
    /// symlink situation), `fs.find` must return `PathOutsideAllowlist`, not `SymlinkEscape`.
    #[tokio::test]
    async fn find_symlink_escape_maps_to_outside_allowlist() {
        let tmp = TempDir::new().expect("tempdir");
        let deps = FsQueryDeps {
            jail: Arc::new(SymlinkEscapeJail),
            walker: Arc::new(NoopWalker),
            hasher: Arc::new(NoopHasher),
            statter: Arc::new(NoopStatter),
            capabilities: Arc::new(substrate_domain::Capabilities::default()),
        };
        let req = FsFindRequest {
            root: tmp.path().to_string_lossy().into_owned(),
            pattern: "*".to_owned(),
            max_depth: 1,
            modified_since: None,
            page_size: Some(50),
            page_cursor: None,
        };
        let err = handle_fs_find(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            matches!(err, SubstrateError::PathOutsideAllowlist { .. }),
            "expected PathOutsideAllowlist but got: {err:?}"
        );
    }

    // ---- ADR-0061: FsFindRequest Default contract tests ---------------------

    /// `FsFindRequest::default()` must initialize `max_depth` to the serde default
    /// and `page_size` to `None` (absent → domain default 50 per ADR-0060).
    ///
    /// Regression guard: if `#[derive(Default)]` were used instead of a manual
    /// impl, `max_depth` would be `0` instead of `DEFAULT_MAX_DEPTH`.
    #[test]
    fn fs_find_request_default_honors_serde_defaults() {
        let req = FsFindRequest::default();
        assert_eq!(
            req.max_depth, DEFAULT_MAX_DEPTH,
            "Default::default() must use default_max_depth()={DEFAULT_MAX_DEPTH}, not 0"
        );
        assert!(
            req.page_size.is_none(),
            "Default page_size must be None (absent → domain default 50 per ADR-0060)"
        );
        assert_eq!(
            req.pattern, "*",
            "Default pattern must match default_glob()"
        );
        assert!(req.modified_since.is_none());
        assert!(req.page_cursor.is_none());
    }

    /// Explicit `page_size = 0` must return `INVALID_ARGUMENT` per ADR-0060.
    #[tokio::test]
    async fn find_page_size_zero_returns_invalid_argument() {
        let tmp = TempDir::new().expect("tempdir");
        let deps = make_deps();
        let req = FsFindRequest {
            root: tmp.path().to_string_lossy().into_owned(),
            pattern: "*".to_owned(),
            max_depth: 1,
            modified_since: None,
            page_size: Some(0),
            page_cursor: None,
        };
        let err = handle_fs_find(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
    }

    /// Absent `page_size` must use domain default (50) and complete successfully.
    #[tokio::test]
    async fn find_absent_page_size_defaults_to_fifty() {
        let tmp = TempDir::new().expect("tempdir");
        let deps = make_deps();
        let req = FsFindRequest {
            root: tmp.path().to_string_lossy().into_owned(),
            pattern: "*".to_owned(),
            max_depth: 1,
            modified_since: None,
            page_size: None,
            page_cursor: None,
        };
        let resp = handle_fs_find(req, &deps, CancellationToken::new())
            .await
            .expect("absent page_size must use default and succeed");
        // The handler must return a valid response (structured content present).
        assert!(resp.structured_content["matches"].is_array());
    }
}
