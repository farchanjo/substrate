//! Handler for `archive.gzip.compress` — Bucket B, Zone A/B (ADR-0003 / ADR-0040).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: compress a single file with gzip into a sibling .gz file
//! DOES: reads the source file, compresses with flate2 GzEncoder, writes atomically
//!       to <dest>.gz via TmpPath; requires dry_run=true first
//! ARGS: source (string) — input file path within the allowlist;
//!       dest (string) — output .gz path within the allowlist;
//!       dry_run (bool, default true) — preview without writing;
//!       level (u32, default 6) — compression level 0–9
//! RETURNS: {source, dest, source_bytes, compressed_bytes, ratio} or dry-run preview
//! NEXT: archive.hash, archive.gzip.decompress
//! AVOID: compressing directories — use archive.tar.create with gzip compression
//! ```

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers::build_inline_hints;
use crate::response::{ArchiveDeps, ToolResponse};
use crate::tmp_path::TmpPath;

/// Input parameters for `archive.gzip.compress`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GzipCompressRequest {
    /// Source file path within the allowlist.
    /// Also accepts `src` as an alias for compatibility with step implementations.
    #[serde(alias = "src")]
    pub source: String,

    /// Destination `.gz` path within the allowlist.
    pub dest: String,

    /// When `true` (default), preview without writing.
    #[serde(default = "default_true")]
    pub dry_run: bool,

    /// Compression level 0–9. Default 6 (balanced speed/ratio).
    #[serde(default = "default_level")]
    pub level: u32,
}

const fn default_true() -> bool {
    true
}

const fn default_level() -> u32 {
    6
}

