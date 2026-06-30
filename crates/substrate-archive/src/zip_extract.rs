//! Handler for `archive.zip.extract` — Bucket C, Zone B+C (ADR-0040 / ADR-0003).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: unpack a ZIP archive into a jailed extraction directory
//! DOES: validates every member path (Zip Slip) and rejects symlink members before
//!       writing; requires dry_run=true first, then confirmed=true
//! ARGS: archive (string) — path to the .zip file within the allowlist;
//!       dest (string) — extraction root within the allowlist;
//!       dry_run (bool, default true) — preview without writing
//! RETURNS: ArchiveManifest (dry_run) or {extracted_count, dest}
//! NEXT: archive.hash, fs.find
//! AVOID: extracting untrusted archives without dry-run inspection
//! ```
//!
//! # Security
//!
//! Every member path is validated by [`zip_slip_guard::validate_member_path`]
//! and [`symlink_guard::reject_symlink_entry`] before any disk write.
//! Matches scenarios in:
//! - `archive-zip-extract-zip-slip-blocked.feature`
//! - `archive-symlink-member-blocked.feature`

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers::build_job_hints;
use crate::manifest::{ArchiveEntry, ArchiveManifest};
use crate::resource_limit::{
    DEFAULT_MAX_EXTRACT_TOTAL_BYTES, DEFAULT_MAX_OUTPUT_BYTES, DecompressGuard, check_disk_space,
};
use crate::response::{ArchiveDeps, ToolResponse};
use crate::symlink_guard::{EntryKind, reject_symlink_entry, validate_symlink_target};
use crate::tmp_path::{TmpPath, crockford_base32};
use crate::zip_slip_guard::validate_member_path;

/// Input parameters for `archive.zip.extract`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZipExtractRequest {
    /// Path to the ZIP archive within the allowlist.
    pub archive: String,

    /// Extraction root directory within the allowlist.
    pub dest: String,

    /// When `true` (default), preview without writing.
    #[serde(default = "default_true")]
    pub dry_run: bool,

    /// Required `true` for live extract after dry-run.
    #[serde(default)]
    pub confirmed: bool,
}

const fn default_true() -> bool {
    true
}

