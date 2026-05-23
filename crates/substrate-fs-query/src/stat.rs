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
#[serde(deny_unknown_fields)]
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
    let jail_result: SubstrateResult<JailedPath> = tokio::task::spawn_blocking(move || {
        jail.jail(&JailedPath::new_jailed(raw_clone.clone()), &raw_clone)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: None,
    })?;

    // On macOS, ONoFollowAnyJail uses O_NOFOLLOW_ANY which returns SymlinkEscape
    // (ELOOP) for ANY symlink component — including:
    //   (a) broken (dangling) symlinks whose target does not exist → NotFound
    //   (b) internal symlinks whose target is within the allowlist → stat via lstat
    //   (c) genuine escaping symlinks pointing outside the allowlist → SymlinkEscape
    //
    // `fs.stat` has lstat semantics (stats the symlink itself, not the target).
    // We use symlink_metadata to classify the three cases without following.
    let jailed: JailedPath = match jail_result {
        Ok(j) => j,
        Err(SubstrateError::SymlinkEscape { .. }) => {
            return handle_symlink_escape(raw, req, deps).await;
        },
        Err(e) => return Err(e),
    };

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

/// Maximum symlink hops before treating the chain as an escape (loop guard).
const MAX_SYMLINK_HOPS: u8 = 40;

/// Disposition for a path when the jail returns `SymlinkEscape`.
enum SymlinkDisposition {
    /// Symlink exists but target is missing (dangling symlink).
    Broken,
    /// Symlink exists and its canonical target is within the allowlist.
    Internal(std::fs::Metadata),
    /// Symlink resolves outside the allowlist (genuine escape).
    Escape,
}

/// Recursively walks the symlink chain starting at `path` to determine its
/// disposition relative to the allowlist enforced by `jail`.
///
/// - Returns `Internal(lstat)` when all hops are within the allowlist and the
///   final target exists (or the path is not a symlink).
/// - Returns `Broken` when all hops are within the allowlist but the final
///   target does not exist (dangling symlink within the sandbox).
/// - Returns `Escape` when any hop resolves to a path outside the allowlist.
fn symlink_chain_disposition(
    path: &std::path::Path,
    jail: &dyn PathJailPort,
    lstat_of_start: &std::fs::Metadata,
    depth: u8,
) -> SymlinkDisposition {
    if depth >= MAX_SYMLINK_HOPS {
        return SymlinkDisposition::Escape;
    }

    // Read the immediate link target.
    let Ok(direct_target) = std::fs::read_link(path) else {
        // Not a symlink or cannot read link — check if it exists.
        return if std::fs::symlink_metadata(path).is_ok() {
            SymlinkDisposition::Internal(lstat_of_start.clone())
        } else {
            SymlinkDisposition::Broken
        };
    };

    // Resolve relative targets relative to the symlink's parent directory.
    let resolved_target = if direct_target.is_absolute() {
        direct_target
    } else {
        path.parent()
            .map(|p| p.join(&direct_target))
            .unwrap_or(direct_target)
    };

    // Use the jail to check if `resolved_target` is within an allowed root.
    // `JailedPath::new_jailed` + `jail.jail` checks WITHOUT following symlinks.
    // Return Escape only when the jail reports a security boundary violation
    // (PathOutsideAllowlist, SymlinkEscape, etc.).  NotFound / IoError mean the
    // path is absent but the prefix is within the allowlist — fall through to
    // the symlink_metadata check below.
    let jailed_target = JailedPath::new_jailed(resolved_target.clone());
    if let Err(e) = jail.jail(&jailed_target, &resolved_target) {
        // NotFound / IoError mean the path is absent but the prefix is within
        // the allowlist — fall through.  All other errors (PathOutsideAllowlist,
        // SymlinkEscape, …) mean the hop crosses a security boundary.
        let absent_ok =
            matches!(e, SubstrateError::NotFound { .. } | SubstrateError::IoError { .. });
        if !absent_ok {
            return SymlinkDisposition::Escape;
        }
    }

    // Target is within the allowlist. Check if it exists (lstat).
    match std::fs::symlink_metadata(&resolved_target) {
        Err(_) => SymlinkDisposition::Broken,
        Ok(target_meta) if target_meta.file_type().is_symlink() => {
            // Target is itself a symlink — recurse.
            symlink_chain_disposition(&resolved_target, jail, lstat_of_start, depth + 1)
        },
        Ok(_) => SymlinkDisposition::Internal(lstat_of_start.clone()),
    }
}

