//! Handler for `archive.zip.create` — Bucket C, Zone B+C (ADR-0040 / ADR-0003).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: create a ZIP archive using deflate compression
//! DOES: bundles jailed source paths into a ZIP; CRC32 via crc32fast (SIMD CLMUL);
//!       requires dry_run=true first, then confirmed=true
//! ARGS: sources (string[]) — jailed paths to include;
//!       dest (string) — output .zip path within the allowlist;
//!       dry_run (bool, default true) — preview without writing
//! RETURNS: ArchiveManifest (dry_run) or {archive_path, entry_count, size_bytes}
//! NEXT: archive.hash, archive.zip.extract
//! AVOID: passing paths outside the allowlist
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers::build_job_hints;
use crate::manifest::{ArchiveEntry, ArchiveManifest};
use crate::resource_limit::{DEFAULT_MAX_OUTPUT_BYTES, DecompressGuard, check_disk_space};
use crate::response::{ArchiveDeps, ToolResponse};
use crate::tmp_path::TmpPath;

/// Bounded read-buffer size for streaming source files into the ZIP writer.
const ZIP_CHUNK_BYTES: usize = 64 * 1024;

/// Input parameters for `archive.zip.create`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZipCreateRequest {
    /// Jailed source paths to include in the archive.
    pub sources: Vec<String>,

    /// Destination archive path within the allowlist.
    pub dest: String,

    /// When `true` (default), preview without writing.
    #[serde(default = "default_true")]
    pub dry_run: bool,

    /// Required `true` for live write after dry-run.
    #[serde(default)]
    pub confirmed: bool,
}

const fn default_true() -> bool {
    true
}

