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

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers::build_job_hints;
use crate::manifest::{ArchiveEntry, ArchiveManifest};
use crate::response::{ArchiveDeps, ToolResponse};
use crate::symlink_guard::{EntryKind, reject_symlink_entry};
use crate::zip_slip_guard::validate_member_path;

/// Input parameters for `archive.tar.extract`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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
        correlation_id: None,
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
        correlation_id: None,
    })??;

    // Dry-run gate.
    if req.dry_run {
        return produce_dry_run(&req, &jailed_archive, &jailed_dest, deps);
    }

    if !req.confirmed {
        return Err(SubstrateError::ConfirmationRequired {
            correlation_id: None,
        });
    }

    if cancel.is_cancelled() {
        return Err(SubstrateError::Cancelled {
            correlation_id: None,
        });
    }

    let archive_clone = jailed_archive.as_path().to_path_buf();
    let dest_clone = jailed_dest.as_path().to_path_buf();

    let extracted_count = tokio::task::spawn_blocking(move || -> SubstrateResult<usize> {
        extract_tar_blocking(&archive_clone, &dest_clone)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: None,
    })??;

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

fn produce_dry_run(
    _req: &TarExtractRequest,
    jailed_archive: &JailedPath,
    jailed_dest: &JailedPath,
    deps: &ArchiveDeps,
) -> SubstrateResult<ToolResponse> {
    let archive_path = jailed_archive.as_path();
    let dest_root = jailed_dest.as_path();

    let entries = scan_tar_members(archive_path, dest_root)?;
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
        correlation_id: None,
    })?;
    let mut ar = tar::Archive::new(file);
    let mut entries = Vec::new();

    for entry_result in ar.entries().map_err(|e| SubstrateError::IoError {
        path: format!("tar entries: {e}"),
        correlation_id: None,
    })? {
        let entry = entry_result.map_err(|e| SubstrateError::IoError {
            path: format!("tar entry: {e}"),
            correlation_id: None,
        })?;
        let header = entry.header();
        let member_path = entry.path().map_err(|e| SubstrateError::EncodingError {
            detail: format!("tar entry path: {e}"),
            correlation_id: None,
        })?;

        // Zip Slip guard.
        validate_member_path(dest_root, &member_path)?;

        // Symlink guard.
        let kind = match header.entry_type() {
            tar::EntryType::Symlink => EntryKind::Symlink,
            tar::EntryType::Regular | tar::EntryType::Continuous => EntryKind::File,
            tar::EntryType::Directory => EntryKind::Directory,
            _ => EntryKind::Other,
        };
        reject_symlink_entry(kind, &member_path.to_string_lossy())?;

        entries.push(ArchiveEntry {
            archive_path: member_path.to_string_lossy().into_owned(),
            uncompressed_bytes: header.size().unwrap_or(0),
            compression_method: "none".to_owned(),
            modified_at: None,
        });
    }

    Ok(entries)
}

/// Synchronous TAR extraction inside `spawn_blocking`.
fn extract_tar_blocking(
    archive: &std::path::Path,
    dest_root: &std::path::Path,
) -> SubstrateResult<usize> {
    let file = std::fs::File::open(archive).map_err(|_| SubstrateError::NotFound {
        resource: archive.to_string_lossy().into_owned(),
        correlation_id: None,
    })?;

    // Detect gzip by trying to read a gzip header lexically.
    let is_gz = archive
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gz") || e.eq_ignore_ascii_case("tgz"));

    let extracted_count = if is_gz {
        let decoder = flate2::read::GzDecoder::new(file);
        let mut ar = tar::Archive::new(decoder);
        extract_entries(&mut ar, dest_root)?
    } else {
        let mut ar = tar::Archive::new(file);
        extract_entries(&mut ar, dest_root)?
    };

    Ok(extracted_count)
}

fn extract_entries<R: std::io::Read>(
    ar: &mut tar::Archive<R>,
    dest_root: &std::path::Path,
) -> SubstrateResult<usize> {
    let mut count = 0usize;
    for entry_result in ar.entries().map_err(|e| SubstrateError::IoError {
        path: format!("tar entries: {e}"),
        correlation_id: None,
    })? {
        let mut entry = entry_result.map_err(|e| SubstrateError::IoError {
            path: format!("tar entry read: {e}"),
            correlation_id: None,
        })?;
        let member_path = entry.path().map_err(|e| SubstrateError::EncodingError {
            detail: format!("tar entry path: {e}"),
            correlation_id: None,
        })?;

        // Zip Slip guard (Tar Slip).
        let resolved = validate_member_path(dest_root, &member_path)?;

        // Symlink guard.
        let kind = match entry.header().entry_type() {
            tar::EntryType::Symlink => EntryKind::Symlink,
            tar::EntryType::Regular | tar::EntryType::Continuous => EntryKind::File,
            tar::EntryType::Directory => EntryKind::Directory,
            _ => EntryKind::Other,
        };
        reject_symlink_entry(kind, &member_path.to_string_lossy())?;

        if kind == EntryKind::Directory {
            std::fs::create_dir_all(&resolved).map_err(|e| SubstrateError::IoError {
                path: format!("{}: {e}", resolved.display()),
                correlation_id: None,
            })?;
        } else {
            // Transactional write via TmpPath.
            use crate::tmp_path::TmpPath;
            if let Some(parent) = resolved.parent() {
                std::fs::create_dir_all(parent).map_err(|e| SubstrateError::IoError {
                    path: format!("{}: {e}", parent.display()),
                    correlation_id: None,
                })?;
            }
            let tmp = TmpPath::new_for(&resolved);
            {
                let mut out =
                    std::fs::File::create(tmp.tmp_path()).map_err(|e| SubstrateError::IoError {
                        path: format!("{}: {e}", tmp.tmp_path().display()),
                        correlation_id: None,
                    })?;
                std::io::copy(&mut entry, &mut out).map_err(|e| SubstrateError::IoError {
                    path: format!("{}: {e}", resolved.display()),
                    correlation_id: None,
                })?;
            }
            // Sync commit (blocking context).
            std::fs::rename(tmp.tmp_path(), tmp.final_path()).map_err(|e| {
                SubstrateError::IoError {
                    path: format!("{}: {e}", resolved.display()),
                    correlation_id: None,
                }
            })?;
            // Prevent Drop from removing the now-renamed file.
            std::mem::forget(tmp);
        }
        count += 1;
    }
    Ok(count)
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
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
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
        assert!(
            matches!(err, SubstrateError::SymlinkEscape { .. }),
            "expected SymlinkEscape, got: {err:?}"
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
