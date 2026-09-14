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

use substrate_domain::{
    JailedPath, PathJailPort, SubstrateError, SubstrateResult, value_objects::PageSize,
};

use crate::hint_helpers::build_hints;
use crate::response::{FsQueryDeps, ToolResponse};

/// Maximum page size for `fs.read_dir`.
const MAX_PAGE_SIZE: u32 = 5_000;

/// Default page size for `fs.read_dir`.
const DEFAULT_PAGE_SIZE: u32 = 100;

/// Inbound request for `fs.read_dir`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsReadDirRequest {
    /// The directory to list; must be within an allowlist root.
    pub path: String,

    /// Maximum number of entries per page.
    ///
    /// Routed through the domain [`PageSize`] value object (ADR-0057 /
    /// ADR-0060): `0` or a value above [`PageSize::MAX`] (10 000) returns
    /// `INVALID_ARGUMENT`. Values within `[1, 10_000]` but above
    /// [`MAX_PAGE_SIZE`] (5 000) are silently capped to `MAX_PAGE_SIZE` at the
    /// handler level (ADR-0008), mirroring `fs.find`'s `FS_FIND_PAGE_SIZE_CAP`.
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
#[expect(
    clippy::too_many_lines,
    reason = "handle_fs_read_dir orchestrates PageSize validation, overflow-checked cursor \
              arithmetic, jail, and paginated directory iteration in one cohesive Zone-A handler"
)]
#[instrument(skip(deps, _cancel), fields(path = %req.path))]
pub async fn handle_fs_read_dir(
    req: FsReadDirRequest,
    deps: &FsQueryDeps,
    _cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    // Route `page_size` through the domain `PageSize` value object (ADR-0057 /
    // ADR-0060): `0` or a value above `PageSize::MAX` (10 000) is rejected
    // outright instead of being silently clamped. A value inside the domain
    // range but above the handler-level `MAX_PAGE_SIZE` (5 000) is still
    // capped, mirroring `fs.find`'s `FS_FIND_PAGE_SIZE_CAP` (ADR-0008).
    let page_size = PageSize::try_from(req.page_size)?.get().min(MAX_PAGE_SIZE);
    let skip_count: usize = if let Some(ref cursor_str) = req.page_cursor {
        decode_cursor(cursor_str)?
    } else {
        0
    };

    // Guard against a malformed or adversarial cursor whose decoded
    // `skip_count` is large enough that adding `page_size` (or the +1
    // lookahead) would overflow `usize`. Treated the same way as malformed
    // base64 below: both are caller input errors, not internal errors.
    let skip_plus_page = skip_count.checked_add(page_size as usize).ok_or_else(|| {
        SubstrateError::InvalidArgument {
            offending_field: "page_cursor".to_owned(),
            reason: "page_cursor combined with page_size overflows usize".to_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        }
    })?;
    let target = skip_plus_page
        .checked_add(1)
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "page_cursor".to_owned(),
            reason: "page_cursor combined with page_size overflows usize".to_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;

    // Jail the path against the real allowlist root (never the path itself —
    // see `FsQueryDeps::allowlist_root` doc comment for why that would make
    // the kernel dirfd confinement check a no-op).
    let raw = std::path::Path::new(&req.path).to_path_buf();
    let jail: Arc<dyn PathJailPort> = Arc::clone(&deps.jail);
    let raw_clone = raw.clone();
    let allowlist_root = deps.allowlist_root.clone();
    let jailed: JailedPath =
        tokio::task::spawn_blocking(move || jail.jail(&allowlist_root, &raw_clone))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking join error: {e}"),
                correlation_id: None,
            })??;

    // Zone A: async read_dir.
    let mut read_dir = tokio::fs::read_dir(jailed.as_path())
        .await
        .map_err(|e| map_io_err(e, &req.path))?;

    // Collect all entries (async iterator). `target` was already validated
    // above (skip_count + page_size + 1, overflow-checked).
    let mut all_entries: Vec<DirEntryInfo> = Vec::new();

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

    let has_more = all_entries.len() > skip_plus_page;
    let page: Vec<DirEntryInfo> = all_entries
        .into_iter()
        .skip(skip_count)
        .take(page_size as usize)
        .collect();

    let next_cursor = if has_more {
        Some(encode_cursor(skip_plus_page))
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

    /// Regression guard for the self-referential jail bug: asserts the
    /// `allowlist_root` argument `handle_fs_read_dir` passes to `jail()` is
    /// the real configured root (`deps.allowlist_root`), never a `JailedPath`
    /// fabricated from the request path itself (which would make the kernel
    /// dirfd containment check a no-op).
    struct AssertRealRootJail {
        expected_root: std::path::PathBuf,
    }
    impl substrate_domain::PathJailPort for AssertRealRootJail {
        fn jail(
            &self,
            allowlist_root: &JailedPath,
            raw_path: &std::path::Path,
        ) -> SubstrateResult<JailedPath> {
            assert_eq!(
                allowlist_root.as_path(),
                self.expected_root.as_path(),
                "jail() must receive the real allowlist root, not a fabricated one"
            );
            assert_ne!(
                allowlist_root.as_path(),
                raw_path,
                "allowlist_root must not equal the raw request path (self-referential jail)"
            );
            Ok(JailedPath::new_jailed(raw_path.to_path_buf()))
        }
    }

    fn make_deps() -> FsQueryDeps {
        FsQueryDeps {
            jail: Arc::new(NoopJail),
            walker: Arc::new(crate::walker::legacy::LegacyWalker::new()),
            hasher: Arc::new(crate::hash_factory::Blake3Hasher::new()),
            statter: Arc::new(crate::stat_factory::PortableStatter::new()),
            capabilities: Arc::new(substrate_domain::Capabilities::default()),
            allowlist_root: JailedPath::new_jailed(std::env::temp_dir()),
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

    /// Explicit `page_size = 0` must return `INVALID_ARGUMENT`, not be clamped
    /// to 1 (ADR-0008 / ADR-0060).
    #[tokio::test]
    async fn read_dir_page_size_zero_returns_invalid_argument() {
        let tmp = TempDir::new().unwrap();
        let deps = make_deps();
        let req = FsReadDirRequest {
            path: tmp.path().to_string_lossy().into_owned(),
            page_size: 0,
            page_cursor: None,
        };
        let err = handle_fs_read_dir(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
    }

    /// `page_size` above `PageSize::MAX` (10 000) must be rejected outright
    /// (ADR-0057 / ADR-0060), not silently clamped to `MAX_PAGE_SIZE` (5 000)
    /// as the pre-fix handler-only bound check would have allowed.
    #[tokio::test]
    async fn read_dir_page_size_above_domain_max_returns_invalid_argument() {
        let tmp = TempDir::new().unwrap();
        let deps = make_deps();
        let req = FsReadDirRequest {
            path: tmp.path().to_string_lossy().into_owned(),
            page_size: 10_001,
            page_cursor: None,
        };
        let err = handle_fs_read_dir(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
    }

    /// A `page_size` within the domain range but above the handler-level
    /// `MAX_PAGE_SIZE` (5 000) is still silently capped (ADR-0008), mirroring
    /// `fs.find`'s `FS_FIND_PAGE_SIZE_CAP` behavior — this must keep working
    /// after routing through `PageSize`.
    #[tokio::test]
    async fn read_dir_page_size_above_handler_cap_is_silently_capped() {
        let tmp = TempDir::new().unwrap();
        let deps = make_deps();
        let req = FsReadDirRequest {
            path: tmp.path().to_string_lossy().into_owned(),
            page_size: 9_000,
            page_cursor: None,
        };
        let resp = handle_fs_read_dir(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert!(resp.structured_content["entries"].is_array());
    }

    /// Regression test for the cursor-overflow fix (FIX 3): a decoded
    /// `page_cursor` close to `usize::MAX` must return `INVALID_ARGUMENT`
    /// instead of panicking (debug) or wrapping (release) when combined with
    /// `page_size`.
    #[tokio::test]
    async fn read_dir_cursor_overflow_returns_invalid_argument() {
        let tmp = TempDir::new().unwrap();
        let deps = make_deps();
        let malicious_cursor = encode_cursor(usize::MAX - 1);
        let req = FsReadDirRequest {
            path: tmp.path().to_string_lossy().into_owned(),
            page_size: 100,
            page_cursor: Some(malicious_cursor),
        };
        let err = handle_fs_read_dir(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.code(), "SUBSTRATE_INVALID_ARGUMENT");
    }

    /// Regression test for the self-referential jail bug (FIX 1):
    /// `handle_fs_read_dir` must jail the requested path against
    /// `deps.allowlist_root`, not against a `JailedPath` fabricated from the
    /// path itself.
    #[tokio::test]
    async fn read_dir_jails_against_configured_allowlist_root() {
        let tmp = TempDir::new().unwrap();
        // List a subdirectory (not the allowlist root itself) so the
        // assertion genuinely distinguishes "self-referential jail" from the
        // legitimate case of listing the allowlist root directory itself.
        let subdir = tmp.path().join("child");
        std::fs::create_dir(&subdir).unwrap();
        let deps = FsQueryDeps {
            jail: Arc::new(AssertRealRootJail {
                expected_root: tmp.path().to_path_buf(),
            }),
            walker: Arc::new(crate::walker::legacy::LegacyWalker::new()),
            hasher: Arc::new(crate::hash_factory::Blake3Hasher::new()),
            statter: Arc::new(crate::stat_factory::PortableStatter::new()),
            capabilities: Arc::new(substrate_domain::Capabilities::default()),
            allowlist_root: JailedPath::new_jailed(tmp.path().to_path_buf()),
        };
        let req = FsReadDirRequest {
            path: subdir.to_string_lossy().into_owned(),
            page_size: 100,
            page_cursor: None,
        };
        handle_fs_read_dir(req, &deps, CancellationToken::new())
            .await
            .unwrap();
    }
}