/// Handler for `archive.zip.create`.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, I/O, or cancellation.
#[instrument(skip(deps, cancel), fields(dest = %req.dest, dry_run = req.dry_run))]
pub async fn handle_archive_zip_create(
    req: ZipCreateRequest,
    deps: &ArchiveDeps,
    cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    if req.dry_run {
        return produce_dry_run(&req, deps).await;
    }
    if !req.confirmed {
        return Err(SubstrateError::ConfirmationRequired {
            correlation_id: Some(uuid::Uuid::now_v7()),
        });
    }

    // Jail destination. The `.zip` output does not exist yet, so it cannot be
    // canonicalized: jail its parent directory and reconstruct the dest beneath
    // it (see `crate::dest_jail`). Jailing the dest directly returned
    // SUBSTRATE_NOT_FOUND for every brand-new archive path.
    let dest_path = std::path::PathBuf::from(&req.dest);
    let jail = std::sync::Arc::clone(&deps.jail);
    let dp_clone = dest_path.clone();
    let jailed_dest: JailedPath = tokio::task::spawn_blocking(move || {
        crate::dest_jail::jail_dest_via_parent(jail.as_ref(), &dp_clone)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })??;

    // Jail sources.
    let mut jailed_sources = Vec::with_capacity(req.sources.len());
    for src in &req.sources {
        let sp = std::path::PathBuf::from(src);
        let jail2 = std::sync::Arc::clone(&deps.jail);
        let sc = sp.clone();
        let jailed = tokio::task::spawn_blocking(move || {
            jail2.jail(&JailedPath::new_jailed(sc.clone()), &sc)
        })
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })??;
        jailed_sources.push(jailed);
    }

    // Disk-space preflight (ADR-0033): sum of source sizes is a lower bound on
    // the archive's footprint in the destination filesystem.
    let sources_for_size = jailed_sources.clone();
    let total_source_bytes =
        tokio::task::spawn_blocking(move || sum_source_bytes(&sources_for_size))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking: {e}"),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })?;
    if let Some(parent) = jailed_dest.as_path().parent() {
        check_disk_space(parent, total_source_bytes).await?;
    }

    let dest_final = jailed_dest.as_path().to_path_buf();
    let tmp = TmpPath::new_for(&dest_final);
    let tmp_path = tmp.tmp_path().to_path_buf();
    let sources_snapshot = jailed_sources.clone();

    // ADR-0037: bridge the async CancellationToken into the blocking build via an
    // `Arc<AtomicBool>` polled at every source boundary.
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let watcher_flag = Arc::clone(&cancel_flag);
    let watch_cancel = cancel.clone();
    let watcher = tokio::spawn(async move {
        watch_cancel.cancelled().await;
        watcher_flag.store(true, Ordering::SeqCst);
    });
    let blocking_flag = Arc::clone(&cancel_flag);

    let build = tokio::task::spawn_blocking(move || -> SubstrateResult<(usize, u64)> {
        build_zip_blocking(&tmp_path, &sources_snapshot, &blocking_flag)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    });
    watcher.abort();

    let (entry_count, archive_bytes) = build??;

    if cancel.is_cancelled() {
        drop(tmp);
        return Err(SubstrateError::Cancelled {
            correlation_id: Some(uuid::Uuid::now_v7()),
        });
    }

    tmp.commit().await.map_err(|_| SubstrateError::IoError {
        path: dest_final.to_string_lossy().into_owned(),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;

    let hints = build_job_hints(None, Some("archive.hash"), &deps.capabilities, false);
    let content = format!(
        "USE: bundle sources into ZIP archive\nDOES: created '{}' ({entry_count} entries, {archive_bytes} bytes)\nNEXT: archive.hash\nAVOID: re-archiving without integrity check",
        req.dest
    );
    let structured_content = json!({
        "tool": "archive.zip.create",
        "archive_path": req.dest,
        "entry_count": entry_count,
        "size_bytes": archive_bytes,
        "compression": "deflate",
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

/// Dry-run pass: build the manifest after jailing each source path.
///
/// Source paths are jailed BEFORE any `metadata` syscall so the preview cannot
/// disclose the existence or size of host files outside the allowlist. The jail
/// runs inside `spawn_blocking` per ADR-0003.
async fn produce_dry_run(
    req: &ZipCreateRequest,
    deps: &ArchiveDeps,
) -> SubstrateResult<ToolResponse> {
    let mut entries: Vec<ArchiveEntry> = Vec::with_capacity(req.sources.len());
    for src in &req.sources {
        let jail = Arc::clone(&deps.jail);
        let raw = std::path::PathBuf::from(src);
        if let Some(entry) =
            tokio::task::spawn_blocking(move || jailed_dry_run_entry(jail.as_ref(), &raw))
                .await
                .map_err(|e| SubstrateError::InternalError {
                    reason: format!("spawn_blocking: {e}"),
                    correlation_id: Some(uuid::Uuid::now_v7()),
                })??
        {
            entries.push(entry);
        }
    }

    let manifest = ArchiveManifest::from_entries(entries);
    let hints = build_job_hints(None, Some("archive.hash"), &deps.capabilities, true);
    let content = format!(
        "USE: preview ZIP create\nDOES: dry-run; {} entries; set dry_run=false&&confirmed=true to write\nNEXT: archive.zip.create (live)\nAVOID: skipping dry-run",
        manifest.entry_count
    );
    let structured_content = serde_json::json!({
        "tool": "archive.zip.create",
        "dry_run": true,
        "manifest": serde_json::Value::from(manifest),
        "hints": hints,
    });
    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

/// Jails a single source path, then stats it for the dry-run manifest.
///
/// Returns `Ok(None)` when the jailed path cannot be stat'd.
///
/// # Errors
///
/// Propagates any jail error (path outside allowlist, traversal, symlink escape).
fn jailed_dry_run_entry(
    jail: &dyn substrate_domain::PathJailPort,
    raw: &std::path::Path,
) -> SubstrateResult<Option<ArchiveEntry>> {
    let jailed = jail.jail(&JailedPath::new_jailed(raw.to_path_buf()), raw)?;
    let path = jailed.as_path();
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(None);
    };
    Ok(Some(ArchiveEntry {
        archive_path: path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        ),
        uncompressed_bytes: meta.len(),
        compression_method: "deflate".to_owned(),
        modified_at: None,
    }))
}

/// Sums the on-disk sizes of all jailed sources (files and directory trees) for
/// the disk-space preflight. Unreadable entries contribute zero.
fn sum_source_bytes(sources: &[JailedPath]) -> u64 {
    sources
        .iter()
        .map(|s| dir_or_file_size(s.as_path()))
        .fold(0u64, u64::saturating_add)
}

fn dir_or_file_size(path: &std::path::Path) -> u64 {
    if path.is_dir() {
        std::fs::read_dir(path).map_or(0u64, |rd| {
            rd.flatten()
                .map(|e| dir_or_file_size(&e.path()))
                .fold(0u64, u64::saturating_add)
        })
    } else {
        std::fs::metadata(path).map_or(0u64, |m| m.len())
    }
}

/// Synchronous ZIP creation inside `spawn_blocking`.
///
/// Uses `zip::write::ZipWriter` with `Deflated` compression. CRC32 is handled by
/// the `zip` crate internally (backed by `crc32fast`). Each source file is
/// streamed in [`ZIP_CHUNK_BYTES`] chunks (never read fully into the heap) and
/// guarded by a per-file [`DecompressGuard`] ceiling. Cancellation is polled at
/// every top-level source boundary (ADR-0037).
fn build_zip_blocking(
    tmp_path: &std::path::Path,
    sources: &[JailedPath],
    cancel: &Arc<AtomicBool>,
) -> SubstrateResult<(usize, u64)> {
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    let file = std::fs::File::create(tmp_path).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", tmp_path.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    let mut writer = zip::write::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mut entry_count = 0usize;

    for src in sources {
        if cancel.load(Ordering::SeqCst) {
            return Err(SubstrateError::Cancelled {
                correlation_id: Some(uuid::Uuid::now_v7()),
            });
        }
        let path = src.as_path();
        if path.is_dir() {
            add_dir_to_zip(&mut writer, path, path, &options, &mut entry_count)?;
        } else {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            stream_file_into_zip(&mut writer, path, &name, options)?;
            entry_count += 1;
        }
    }

    writer.finish().map_err(|e| SubstrateError::IoError {
        path: format!("zip finish: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;

    let archive_bytes = std::fs::metadata(tmp_path).map_or(0, |m| m.len());
    Ok((entry_count, archive_bytes))
}

fn add_dir_to_zip(
    writer: &mut zip::write::ZipWriter<std::fs::File>,
    base: &std::path::Path,
    dir: &std::path::Path,
    options: &zip::write::SimpleFileOptions,
    entry_count: &mut usize,
) -> SubstrateResult<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", dir.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })? {
        let entry = entry.map_err(|e| SubstrateError::IoError {
            path: format!("readdir: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
        let path = entry.path();
        let relative = path.strip_prefix(base).unwrap_or(&path);
        let name = relative.to_string_lossy().into_owned();

        if path.is_dir() {
            add_dir_to_zip(writer, base, &path, options, entry_count)?;
        } else {
            stream_file_into_zip(writer, &path, &name, *options)?;
            *entry_count += 1;
        }
    }
    Ok(())
}

/// Streams a single file into the ZIP writer in bounded chunks, enforcing a
/// per-file output ceiling via [`DecompressGuard`] (fix-7 unbounded-read guard).
fn stream_file_into_zip(
    writer: &mut zip::write::ZipWriter<std::fs::File>,
    path: &std::path::Path,
    name: &str,
    options: zip::write::SimpleFileOptions,
) -> SubstrateResult<()> {
    use std::io::{Read as _, Write as _};

    writer
        .start_file(name, options)
        .map_err(|e| SubstrateError::IoError {
            path: format!("{name}: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;
    let mut in_file = std::fs::File::open(path).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", path.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;
    let mut guard = DecompressGuard::new(DEFAULT_MAX_OUTPUT_BYTES);
    let mut buf = vec![0u8; ZIP_CHUNK_BYTES];
    loop {
        let n = in_file
            .read(&mut buf)
            .map_err(|e| SubstrateError::IoError {
                path: format!("{}: {e}", path.display()),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })?;
        if n == 0 {
            break;
        }
        guard.record(n as u64)?;
        writer
            .write_all(&buf[..n])
            .map_err(|e| SubstrateError::IoError {
                path: format!("{name}: {e}"),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })?;
    }
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

    #[tokio::test]
    async fn zip_create_and_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("data.txt");
        std::fs::write(&src, b"substrate-zip-test").unwrap();
        let archive = tmp.path().join("out.zip");
        let deps = make_deps();
        let req = ZipCreateRequest {
            sources: vec![src.to_string_lossy().into_owned()],
            dest: archive.to_string_lossy().into_owned(),
            dry_run: false,
            confirmed: true,
        };
        let resp = handle_archive_zip_create(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert!(archive.exists());
        assert!(resp.structured_content["entry_count"].as_u64().unwrap_or(0) >= 1);
    }

    #[tokio::test]
    async fn dry_run_returns_manifest() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("b.txt");
        std::fs::write(&src, b"hi").unwrap();
        let deps = make_deps();
        let req = ZipCreateRequest {
            sources: vec![src.to_string_lossy().into_owned()],
            dest: tmp.path().join("out.zip").to_string_lossy().into_owned(),
            dry_run: true,
            confirmed: false,
        };
        let resp = handle_archive_zip_create(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert!(
            resp.structured_content["dry_run"]
                .as_bool()
                .unwrap_or(false)
        );
    }
}
