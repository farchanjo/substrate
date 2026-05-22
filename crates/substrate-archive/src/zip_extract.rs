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
use crate::response::{ArchiveDeps, ToolResponse};
use crate::symlink_guard::{EntryKind, reject_symlink_entry};
use crate::tmp_path::TmpPath;
use crate::zip_slip_guard::validate_member_path;

/// Input parameters for `archive.zip.extract`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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
                correlation_id: None,
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
                correlation_id: None,
            })??;

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

    let extracted_count =
        tokio::task::spawn_blocking(move || extract_zip_blocking(&archive_clone, &dest_clone))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking: {e}"),
                correlation_id: None,
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

fn produce_dry_run(
    _req: &ZipExtractRequest,
    jailed_archive: &JailedPath,
    jailed_dest: &JailedPath,
    deps: &ArchiveDeps,
) -> SubstrateResult<ToolResponse> {
    let archive_path = jailed_archive.as_path();
    let dest_root = jailed_dest.as_path();

    let file = std::fs::File::open(archive_path).map_err(|_| SubstrateError::NotFound {
        resource: archive_path.to_string_lossy().into_owned(),
        correlation_id: None,
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| SubstrateError::IoError {
        path: format!("zip open: {e}"),
        correlation_id: None,
    })?;

    let mut entries = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|e| SubstrateError::IoError {
            path: format!("zip entry {i}: {e}"),
            correlation_id: None,
        })?;
        let member = std::path::Path::new(entry.name());
        validate_member_path(dest_root, member)?;
        let kind = if entry.is_symlink() {
            EntryKind::Symlink
        } else if entry.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        reject_symlink_entry(kind, entry.name())?;
        entries.push(ArchiveEntry {
            archive_path: entry.name().to_owned(),
            uncompressed_bytes: entry.size(),
            compression_method: "deflate".to_owned(),
            modified_at: None,
        });
    }

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

/// Synchronous ZIP extraction inside `spawn_blocking`.
fn extract_zip_blocking(
    archive: &std::path::Path,
    dest_root: &std::path::Path,
) -> SubstrateResult<usize> {
    use std::io::Read as _;
    use std::io::Write as _;

    let file = std::fs::File::open(archive).map_err(|_| SubstrateError::NotFound {
        resource: archive.to_string_lossy().into_owned(),
        correlation_id: None,
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| SubstrateError::IoError {
        path: format!("zip open: {e}"),
        correlation_id: None,
    })?;

    // Pre-validate all member paths before any writes.
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|e| SubstrateError::IoError {
            path: format!("zip entry {i}: {e}"),
            correlation_id: None,
        })?;
        let member = std::path::Path::new(entry.name());
        validate_member_path(dest_root, member)?;
        let kind = if entry.is_symlink() {
            EntryKind::Symlink
        } else if entry.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        reject_symlink_entry(kind, entry.name())?;
    }

    // All validated: proceed with extraction.
    let mut extracted_count = 0usize;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| SubstrateError::IoError {
            path: format!("zip entry {i}: {e}"),
            correlation_id: None,
        })?;
        let member = std::path::Path::new(entry.name());
        let resolved = validate_member_path(dest_root, member)?;

        if entry.is_dir() {
            std::fs::create_dir_all(&resolved).map_err(|e| SubstrateError::IoError {
                path: format!("{}: {e}", resolved.display()),
                correlation_id: None,
            })?;
        } else {
            if let Some(parent) = resolved.parent() {
                std::fs::create_dir_all(parent).map_err(|e| SubstrateError::IoError {
                    path: format!("{}: {e}", parent.display()),
                    correlation_id: None,
                })?;
            }
            // Transactional write.
            let tmp = TmpPath::new_for(&resolved);
            {
                let mut out =
                    std::fs::File::create(tmp.tmp_path()).map_err(|e| SubstrateError::IoError {
                        path: format!("{}: {e}", tmp.tmp_path().display()),
                        correlation_id: None,
                    })?;
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .map_err(|e| SubstrateError::IoError {
                        path: format!("zip read entry: {e}"),
                        correlation_id: None,
                    })?;
                out.write_all(&buf).map_err(|e| SubstrateError::IoError {
                    path: format!("{}: {e}", resolved.display()),
                    correlation_id: None,
                })?;
            }
            std::fs::rename(tmp.tmp_path(), tmp.final_path()).map_err(|e| {
                SubstrateError::IoError {
                    path: format!("{}: {e}", resolved.display()),
                    correlation_id: None,
                }
            })?;
            std::mem::forget(tmp);
            extracted_count += 1;
        }
    }

    Ok(extracted_count)
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