/// Handler for `archive.zip.extract`.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, security checks, or I/O.
#[instrument(skip(deps, cancel), fields(archive = %req.archive, dest = %req.dest, dry_run = req.dry_run))]
pub async fn handle_archive_zip_extract(
    req: ZipExtractRequest,
    deps: &ArchiveDeps,
    cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    // Jail archive path.
    let archive_path = std::path::PathBuf::from(&req.archive);
    let jail = std::sync::Arc::clone(&deps.jail);
    let ap = archive_path.clone();
    let jailed_archive: JailedPath =
        tokio::task::spawn_blocking(move || jail.jail(&JailedPath::new_jailed(ap.clone()), &ap))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking: {e}"),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })??;

    // Jail destination.
    let dest_path = std::path::PathBuf::from(&req.dest);
    let jail2 = std::sync::Arc::clone(&deps.jail);
    let dp = dest_path.clone();
    let jailed_dest: JailedPath =
        tokio::task::spawn_blocking(move || jail2.jail(&JailedPath::new_jailed(dp.clone()), &dp))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking: {e}"),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })??;

    if req.dry_run {
        return produce_dry_run(&jailed_archive, &jailed_dest, deps).await;
    }
    if !req.confirmed {
        return Err(SubstrateError::ConfirmationRequired {
            correlation_id: Some(uuid::Uuid::now_v7()),
        });
    }

    if cancel.is_cancelled() {
        return Err(SubstrateError::Cancelled {
            correlation_id: Some(uuid::Uuid::now_v7()),
        });
    }

    // Disk-space preflight (ADR-0033): the compressed archive size is a cheap
    // lower bound on the uncompressed payload landing in the destination.
    let archive_for_meta = jailed_archive.as_path().to_path_buf();
    let archive_size = tokio::task::spawn_blocking(move || {
        std::fs::metadata(&archive_for_meta).map_or(0u64, |m| m.len())
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    check_disk_space(jailed_dest.as_path(), archive_size).await?;

    let archive_clone = jailed_archive.as_path().to_path_buf();
    let dest_clone = jailed_dest.as_path().to_path_buf();

    let extracted_count =
        tokio::task::spawn_blocking(move || extract_zip_blocking(&archive_clone, &dest_clone))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking: {e}"),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })??;

    let hints = build_job_hints(None, Some("archive.hash"), &deps.capabilities, false);
    let content = format!(
        "USE: unpack ZIP archive\nDOES: extracted {extracted_count} entries to '{}'\nNEXT: archive.hash\nAVOID: overwriting without inspection",
        req.dest
    );
    let structured_content = json!({
        "tool": "archive.zip.extract",
        "archive": req.archive,
        "dest": req.dest,
        "extracted_count": extracted_count,
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

async fn produce_dry_run(
    jailed_archive: &JailedPath,
    jailed_dest: &JailedPath,
    deps: &ArchiveDeps,
) -> SubstrateResult<ToolResponse> {
    let archive_path = jailed_archive.as_path().to_path_buf();
    let dest_root = jailed_dest.as_path().to_path_buf();

    // The member scan opens and reads the archive — run it off the executor
    // (ADR-0003 async-zone B). The previous inline scan blocked the reactor.
    let entries = tokio::task::spawn_blocking(move || scan_zip_members(&archive_path, &dest_root))
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })??;

    let manifest = ArchiveManifest::from_entries(entries);
    let hints = build_job_hints(None, Some("archive.hash"), &deps.capabilities, true);
    let content = format!(
        "USE: preview ZIP extract\nDOES: dry-run; {} entries; set dry_run=false&&confirmed=true to extract\nNEXT: archive.zip.extract (live)\nAVOID: skipping Zip Slip dry-run validation",
        manifest.entry_count
    );
    let structured_content = serde_json::json!({
        "tool": "archive.zip.extract",
        "dry_run": true,
        "manifest": serde_json::Value::from(manifest),
        "hints": hints,
    });
    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

/// Scans ZIP members for the dry-run manifest without writing to disk.
///
/// Opens the archive, validates every member path (Zip Slip + symlink target),
/// and collects an [`ArchiveEntry`] per member. Runs inside `spawn_blocking`.
fn scan_zip_members(
    archive_path: &std::path::Path,
    dest_root: &std::path::Path,
) -> SubstrateResult<Vec<ArchiveEntry>> {
    let file = std::fs::File::open(archive_path).map_err(|_| SubstrateError::NotFound {
        resource: archive_path.to_string_lossy().into_owned(),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| SubstrateError::IoError {
        path: format!("zip open: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;

    let mut entries = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| SubstrateError::IoError {
            path: format!("zip entry {i}: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
        // Copy name to owned String to avoid borrow conflict when reading content.
        let name = entry.name().to_owned();
        let member = std::path::Path::new(&name);
        validate_member_path(dest_root, member)?;
        let kind = if entry.is_symlink() {
            EntryKind::Symlink
        } else if entry.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        let uncompressed = entry.size();
        // Symlink guard — ADR-0035 two-tier: validate target stays within root.
        if kind == EntryKind::Symlink {
            use std::io::Read as _;
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| SubstrateError::IoError {
                    path: format!("zip symlink content: {e}"),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                })?;
            let target_str =
                std::str::from_utf8(&buf).map_err(|e| SubstrateError::EncodingError {
                    detail: format!("zip symlink target utf8: {e}"),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                })?;
            let link_path = dest_root.join(member);
            validate_symlink_target(dest_root, &link_path, std::path::Path::new(target_str))?;
        } else {
            reject_symlink_entry(kind, &name)?;
        }
        entries.push(ArchiveEntry {
            archive_path: name,
            uncompressed_bytes: uncompressed,
            compression_method: "deflate".to_owned(),
            modified_at: None,
        });
    }
    Ok(entries)
}

/// Synchronous ZIP extraction inside `spawn_blocking`.
///
/// Extracts into a sibling staging directory `<dest>.tmp.<uuid7>` and atomically
/// renames it onto `dest_root` only after the full archive succeeds (ADR-0033).
/// On any error (`DecompressGuard` overflow, I/O, security rejection) the
/// staging tree is removed so no partial tree is ever visible at `dest_root`.
fn extract_zip_blocking(
    archive: &std::path::Path,
    dest_root: &std::path::Path,
) -> SubstrateResult<usize> {
    let staging = make_staging_dir(dest_root)?;

    let result = extract_zip_into_staging(archive, &staging);

    match result {
        Ok(count) => {
            commit_staging(&staging, dest_root)?;
            Ok(count)
        },
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(e)
        },
    }
}

/// Extracts every ZIP member into `staging`. All members are pre-validated
/// before any disk write (ADR-0035); files land in `staging` and are promoted
/// to the final destination only on full success by the caller.
fn extract_zip_into_staging(
    archive: &std::path::Path,
    staging: &std::path::Path,
) -> SubstrateResult<usize> {
    use std::io::Read as _;

    let file = std::fs::File::open(archive).map_err(|_| SubstrateError::NotFound {
        resource: archive.to_string_lossy().into_owned(),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| SubstrateError::IoError {
        path: format!("zip open: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;

    // Security-first: validate all members before any disk write (ADR-0035).
    zip_prevalidate_members(&mut zip, staging)?;

    // Aggregate ceiling across the whole archive guards against many-member
    // DEFLATE bombs whose individual entries stay below the per-entry limit.
    let mut total_guard = DecompressGuard::new(DEFAULT_MAX_EXTRACT_TOTAL_BYTES);

    // All validated: proceed with extraction.
    let mut extracted_count = 0usize;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| SubstrateError::IoError {
            path: format!("zip entry {i}: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
        let name = entry.name().to_owned();
        let member = std::path::Path::new(&name);
        let resolved = validate_member_path(staging, member)?;

        if entry.is_dir() {
            std::fs::create_dir_all(&resolved).map_err(|e| SubstrateError::IoError {
                path: format!("{}: {e}", resolved.display()),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })?;
        } else if entry.is_symlink() {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| SubstrateError::IoError {
                    path: format!("zip symlink content: {e}"),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                })?;
            let target_str =
                std::str::from_utf8(&buf).map_err(|e| SubstrateError::EncodingError {
                    detail: format!("zip symlink target utf8: {e}"),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                })?;
            zip_write_symlink(&resolved, target_str)?;
            extracted_count += 1;
        } else {
            zip_write_file(&mut entry, &resolved, &mut total_guard)?;
            extracted_count += 1;
        }
    }

    Ok(extracted_count)
}

/// Creates a sibling staging directory `<dest>.tmp.<uuid7>` for transactional
/// extraction. The parent of `dest_root` must already exist (it was jailed).
fn make_staging_dir(dest_root: &std::path::Path) -> SubstrateResult<std::path::PathBuf> {
    let suffix = crockford_base32(uuid::Uuid::now_v7().as_bytes());
    let dir_name = dest_root.file_name().and_then(|n| n.to_str()).map_or_else(
        || format!("extract.tmp.{suffix}"),
        |name| format!("{name}.tmp.{suffix}"),
    );
    let parent = dest_root
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let staging = parent.join(dir_name);
    std::fs::create_dir_all(&staging).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", staging.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    Ok(staging)
}

/// Atomically promotes the staging tree onto `dest_root`.
///
/// When `dest_root` already exists (the jail canonicalised an existing
/// directory), its contents are merged from the staging tree entry-by-entry so
/// the rename does not fail on a non-empty target.
fn commit_staging(staging: &std::path::Path, dest_root: &std::path::Path) -> SubstrateResult<()> {
    if std::fs::rename(staging, dest_root).is_ok() {
        return Ok(());
    }
    // Target exists / is non-empty: merge children, then drop the staging dir.
    merge_into(staging, dest_root)?;
    let _ = std::fs::remove_dir_all(staging);
    Ok(())
}

/// Recursively moves the children of `src` into `dst`, creating directories as
/// needed and renaming leaf entries.
fn merge_into(src: &std::path::Path, dst: &std::path::Path) -> SubstrateResult<()> {
    std::fs::create_dir_all(dst).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", dst.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    for entry in std::fs::read_dir(src).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", src.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })? {
        let entry = entry.map_err(|e| SubstrateError::IoError {
            path: format!("readdir: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() && to.exists() {
            merge_into(&from, &to)?;
        } else {
            std::fs::rename(&from, &to).map_err(|e| SubstrateError::IoError {
                path: format!("{}: {e}", to.display()),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })?;
        }
    }
    Ok(())
}

/// Pre-validates all ZIP members — Zip Slip + symlink escape checks — before any disk write.
///
/// For symlink members, the content (target path) is read to validate it stays within
/// `dest_root` per ADR-0035 §symlink-validation.  After individual validation, a second
/// pass detects symlink cycles between members (e.g., `a → b` and `b → a`), which
/// are blocked as `SUBSTRATE_PATH_TRAVERSAL_BLOCKED` to prevent infinite-loop extraction.
fn zip_prevalidate_members(
    zip: &mut zip::ZipArchive<std::fs::File>,
    dest_root: &std::path::Path,
) -> SubstrateResult<()> {
    use std::io::Read as _;

    // Collect (normalised_link_name, normalised_target_name) for cycle detection.
    let mut symlink_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| SubstrateError::IoError {
            path: format!("zip entry {i}: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
        let name = entry.name().to_owned();
        let member = std::path::Path::new(&name);
        validate_member_path(dest_root, member)?;
        let kind = if entry.is_symlink() {
            EntryKind::Symlink
        } else if entry.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        if kind == EntryKind::Symlink {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| SubstrateError::IoError {
                    path: format!("zip symlink content: {e}"),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                })?;
            let target_str =
                std::str::from_utf8(&buf).map_err(|e| SubstrateError::EncodingError {
                    detail: format!("zip symlink target utf8: {e}"),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                })?;
            let link_path = dest_root.join(member);
            validate_symlink_target(dest_root, &link_path, std::path::Path::new(target_str))?;
            // Record for cycle detection.  Normalise by stripping a leading `./` if present.
            let norm_name = name.trim_start_matches("./").to_owned();
            let norm_target = target_str.trim_start_matches("./").to_owned();
            symlink_map.insert(norm_name, norm_target);
        } else {
            reject_symlink_entry(kind, &name)?;
        }
    }

    // Cycle detection: for each symlink, follow the chain up to symlink_map.len() + 1 hops.
    // If we revisit a node we've seen in this chain, there is a cycle.
    for start in symlink_map.keys() {
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut current = start.as_str();
        let max_hops = symlink_map.len() + 1;
        for _ in 0..max_hops {
            if !visited.insert(current.to_owned()) {
                // Cycle detected — two or more archive members form a symlink loop.
                return Err(SubstrateError::PathTraversalBlocked {
                    path: format!("symlink loop involving member: {current}"),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                });
            }
            match symlink_map.get(current) {
                Some(next) => current = next.as_str(),
                None => break, // target is not itself a symlink source — no cycle via this path.
            }
        }
    }

    Ok(())
}

/// Creates a validated symlink at `resolved` pointing to `target_str` (ADR-0004 §symlink-restore).
///
/// The caller MUST have already validated `target_str` via [`validate_symlink_target`].
fn zip_write_symlink(resolved: &std::path::Path, target_str: &str) -> SubstrateResult<()> {
    if let Some(parent) = resolved.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SubstrateError::IoError {
            path: format!("{}: {e}", parent.display()),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target_str, resolved).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", resolved.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    #[cfg(windows)]
    {
        let _ = (target_str, resolved);
        return Err(SubstrateError::InternalError {
            reason: "symlink extraction not supported on Windows".to_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        });
    }
    Ok(())
}

/// Writes a regular ZIP file entry to `resolved` using a transactional tmp rename (ADR-0033).
///
/// Streams the (potentially DEFLATE-bomb) entry in bounded 64 KiB chunks rather
/// than reading it fully into the heap, recording every chunk against a
/// per-entry [`DecompressGuard`] and the archive-wide `total_guard`. Either
/// overflow aborts extraction with `SUBSTRATE_RESOURCE_LIMIT` (fix-1).
fn zip_write_file(
    entry: &mut zip::read::ZipFile<'_>,
    resolved: &std::path::Path,
    total_guard: &mut DecompressGuard,
) -> SubstrateResult<()> {
    use std::io::Read as _;
    use std::io::Write as _;

    if let Some(parent) = resolved.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SubstrateError::IoError {
            path: format!("{}: {e}", parent.display()),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
    }
    let mut entry_guard = DecompressGuard::new(DEFAULT_MAX_OUTPUT_BYTES);
    let tmp = TmpPath::new_for(resolved);
    {
        let mut out =
            std::fs::File::create(tmp.tmp_path()).map_err(|e| SubstrateError::IoError {
                path: format!("{}: {e}", tmp.tmp_path().display()),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = entry.read(&mut buf).map_err(|e| SubstrateError::IoError {
                path: format!("zip read entry: {e}"),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })?;
            if n == 0 {
                break;
            }
            entry_guard.record(n as u64)?;
            total_guard.record(n as u64)?;
            out.write_all(&buf[..n])
                .map_err(|e| SubstrateError::IoError {
                    path: format!("{}: {e}", resolved.display()),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                })?;
        }
    }
    std::fs::rename(tmp.tmp_path(), tmp.final_path()).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", resolved.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    std::mem::forget(tmp);
    Ok(())
}

// ---- Tests -------------------------------------------------------------------

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

    fn make_deps() -> ArchiveDeps {
        use substrate_domain::{Capabilities, HashPort, ports::hash::Blake3Digest};
        struct NoopHasher;
        impl HashPort for NoopHasher {
            fn hash_file(&self, _: &JailedPath) -> SubstrateResult<Blake3Digest> {
                Ok(Blake3Digest::new([0u8; 32]))
            }
            fn hash_bytes(&self, _: &[u8]) -> Blake3Digest {
                Blake3Digest::new([0u8; 32])
            }
        }
        ArchiveDeps {
            jail: Arc::new(NoopJail),
            hasher: Arc::new(NoopHasher),
            capabilities: Arc::new(Capabilities::default()),
        }
    }

    fn create_test_zip(archive: &std::path::Path, files: &[(&str, &[u8])]) {
        use std::io::Write as _;
        use zip::CompressionMethod;
        use zip::write::SimpleFileOptions;

        let file = std::fs::File::create(archive).unwrap();
        let mut writer = zip::write::ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, data) in files {
            writer.start_file(*name, opts).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    #[tokio::test]
    async fn zip_extract_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("test.zip");
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        create_test_zip(&archive, &[("hello.txt", b"substrate-zip-extract-test")]);

        let deps = make_deps();
        let req = ZipExtractRequest {
            archive: archive.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            confirmed: true,
        };
        let resp = handle_archive_zip_extract(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        let out = dest.join("hello.txt");
        assert!(out.exists());
        assert_eq!(std::fs::read(&out).unwrap(), b"substrate-zip-extract-test");
        assert_eq!(
            resp.structured_content["extracted_count"]
                .as_u64()
                .unwrap_or(0),
            1
        );
    }

    #[tokio::test]
    async fn zip_slip_blocked_dotdot() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("evil.zip");
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        create_test_zip(&archive, &[("../evil.txt", b"bad")]);

        let deps = make_deps();
        let req = ZipExtractRequest {
            archive: archive.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            confirmed: true,
        };
        let err = handle_archive_zip_extract(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, SubstrateError::PathTraversalBlocked { .. }));
        assert!(!tmp.path().join("evil.txt").exists());
    }

    #[tokio::test]
    async fn dry_run_returns_manifest_without_writes() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("a.zip");
        let dest = tmp.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        create_test_zip(&archive, &[("file.txt", b"data")]);

        let deps = make_deps();
        let req = ZipExtractRequest {
            archive: archive.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: true,
            confirmed: false,
        };
        let resp = handle_archive_zip_extract(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert!(
            resp.structured_content["dry_run"]
                .as_bool()
                .unwrap_or(false)
        );
        assert!(!dest.join("file.txt").exists());
    }
}