/// Handles the `SymlinkEscape` case from the jail for `fs.stat`.
///
/// On macOS, `ONoFollowAnyJail` emits `SymlinkEscape` for ALL symlink
/// components.  This function probes to distinguish three cases:
/// - broken (dangling) symlink → `NotFound`
/// - internal symlink (target within allowlist) → lstat result
/// - genuine escape (target outside allowlist) → `SymlinkEscape`
///
/// The jail is used to verify the canonical (fully-resolved) target path
/// without symlinks — this is the authoritative allowlist check.
async fn handle_symlink_escape(
    raw: std::path::PathBuf,
    req: FsStatRequest,
    deps: &FsQueryDeps,
) -> SubstrateResult<ToolResponse> {
    let jail = Arc::clone(&deps.jail);
    let raw_clone = raw.clone();
    let disposition = tokio::task::spawn_blocking(move || {
        // lstat the symlink node itself — succeeds for both internal and escaping
        // symlinks regardless of whether the target exists.
        let Ok(lstat) = std::fs::symlink_metadata(&raw_clone) else {
            return SymlinkDisposition::Escape;
        };
        if !lstat.file_type().is_symlink() {
            // Not a symlink — unexpected; treat as internal.
            return SymlinkDisposition::Internal(lstat);
        }
        // Walk the symlink chain step-by-step to determine if it escapes the
        // allowlist. We use `read_link` (not `canonicalize`) to inspect each hop
        // so that non-existent targets and escaping chains are handled correctly.
        // Strategy: resolve each hop and ask the jail if the hop target is allowed.
        // A non-existent hop within the allowlist → Broken.
        // Any hop outside the allowlist → Escape.
        // All hops within allowlist and final target exists → Internal.
        symlink_chain_disposition(&raw_clone, jail.as_ref(), &lstat, 0)
    })
    .await
    .unwrap_or(SymlinkDisposition::Escape);

    match disposition {
        SymlinkDisposition::Broken => Err(SubstrateError::NotFound {
            resource: req.path,
            correlation_id: Some(uuid::Uuid::now_v7()),
        }),
        SymlinkDisposition::Internal(lstat) => Ok(serve_lstat_response(&lstat, &req, deps)),
        SymlinkDisposition::Escape => Err(SubstrateError::SymlinkEscape {
            path: req.path,
            correlation_id: Some(uuid::Uuid::now_v7()),
        }),
    }
}

/// Builds a `ToolResponse` from OS-level `std::fs::Metadata` (lstat result).
///
/// Called when the path jail returns `SymlinkEscape` but the symlink is
/// classified as internal (target within the allowlist).  We serve the
/// symlink's own lstat data directly without going through [`StatPort`].
fn serve_lstat_response(
    meta: &std::fs::Metadata,
    req: &FsStatRequest,
    deps: &FsQueryDeps,
) -> ToolResponse {
    use std::time::UNIX_EPOCH;
    use time::format_description::well_known::Rfc3339;

    let ft = meta.file_type();
    let is_symlink = ft.is_symlink();
    let is_dir = ft.is_dir();
    let is_file = ft.is_file();

    let kind = if is_symlink {
        "symlink"
    } else if is_dir {
        "directory"
    } else if is_file {
        "file"
    } else {
        "special"
    };

    let size_bytes = meta.len();

    let fmt_time = |t: std::result::Result<std::time::SystemTime, _>| {
        t.ok()
            .and_then(|st| st.duration_since(UNIX_EPOCH).ok())
            .map_or_else(
                || "1970-01-01T00:00:00Z".to_owned(),
                |d| {
                    #[expect(
                        clippy::cast_possible_wrap,
                        reason = "unix timestamps in the valid range fit in i64"
                    )]
                    let ts = time::OffsetDateTime::from_unix_timestamp(d.as_secs() as i64)
                        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
                    ts.format(&Rfc3339)
                        .unwrap_or_else(|_| d.as_secs().to_string())
                },
            )
    };

    let modified_at = fmt_time(meta.modified());
    let accessed_at = fmt_time(meta.accessed());

    let hints = build_hints(
        Some("fs.read"),
        Some("fs.hash"),
        Some("Use fs.read_dir for bulk metadata of directory children"),
        &deps.capabilities,
        false,
    );

    let content = format!(
        "USE: retrieve single-path metadata\nDOES: stat of {kind} at '{}'\nNEXT: fs.read, fs.hash\nAVOID: stat in a loop → use fs.read_dir",
        req.path
    );

    let structured_content = serde_json::json!({
        "tool": "fs.stat",
        "path": req.path,
        "size_bytes": size_bytes,
        "is_dir": is_dir,
        "is_file": is_file,
        "is_symlink": is_symlink,
        "kind": kind,
        "modified_at": modified_at,
        "accessed_at": accessed_at,
        "hints": hints,
    });

    ToolResponse::with_hints(content, structured_content, hints)
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
