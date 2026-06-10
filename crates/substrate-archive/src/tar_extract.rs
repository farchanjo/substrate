//! Handler for `archive.tar.extract` — Bucket C, Zone B+C (ADR-0040 / ADR-0003).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: unpack a TAR archive into a jailed destination directory
//! DOES: streams entries from a TAR file, validates every path (Zip Slip / symlink),
//!       then writes each entry atomically; requires dry_run=true first
//! ARGS: archive (string) — path to the .tar or .tar.gz file;
//!       dest (string) — extraction root within the allowlist;
//!       dry_run (bool, default true) — preview without writing
//! RETURNS: ArchiveManifest (dry_run) or {extracted_count, dest}
//! NEXT: archive.hash, fs.find
//! AVOID: extracting untrusted archives without dry-run inspection
//! ```
//!
//! # Security
//!
//! Every entry path is validated by [`zip_slip_guard::validate_member_path`]
//! and [`symlink_guard::reject_symlink_entry`] before any disk write.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
use crate::tmp_path::crockford_base32;
use crate::zip_slip_guard::validate_member_path;

/// Input parameters for `archive.tar.extract`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TarExtractRequest {
    /// Path to the TAR (or TAR.GZ) archive within the allowlist.
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

/// Handler for `archive.tar.extract`.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, I/O, or security checks.
#[instrument(skip(deps, cancel), fields(archive = %req.archive, dest = %req.dest, dry_run = req.dry_run))]
pub async fn handle_archive_tar_extract(
    req: TarExtractRequest,
    deps: &ArchiveDeps,
    cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    // Jail archive path.
    let archive_path = std::path::PathBuf::from(&req.archive);
    let jail = std::sync::Arc::clone(&deps.jail);
    let ap_clone = archive_path.clone();
    let jailed_archive: JailedPath = tokio::task::spawn_blocking(move || {
        jail.jail(&JailedPath::new_jailed(ap_clone.clone()), &ap_clone)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })??;

    // Jail destination directory.
    let dest_path = std::path::PathBuf::from(&req.dest);
    let jail2 = std::sync::Arc::clone(&deps.jail);
    let dp_clone = dest_path.clone();
    let jailed_dest: JailedPath = tokio::task::spawn_blocking(move || {
        jail2.jail(&JailedPath::new_jailed(dp_clone.clone()), &dp_clone)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })??;

    // Dry-run gate.
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
    // lower bound on the uncompressed payload that will land in the staging dir.
    let archive_for_meta = jailed_archive.as_path().to_path_buf();
    let archive_size = tokio::task::spawn_blocking(move || {
        std::fs::metadata(&archive_for_meta).map_or(0u64, |m| m.len())
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    check_disk_space(jailed_dest.as_path(), archive_size).await?;

    let archive_clone = jailed_archive.as_path().to_path_buf();
    let dest_clone = jailed_dest.as_path().to_path_buf();
    // ADR-0037: bridge the async CancellationToken into the blocking extract via
    // an `Arc<AtomicBool>`. A permit/token cannot cross into `spawn_blocking`, so
    // the flag is flipped by an async watcher and polled at every entry boundary.
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let watcher_flag = Arc::clone(&cancel_flag);
    let watch_cancel = cancel.clone();
    let watcher = tokio::spawn(async move {
        watch_cancel.cancelled().await;
        watcher_flag.store(true, Ordering::SeqCst);
    });

    let blocking_flag = Arc::clone(&cancel_flag);
    let extract_result = tokio::task::spawn_blocking(move || -> SubstrateResult<usize> {
        extract_tar_blocking(&archive_clone, &dest_clone, &blocking_flag)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    });
    watcher.abort();

    let extracted_count = extract_result??;

    let hints = build_job_hints(None, Some("archive.hash"), &deps.capabilities, false);
    let content = format!(
        "USE: unpack TAR archive\nDOES: extracted {extracted_count} entries to '{}'\nNEXT: archive.hash\nAVOID: extracting without subsequent hash verification",
        req.dest
    );
    let structured_content = json!({
        "tool": "archive.tar.extract",
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

    // The member scan performs blocking file I/O — run it off the executor
    // (ADR-0003 async-zone B). The previous inline scan blocked the reactor.
    let entries = tokio::task::spawn_blocking(move || scan_tar_members(&archive_path, &dest_root))
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })??;
    let manifest = ArchiveManifest::from_entries(entries);

    let hints = build_job_hints(None, Some("archive.hash"), &deps.capabilities, true);
    let content = format!(
        "USE: preview TAR extract\nDOES: dry-run; {} entries; set dry_run=false&&confirmed=true to extract\nNEXT: archive.tar.extract (live)\nAVOID: skipping confirmation",
        manifest.entry_count
    );
    let structured_content = serde_json::json!({
        "tool": "archive.tar.extract",
        "dry_run": true,
        "manifest": serde_json::Value::from(manifest),
        "hints": hints,
    });
    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

/// Scans TAR members for dry-run without writing to disk.
fn scan_tar_members(
    archive: &std::path::Path,
    dest_root: &std::path::Path,
) -> SubstrateResult<Vec<ArchiveEntry>> {
    let file = std::fs::File::open(archive).map_err(|_| SubstrateError::NotFound {
        resource: archive.to_string_lossy().into_owned(),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    let mut ar = tar::Archive::new(file);
    let mut entries = Vec::new();

    for entry_result in ar.entries().map_err(|e| SubstrateError::IoError {
        path: format!("tar entries: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })? {
        let entry = entry_result.map_err(|e| SubstrateError::IoError {
            path: format!("tar entry: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
        let header = entry.header();
        let member_path = entry.path().map_err(|e| SubstrateError::EncodingError {
            detail: format!("tar entry path: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;

        // Zip Slip guard.
        validate_member_path(dest_root, &member_path)?;

        // Symlink guard — ADR-0035 two-tier validation:
        // safe (target stays within root) → allowed; escaping → PathTraversalBlocked.
        // Hard links (`EntryType::Link`) reference another path and are treated
        // as symlinks for validation so they cannot escape the extraction root.
        let kind = classify_entry(header.entry_type());
        if kind == EntryKind::Symlink {
            // For dry-run, validate the target without creating the link.
            let link_target = header
                .link_name()
                .map_err(|e| SubstrateError::EncodingError {
                    detail: format!("tar link target: {e}"),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                })?
                .unwrap_or_default();
            let link_path = dest_root.join(&*member_path);
            validate_symlink_target(dest_root, &link_path, &link_target)?;
        } else {
            reject_symlink_entry(kind, &member_path.to_string_lossy())?;
        }

        entries.push(ArchiveEntry {
            archive_path: member_path.to_string_lossy().into_owned(),
            uncompressed_bytes: header.size().unwrap_or(0),
            compression_method: "none".to_owned(),
            modified_at: None,
        });
    }

    Ok(entries)
}

/// Maps a `tar::EntryType` to the guard's `EntryKind`.
///
/// Hard links (`Link`) reference another in-archive path; like symlinks they are
/// routed through target validation so a crafted entry cannot escape the root
/// (ADR-0035). Devices, FIFOs, and other special types map to `Other` and are
/// skipped during extraction.
fn classify_entry(entry_type: tar::EntryType) -> EntryKind {
    match entry_type {
        tar::EntryType::Symlink | tar::EntryType::Link => EntryKind::Symlink,
        tar::EntryType::Regular | tar::EntryType::Continuous => EntryKind::File,
        tar::EntryType::Directory => EntryKind::Directory,
        _ => EntryKind::Other,
    }
}

/// Returns `true` when the first two bytes of `archive` are the gzip magic
/// (`0x1f 0x8b`), independent of file extension. Falls back to the extension
/// hint when the file cannot be probed.
fn is_gzip_archive(archive: &std::path::Path) -> bool {
    use std::io::Read as _;

    if let Ok(mut f) = std::fs::File::open(archive) {
        let mut magic = [0u8; 2];
        if f.read_exact(&mut magic).is_ok() {
            return magic == [0x1f, 0x8b];
        }
    }
    archive
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gz") || e.eq_ignore_ascii_case("tgz"))
}

/// Synchronous TAR extraction inside `spawn_blocking`.
///
/// Extracts into a sibling staging directory `<dest>.tmp.<uuid7>` and atomically
/// renames it onto `dest_root` only after the full archive succeeds (ADR-0033).
/// On any error or cancellation the staging tree is removed so no partial tree
/// is ever visible at `dest_root`.
fn extract_tar_blocking(
    archive: &std::path::Path,
    dest_root: &std::path::Path,
    cancel: &Arc<AtomicBool>,
) -> SubstrateResult<usize> {
    let staging = make_staging_dir(dest_root)?;

    let result = (|| -> SubstrateResult<usize> {
        let file = std::fs::File::open(archive).map_err(|_| SubstrateError::NotFound {
            resource: archive.to_string_lossy().into_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
        // Detect gzip by magic bytes, not extension (ADR-0035 defense in depth).
        if is_gzip_archive(archive) {
            let decoder = flate2::read::GzDecoder::new(file);
            let mut ar = tar::Archive::new(decoder);
            extract_entries(&mut ar, &staging, cancel)
        } else {
            let mut ar = tar::Archive::new(file);
            extract_entries(&mut ar, &staging, cancel)
        }
    })();

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

/// Creates a sibling staging directory `<dest>.tmp.<uuid7>` for transactional
/// extraction. The parent of `dest_root` must already exist (it was jailed).
fn make_staging_dir(dest_root: &std::path::Path) -> SubstrateResult<std::path::PathBuf> {
    let suffix = crockford_base32(uuid::Uuid::now_v7().as_bytes());
    let dir_name = match dest_root.file_name().and_then(|n| n.to_str()) {
        Some(name) => format!("{name}.tmp.{suffix}"),
        None => format!("extract.tmp.{suffix}"),
    };
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

fn extract_entries<R: std::io::Read>(
    ar: &mut tar::Archive<R>,
    dest_root: &std::path::Path,
    cancel: &Arc<AtomicBool>,
) -> SubstrateResult<usize> {
    // Aggregate ceiling across the whole archive guards against many-member
    // bombs whose individual entries each stay below the per-entry limit.
    let mut total_guard = DecompressGuard::new(DEFAULT_MAX_EXTRACT_TOTAL_BYTES);
    let mut count = 0usize;
    for entry_result in ar.entries().map_err(|e| SubstrateError::IoError {
        path: format!("tar entries: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })? {
        // ADR-0037: poll cancellation at each entry boundary.
        if cancel.load(Ordering::SeqCst) {
            return Err(SubstrateError::Cancelled {
                correlation_id: Some(uuid::Uuid::now_v7()),
            });
        }
        let mut entry = entry_result.map_err(|e| SubstrateError::IoError {
            path: format!("tar entry read: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
        let member_path = entry.path().map_err(|e| SubstrateError::EncodingError {
            detail: format!("tar entry path: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;

        // Zip Slip guard (Tar Slip).
        let resolved = validate_member_path(dest_root, &member_path)?;
        let kind = classify_entry(entry.header().entry_type());

        if kind == EntryKind::Directory {
            create_dir_all_mapped(&resolved)?;
        } else if kind == EntryKind::Symlink {
            extract_symlink_entry(&mut entry, dest_root, &resolved)?;
        } else if kind == EntryKind::File {
            extract_file_entry(&mut entry, &resolved, &mut total_guard)?;
        } else {
            // Devices, FIFOs, and other special types are skipped.
            continue;
        }
        count += 1;
    }
    Ok(count)
}

/// Creates a directory tree, mapping I/O failures to [`SubstrateError::IoError`].
fn create_dir_all_mapped(path: &std::path::Path) -> SubstrateResult<()> {
    std::fs::create_dir_all(path).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", path.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })
}

/// Validates and restores a symlink (or hard-linked) member (ADR-0004 §symlink-validation).
fn extract_symlink_entry<R: std::io::Read>(
    entry: &mut tar::Entry<'_, R>,
    dest_root: &std::path::Path,
    resolved: &std::path::Path,
) -> SubstrateResult<()> {
    let link_target = entry
        .header()
        .link_name()
        .map_err(|e| SubstrateError::EncodingError {
            detail: format!("tar link target: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?
        .unwrap_or_default();
    validate_symlink_target(dest_root, resolved, &link_target)?;
    if let Some(parent) = resolved.parent() {
        create_dir_all_mapped(parent)?;
    }
    // The link target is validated above — it cannot escape extraction_root.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&*link_target, resolved).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", resolved.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    #[cfg(windows)]
    {
        let _ = link_target;
        return Err(SubstrateError::InternalError {
            reason: "symlink extraction not supported on Windows".to_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        });
    }
    Ok(())
}

/// Streams a regular file member to disk in bounded chunks, recording every
/// chunk against `total_guard` (per-entry + aggregate ceiling) and committing
/// via a transactional tmp rename (ADR-0033 / fix-1 zip-bomb guard).
fn extract_file_entry<R: std::io::Read>(
    entry: &mut tar::Entry<'_, R>,
    resolved: &std::path::Path,
    total_guard: &mut DecompressGuard,
) -> SubstrateResult<()> {
    use crate::tmp_path::TmpPath;
    use std::io::{Read as _, Write as _};

    if let Some(parent) = resolved.parent() {
        create_dir_all_mapped(parent)?;
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
                path: format!("tar entry data: {e}"),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })?;
            if n == 0 {
                break;
            }
            // Per-entry then aggregate ceiling: either overflow aborts extraction.
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
    // Prevent Drop from removing the now-renamed file.
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
        use substrate_domain::{HashPort, ports::hash::Blake3Digest};

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
            capabilities: Arc::new(substrate_domain::Capabilities::default()),
        }
    }

    fn create_test_tar(archive_path: &std::path::Path, files: &[(&str, &[u8])]) {
        let file = std::fs::File::create(archive_path).unwrap();
        let mut builder = tar::Builder::new(file);
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            let name_bytes = name.as_bytes();
            assert!(
                name_bytes.len() < 100,
                "test path exceeds GNU tar name field"
            );
            header.as_old_mut().name[..name_bytes.len()].copy_from_slice(name_bytes);
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.finish().unwrap();
    }

    #[tokio::test]
    async fn tar_extract_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("test.tar");
        let dest = tmp.path().join("extracted");
        std::fs::create_dir_all(&dest).unwrap();
        create_test_tar(&archive, &[("hello.txt", b"substrate-tar-test")]);

        let deps = make_deps();
        let req = TarExtractRequest {
            archive: archive.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            confirmed: true,
        };
        let resp = handle_archive_tar_extract(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        let extracted_file = dest.join("hello.txt");
        assert!(extracted_file.exists());
        assert_eq!(
            std::fs::read(&extracted_file).unwrap(),
            b"substrate-tar-test"
        );
        assert_eq!(
            resp.structured_content["extracted_count"]
                .as_u64()
                .unwrap_or(0),
            1
        );
    }

    #[tokio::test]
    async fn symlink_member_in_tar_is_blocked() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("evil.tar");
        let dest = tmp.path().join("extracted");
        std::fs::create_dir_all(&dest).unwrap();

        // Build a TAR with a symlink entry.
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut builder = tar::Builder::new(file);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            // link_name points outside; member path is inside extraction root.
            builder
                .append_link(&mut header, "innocent.txt", "/etc/passwd")
                .unwrap();
            builder.finish().unwrap();
        }

        let deps = make_deps();
        let req = TarExtractRequest {
            archive: archive.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            confirmed: true,
        };
        let err = handle_archive_tar_extract(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        // Absolute symlink targets escape the extraction root — PathTraversalBlocked per ADR-0035.
        assert!(
            matches!(err, SubstrateError::PathTraversalBlocked { .. }),
            "expected PathTraversalBlocked, got: {err:?}"
        );
        assert!(!dest.join("innocent.txt").exists());
    }

    #[tokio::test]
    async fn tar_slip_dotdot_is_blocked() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("slip.tar");
        let dest = tmp.path().join("extracted");
        std::fs::create_dir_all(&dest).unwrap();

        create_test_tar(&archive, &[("../escape.txt", b"bad")]);

        let deps = make_deps();
        let req = TarExtractRequest {
            archive: archive.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            confirmed: true,
        };
        let err = handle_archive_tar_extract(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, SubstrateError::PathTraversalBlocked { .. }));
        assert!(!tmp.path().join("escape.txt").exists());
    }

    #[tokio::test]
    async fn dry_run_returns_manifest_without_writing() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("test.tar");
        let dest = tmp.path().join("extracted");
        std::fs::create_dir_all(&dest).unwrap();
        create_test_tar(&archive, &[("file.txt", b"data")]);

        let deps = make_deps();
        let req = TarExtractRequest {
            archive: archive.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: true,
            confirmed: false,
        };
        let resp = handle_archive_tar_extract(req, &deps, CancellationToken::new())
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