/// Handler for `archive.gzip.compress`.
///
/// Bucket B: auto-mode. Files above 128 KiB are dispatched via `spawn_blocking`.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, I/O, or dry-run size checks.
#[instrument(skip(deps, _cancel), fields(source = %req.source, dest = %req.dest, dry_run = req.dry_run))]
pub async fn handle_archive_gzip_compress(
    req: GzipCompressRequest,
    deps: &ArchiveDeps,
    _cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    // Dry-run gate.
    if req.dry_run {
        let source_meta = std::fs::metadata(&req.source).map_err(|_| SubstrateError::NotFound {
            resource: req.source.clone(),
            correlation_id: None,
        })?;
        let hints = build_inline_hints(Some("archive.hash"), None, &deps.capabilities, true);
        let content = format!(
            "USE: compress file with gzip\nDOES: dry-run; would compress '{}' ({} bytes) → '{}'; set dry_run=false to write\nNEXT: archive.gzip.compress (live)\nAVOID: compressing directories",
            req.source,
            source_meta.len(),
            req.dest
        );
        let structured_content = json!({
            "tool": "archive.gzip.compress",
            "dry_run": true,
            "source": req.source,
            "dest": req.dest,
            "source_bytes": source_meta.len(),
            "hints": hints,
        });
        return Ok(ToolResponse::with_hints(content, structured_content, hints));
    }

    // Jail paths.
    let source_path = std::path::PathBuf::from(&req.source);
    let jail = std::sync::Arc::clone(&deps.jail);
    let sp = source_path.clone();
    let jailed_source: JailedPath =
        tokio::task::spawn_blocking(move || jail.jail(&JailedPath::new_jailed(sp.clone()), &sp))
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: format!("spawn_blocking: {e}"),
                correlation_id: None,
            })??;

    let dest_path = std::path::PathBuf::from(&req.dest);
    let jail2 = std::sync::Arc::clone(&deps.jail);
    let dp = dest_path.clone();
    // The `.gz` output does not exist yet, so it cannot be canonicalized. Jail
    // its parent directory and reconstruct the dest beneath it (see
    // `crate::dest_jail`); jailing the dest directly returns SUBSTRATE_NOT_FOUND.
    let jailed_dest: JailedPath = tokio::task::spawn_blocking(move || {
        crate::dest_jail::jail_dest_via_parent(jail2.as_ref(), &dp)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking: {e}"),
        correlation_id: None,
    })??;

    let dest_final = jailed_dest.as_path().to_path_buf();
    let tmp = TmpPath::new_for(&dest_final);
    let tmp_path = tmp.tmp_path().to_path_buf();
    let level = req.level;

    let (source_bytes, compressed_bytes) =
        tokio::task::spawn_blocking(move || -> SubstrateResult<(u64, u64)> {
            compress_file_blocking(jailed_source.as_path(), &tmp_path, level)
        })
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking: {e}"),
            correlation_id: None,
        })??;

    tmp.commit().await.map_err(|_| SubstrateError::IoError {
        path: dest_final.to_string_lossy().into_owned(),
        correlation_id: None,
    })?;

    #[expect(
        clippy::cast_precision_loss,
        reason = "file sizes are large u64 values; f64 precision loss is acceptable for a display ratio"
    )]
    let ratio = if source_bytes > 0 {
        compressed_bytes as f64 / source_bytes as f64
    } else {
        1.0
    };

    let hints = build_inline_hints(Some("archive.hash"), None, &deps.capabilities, false);
    let content = format!(
        "USE: compress with gzip\nDOES: compressed '{}' ({source_bytes} bytes) → '{}' ({compressed_bytes} bytes, ratio {ratio:.2})\nNEXT: archive.hash\nAVOID: compressing directories",
        req.source, req.dest
    );
    let structured_content = json!({
        "tool": "archive.gzip.compress",
        "source": req.source,
        "dest": req.dest,
        "source_bytes": source_bytes,
        "compressed_bytes": compressed_bytes,
        "ratio": ratio,
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

fn compress_file_blocking(
    source: &std::path::Path,
    tmp_path: &std::path::Path,
    level: u32,
) -> SubstrateResult<(u64, u64)> {
    use std::io::Write as _;

    let data = std::fs::read(source).map_err(|e| {
        use std::io::ErrorKind;
        match e.kind() {
            ErrorKind::NotFound => SubstrateError::NotFound {
                resource: source.to_string_lossy().into_owned(),
                correlation_id: None,
            },
            ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
                path: source.to_string_lossy().into_owned(),
                correlation_id: None,
            },
            _ => SubstrateError::IoError {
                path: source.to_string_lossy().into_owned(),
                correlation_id: None,
            },
        }
    })?;
    let source_bytes = data.len() as u64;

    let level = flate2::Compression::new(level.min(9));
    let out_file = std::fs::File::create(tmp_path).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", tmp_path.display()),
        correlation_id: None,
    })?;
    let mut encoder = flate2::write::GzEncoder::new(out_file, level);
    encoder
        .write_all(&data)
        .map_err(|e| SubstrateError::IoError {
            path: format!("gzip encode: {e}"),
            correlation_id: None,
        })?;
    encoder.finish().map_err(|e| SubstrateError::IoError {
        path: format!("gzip finish: {e}"),
        correlation_id: None,
    })?;

    let compressed_bytes = std::fs::metadata(tmp_path).map_or(0, |m| m.len());
    Ok((source_bytes, compressed_bytes))
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
    async fn gzip_compress_creates_output() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("input.txt");
        std::fs::write(&src, b"substrate-gzip-compress-test").unwrap();
        let dest = tmp.path().join("input.txt.gz");
        let deps = make_deps();
        let req = GzipCompressRequest {
            source: src.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            level: 6,
        };
        let resp = handle_archive_gzip_compress(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert!(dest.exists());
        assert!(
            resp.structured_content["compressed_bytes"]
                .as_u64()
                .unwrap_or(0)
                > 0
        );
    }

    #[tokio::test]
    async fn dry_run_does_not_write() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("data.txt");
        std::fs::write(&src, b"dry-run data").unwrap();
        let dest = tmp.path().join("data.txt.gz");
        let deps = make_deps();
        let req = GzipCompressRequest {
            source: src.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: true,
            level: 6,
        };
        let resp = handle_archive_gzip_compress(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert!(
            resp.structured_content["dry_run"]
                .as_bool()
                .unwrap_or(false)
        );
        assert!(!dest.exists());
    }

    /// Jail double mirroring real behaviour: `canonicalize` fails on a
    /// non-existent path, so jailing a brand-new `.gz` dest directly errors.
    struct CanonicalizeJail;
    impl substrate_domain::PathJailPort for CanonicalizeJail {
        fn jail(&self, _: &JailedPath, raw: &std::path::Path) -> SubstrateResult<JailedPath> {
            let canon = std::fs::canonicalize(raw).map_err(|_| SubstrateError::NotFound {
                resource: raw.to_string_lossy().into_owned(),
                correlation_id: None,
            })?;
            Ok(JailedPath::new_jailed(canon))
        }
    }

    // Regression for the dest-path-jail bug: a live compress whose `.gz` output
    // does not exist yet must succeed by jailing the parent directory. Before
    // the fix the handler jailed the non-existent dest directly and returned
    // SUBSTRATE_NOT_FOUND.
    #[tokio::test]
    async fn live_compress_to_nonexistent_dest_succeeds_with_real_jail() {
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

        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("in.txt");
        std::fs::write(&src, b"substrate-dest-jail-regression").unwrap();
        let dest = tmp.path().join("in.txt.gz"); // does NOT exist yet

        let deps = ArchiveDeps {
            jail: Arc::new(CanonicalizeJail),
            hasher: Arc::new(NoopHasher),
            capabilities: Arc::new(Capabilities::default()),
        };
        let req = GzipCompressRequest {
            source: src.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            level: 6,
        };
        let resp = handle_archive_gzip_compress(req, &deps, CancellationToken::new())
            .await
            .expect("compress to a non-existent dest must succeed via parent-jail");
        assert!(dest.exists(), "output .gz must be written");
        assert!(
            resp.structured_content["compressed_bytes"]
                .as_u64()
                .unwrap_or(0)
                > 0
        );
    }
}
