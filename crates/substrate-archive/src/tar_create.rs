//! Handler for `archive.tar.create` — Bucket C, Zone B+C (ADR-0040 / ADR-0003).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: bundle a directory tree into a TAR archive for backup or transfer
//! DOES: creates a TAR archive (optionally gzip-compressed) from jailed source paths;
//!       requires dry_run=true first, then confirmed=true
//! ARGS: sources (string[]) — jailed paths to include;
//!       dest (string) — output archive path within the allowlist;
//!       compression ("none"|"gzip") — compression algorithm;
//!       dry_run (bool, default true) — preview without writing
//! RETURNS: ArchiveManifest (dry_run) or {archive_path, entry_count, size_bytes}
//! NEXT: archive.hash, archive.tar.extract
//! AVOID: passing paths outside the allowlist — use fs.find first
//! ```
//!
//! # Security
//!
//! - Source paths validated through `PathJailPort`.
//! - Destination written via `TmpPath` atomic rename (ADR-0033).
//! - Dry-run gate: first call MUST have `dry_run=true`; live write requires
//!   `dry_run=false && confirmed=true`.
//! - `CancellationToken` honoured at entry boundaries.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers::build_job_hints;
use crate::manifest::{ArchiveEntry, ArchiveManifest};
use crate::response::{ArchiveDeps, ToolResponse};
use crate::tmp_path::TmpPath;

/// Compression algorithm for TAR archives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TarCompression {
    /// No compression — plain `.tar`.
    #[default]
    None,
    /// gzip compression — `.tar.gz`.
    Gzip,
}

impl std::fmt::Display for TarCompression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Gzip => f.write_str("gzip"),
        }
    }
}

/// Input parameters for `archive.tar.create`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TarCreateRequest {
    /// Jailed source paths to include in the archive.
    pub sources: Vec<String>,

    /// Destination archive path (must be within the allowlist).
    pub dest: String,

    /// Compression algorithm.
    #[serde(default)]
    pub compression: TarCompression,

    /// When `true` (default), preview without writing.
    #[serde(default = "default_true")]
    pub dry_run: bool,

    /// Required `true` for live write after dry-run preview.
    #[serde(default)]
    pub confirmed: bool,
}

const fn default_true() -> bool {
    true
}

/// Handler for `archive.tar.create`.
///
/// Bucket C: always dispatched as an async job. For MVP the handler runs
/// synchronously inside `spawn_blocking`; the job control-plane integration
/// is wired by `substrate-mcp-server`.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, dry-run gate,
/// confirmation gate, or synchronous TAR I/O.
#[instrument(skip(deps, cancel), fields(dest = %req.dest, compression = %req.compression, dry_run = req.dry_run))]
pub async fn handle_archive_tar_create(
    req: TarCreateRequest,
    deps: &ArchiveDeps,
    cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    // Dry-run gate (ADR-0004 / ADR-0033).
    if req.dry_run {
        return Ok(produce_dry_run(&req, deps));
    }

    if !req.confirmed {
        return Err(SubstrateError::ConfirmationRequired {
            correlation_id: None,
        });
    }

    // Jail the destination path. The archive does not exist yet, so it cannot be
    // canonicalized: jail the parent directory (which must exist) and reconstruct
    // the dest beneath it (see `crate::dest_jail`).
    let dest_path = std::path::PathBuf::from(&req.dest);
    let jail = std::sync::Arc::clone(&deps.jail);
    let dp = dest_path.clone();
    let jailed_dest: JailedPath = tokio::task::spawn_blocking(move || {
        crate::dest_jail::jail_dest_via_parent(jail.as_ref(), &dp)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: None,
    })??;

    // Jail all source paths.
    let mut jailed_sources: Vec<JailedPath> = Vec::with_capacity(req.sources.len());
    for src in &req.sources {
        let src_path = std::path::PathBuf::from(src);
        let jail2 = std::sync::Arc::clone(&deps.jail);
        let src_clone = src_path.clone();
        let jailed = tokio::task::spawn_blocking(move || {
            jail2.jail(&JailedPath::new_jailed(src_clone.clone()), &src_clone)
        })
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error: {e}"),
            correlation_id: None,
        })??;
        jailed_sources.push(jailed);
    }

    let compression = req.compression;
    let dest_final = jailed_dest.as_path().to_path_buf();
    let tmp = TmpPath::new_for(&dest_final);
    let tmp_path = tmp.tmp_path().to_path_buf();
    let sources_snapshot = jailed_sources.clone();

    // Zone B: dispatch I/O-heavy tar creation in spawn_blocking.
    let (entry_count, archive_bytes) =
        tokio::task::spawn_blocking(move || -> SubstrateResult<_> {
            build_tar_blocking(&tmp_path, &sources_snapshot, compression)
        })
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error: {e}"),
            correlation_id: None,
        })??;

    // Check cancellation before committing (ADR-0037).
    if cancel.is_cancelled() {
        drop(tmp); // cleanup temp file
        return Err(SubstrateError::Cancelled {
            correlation_id: None,
        });
    }

    tmp.commit().await.map_err(|_| SubstrateError::IoError {
        path: dest_final.to_string_lossy().into_owned(),
        correlation_id: None,
    })?;

    let hints = build_job_hints(None, Some("archive.hash"), &deps.capabilities, false);
    let content = format!(
        "USE: bundle sources into TAR archive\nDOES: created '{}' ({entry_count} entries, {archive_bytes} bytes)\nNEXT: archive.hash\nAVOID: re-archiving without hash verification",
        req.dest
    );
    let structured_content = json!({
        "tool": "archive.tar.create",
        "archive_path": req.dest,
        "entry_count": entry_count,
        "size_bytes": archive_bytes,
        "compression": compression.to_string(),
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

/// Dry-run pass: compute the manifest without touching disk.
fn produce_dry_run(req: &TarCreateRequest, deps: &ArchiveDeps) -> ToolResponse {
    let entries: Vec<ArchiveEntry> = req
        .sources
        .iter()
        .filter_map(|s| {
            let path = Path::new(s);
            let meta = std::fs::metadata(path).ok()?;
            Some(ArchiveEntry {
                archive_path: path
                    .file_name()
                    .map_or_else(|| s.clone(), |n| n.to_string_lossy().into_owned()),
                uncompressed_bytes: meta.len(),
                compression_method: req.compression.to_string(),
                modified_at: None,
            })
        })
        .collect();

    let manifest = ArchiveManifest::from_entries(entries);
    let hints = build_job_hints(None, Some("archive.hash"), &deps.capabilities, true);
    let content = format!(
        "USE: preview TAR create\nDOES: dry-run; {} entries totalling {} bytes; set dry_run=false&&confirmed=true to write\nNEXT: archive.tar.create (live)\nAVOID: skipping confirmation",
        manifest.entry_count, manifest.total_uncompressed_bytes
    );
    let structured_content = serde_json::json!({
        "tool": "archive.tar.create",
        "dry_run": true,
        "manifest": serde_json::Value::from(manifest),
        "hints": hints,
    });
    ToolResponse::with_hints(content, structured_content, hints)
}

/// Synchronous TAR creation running inside `spawn_blocking`.
///
/// Returns `(entry_count, archive_size_bytes)`.
fn build_tar_blocking(
    tmp_path: &std::path::Path,
    sources: &[JailedPath],
    compression: TarCompression,
) -> SubstrateResult<(usize, u64)> {
    let file = std::fs::File::create(tmp_path).map_err(|_| SubstrateError::IoError {
        path: tmp_path.to_string_lossy().into_owned(),
        correlation_id: None,
    })?;

    let mut entry_count = 0usize;

    match compression {
        TarCompression::None => {
            let mut builder = tar::Builder::new(file);
            for src in sources {
                let path = src.as_path();
                if path.is_dir() {
                    builder
                        .append_dir_all(
                            path.file_name()
                                .map_or(path, std::path::Path::new),
                            path,
                        )
                        .map_err(|e| SubstrateError::IoError {
                            path: format!("{}: {e}", path.display()),
                            correlation_id: None,
                        })?;
                } else {
                    let mut f = std::fs::File::open(path).map_err(|_| SubstrateError::IoError {
                        path: path.to_string_lossy().into_owned(),
                        correlation_id: None,
                    })?;
                    builder
                        .append_file(
                            path.file_name()
                                .map_or(path, std::path::Path::new),
                            &mut f,
                        )
                        .map_err(|e| SubstrateError::IoError {
                            path: format!("{}: {e}", path.display()),
                            correlation_id: None,
                        })?;
                }
                entry_count += 1;
            }
            builder.finish().map_err(|e| SubstrateError::IoError {
                path: format!("tar finish: {e}"),
                correlation_id: None,
            })?;
        },
        TarCompression::Gzip => {
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            for src in sources {
                let path = src.as_path();
                if path.is_dir() {
                    builder
                        .append_dir_all(
                            path.file_name()
                                .map_or(path, std::path::Path::new),
                            path,
                        )
                        .map_err(|e| SubstrateError::IoError {
                            path: format!("{}: {e}", path.display()),
                            correlation_id: None,
                        })?;
                } else {
                    let mut f = std::fs::File::open(path).map_err(|_| SubstrateError::IoError {
                        path: path.to_string_lossy().into_owned(),
                        correlation_id: None,
                    })?;
                    builder
                        .append_file(
                            path.file_name()
                                .map_or(path, std::path::Path::new),
                            &mut f,
                        )
                        .map_err(|e| SubstrateError::IoError {
                            path: format!("{}: {e}", path.display()),
                            correlation_id: None,
                        })?;
                }
                entry_count += 1;
            }
            let encoder = builder.into_inner().map_err(|e| SubstrateError::IoError {
                path: format!("tar finish: {e}"),
                correlation_id: None,
            })?;
            encoder.finish().map_err(|e| SubstrateError::IoError {
                path: format!("gz finish: {e}"),
                correlation_id: None,
            })?;
        },
    }

    let archive_bytes = std::fs::metadata(tmp_path).map_or(0, |m| m.len());
    Ok((entry_count, archive_bytes))
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
        use substrate_domain::{Capabilities, HashPort};

        struct NoopHasher;
        impl HashPort for NoopHasher {
            fn hash_file(
                &self,
                _: &JailedPath,
            ) -> SubstrateResult<substrate_domain::ports::hash::Blake3Digest> {
                Ok(substrate_domain::ports::hash::Blake3Digest::new([0u8; 32]))
            }
            fn hash_bytes(&self, _: &[u8]) -> substrate_domain::ports::hash::Blake3Digest {
                substrate_domain::ports::hash::Blake3Digest::new([0u8; 32])
            }
        }

        ArchiveDeps {
            jail: Arc::new(NoopJail),
            hasher: Arc::new(NoopHasher),
            capabilities: Arc::new(Capabilities::default()),
        }
    }

    #[tokio::test]
    async fn dry_run_returns_manifest() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("a.txt");
        std::fs::write(&src, b"hello").unwrap();
        let deps = make_deps();
        let req = TarCreateRequest {
            sources: vec![src.to_string_lossy().into_owned()],
            dest: tmp.path().join("out.tar").to_string_lossy().into_owned(),
            compression: TarCompression::None,
            dry_run: true,
            confirmed: false,
        };
        let resp = handle_archive_tar_create(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert!(
            resp.structured_content["dry_run"]
                .as_bool()
                .unwrap_or(false)
        );
    }

    #[tokio::test]
    async fn live_tar_create_and_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("data.txt");
        std::fs::write(&src, b"substrate-archive-tar-test").unwrap();
        let archive = tmp.path().join("out.tar");
        let deps = make_deps();
        let req = TarCreateRequest {
            sources: vec![src.to_string_lossy().into_owned()],
            dest: archive.to_string_lossy().into_owned(),
            compression: TarCompression::None,
            dry_run: false,
            confirmed: true,
        };
        let resp = handle_archive_tar_create(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert!(archive.exists());
        assert!(resp.structured_content["entry_count"].as_u64().unwrap_or(0) >= 1);
    }

    #[tokio::test]
    async fn live_write_without_confirmed_returns_error() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("x.txt");
        std::fs::write(&src, b"x").unwrap();
        let deps = make_deps();
        let req = TarCreateRequest {
            sources: vec![src.to_string_lossy().into_owned()],
            dest: tmp.path().join("out.tar").to_string_lossy().into_owned(),
            compression: TarCompression::None,
            dry_run: false,
            confirmed: false,
        };
        let err = handle_archive_tar_create(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, SubstrateError::ConfirmationRequired { .. }));
    }
}
