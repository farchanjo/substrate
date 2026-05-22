//! Handler for `archive.gzip.decompress` — Bucket B, Zone A/B (ADR-0003 / ADR-0040).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: decompress a single .gz file back to the original content
//! DOES: reads the .gz file, decompresses via flate2 GzDecoder, writes atomically;
//!       enforces a configurable maximum output size to guard against gzip-bombs;
//!       requires dry_run=true first
//! ARGS: source (string) — .gz file path within the allowlist;
//!       dest (string) — output path within the allowlist;
//!       dry_run (bool, default true) — preview without writing;
//!       max_output_bytes (u64, default 104857600) — gzip-bomb ceiling
//! RETURNS: {source, dest, compressed_bytes, decompressed_bytes} or dry-run preview
//! NEXT: archive.hash, fs.read
//! AVOID: decompressing untrusted files without resource limit — set max_output_bytes
//! ```
//!
//! # Security
//!
//! Enforces [`resource_limit::DecompressGuard`] to prevent gzip-bomb / unbounded
//! expansion. Matches scenario in
//! `docs/arch/specs/features/archive/archive-gzip-large-input-resource-limit.feature`.

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::hints_helpers::build_inline_hints;
use crate::resource_limit::{DEFAULT_MAX_OUTPUT_BYTES, DecompressGuard};
use crate::response::{ArchiveDeps, ToolResponse};
use crate::tmp_path::TmpPath;

/// Input parameters for `archive.gzip.decompress`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GzipDecompressRequest {
    /// Source `.gz` file path within the allowlist.
    pub source: String,

    /// Destination output path within the allowlist.
    pub dest: String,

    /// When `true` (default), preview without writing.
    #[serde(default = "default_true")]
    pub dry_run: bool,

    /// Maximum allowed decompressed output size in bytes. Default: 100 MiB.
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: u64,
}

const fn default_true() -> bool {
    true
}

const fn default_max_output_bytes() -> u64 {
    DEFAULT_MAX_OUTPUT_BYTES
}

/// Handler for `archive.gzip.decompress`.
///
/// Bucket B: auto-mode. Dispatches via `spawn_blocking` for non-trivial files.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, I/O, or resource-limit checks.
#[instrument(skip(deps, _cancel), fields(source = %req.source, dest = %req.dest, dry_run = req.dry_run))]
pub async fn handle_archive_gzip_decompress(
    req: GzipDecompressRequest,
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
            "USE: decompress gzip file\nDOES: dry-run; would decompress '{}' ({} compressed bytes) → '{}'; max_output_bytes={}\nNEXT: archive.gzip.decompress (live)\nAVOID: skipping resource limit",
            req.source,
            source_meta.len(),
            req.dest,
            req.max_output_bytes
        );
        let structured_content = json!({
            "tool": "archive.gzip.decompress",
            "dry_run": true,
            "source": req.source,
            "dest": req.dest,
            "compressed_bytes": source_meta.len(),
            "max_output_bytes": req.max_output_bytes,
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
    // The decompressed output does not exist yet, so it cannot be canonicalized.
    // Jail its parent directory and reconstruct the dest beneath it (see
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
    let max_bytes = req.max_output_bytes;

    let (compressed_bytes, decompressed_bytes) =
        tokio::task::spawn_blocking(move || -> SubstrateResult<(u64, u64)> {
            decompress_file_blocking(jailed_source.as_path(), &tmp_path, max_bytes)
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

    let hints = build_inline_hints(
        Some("archive.hash"),
        Some("fs.read"),
        &deps.capabilities,
        false,
    );
    let content = format!(
        "USE: decompress gzip file\nDOES: decompressed '{}' ({compressed_bytes} bytes) → '{}' ({decompressed_bytes} bytes)\nNEXT: archive.hash\nAVOID: decompressing without integrity check",
        req.source, req.dest
    );
    let structured_content = json!({
        "tool": "archive.gzip.decompress",
        "source": req.source,
        "dest": req.dest,
        "compressed_bytes": compressed_bytes,
        "decompressed_bytes": decompressed_bytes,
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
}

fn decompress_file_blocking(
    source: &std::path::Path,
    tmp_path: &std::path::Path,
    max_bytes: u64,
) -> SubstrateResult<(u64, u64)> {
    use std::io::{Read as _, Write as _};

    let compressed_bytes = std::fs::metadata(source).map_or(0, |m| m.len());

    let in_file = std::fs::File::open(source).map_err(|e| {
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

    let mut decoder = flate2::read::GzDecoder::new(in_file);
    let out_file = std::fs::File::create(tmp_path).map_err(|e| SubstrateError::IoError {
        path: format!("{}: {e}", tmp_path.display()),
        correlation_id: None,
    })?;
    let mut out = std::io::BufWriter::new(out_file);
    let mut guard = DecompressGuard::new(max_bytes);

    let chunk_size = 64 * 1024usize;
    let mut buf = vec![0u8; chunk_size];
    loop {
        let n = decoder
            .read(&mut buf)
            .map_err(|e| SubstrateError::IoError {
                path: format!("gzip decode: {e}"),
                correlation_id: None,
            })?;
        if n == 0 {
            break;
        }
        guard.record(n as u64)?;
        out.write_all(&buf[..n])
            .map_err(|e| SubstrateError::IoError {
                path: format!("{}: {e}", tmp_path.display()),
                correlation_id: None,
            })?;
    }

    out.flush().map_err(|e| SubstrateError::IoError {
        path: format!("flush: {e}"),
        correlation_id: None,
    })?;

    Ok((compressed_bytes, guard.written()))
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

    fn create_gz(path: &std::path::Path, data: &[u8]) {
        use std::io::Write as _;
        let f = std::fs::File::create(path).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap();
    }

    #[tokio::test]
    async fn gzip_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let gz = tmp.path().join("data.txt.gz");
        let dest = tmp.path().join("data.txt");
        create_gz(&gz, b"substrate-gzip-decompress-test");

        let deps = make_deps();
        let req = GzipDecompressRequest {
            source: gz.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        };
        let resp = handle_archive_gzip_decompress(req, &deps, CancellationToken::new())
            .await
            .unwrap();
        assert!(dest.exists());
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"substrate-gzip-decompress-test"
        );
        assert!(
            resp.structured_content["decompressed_bytes"]
                .as_u64()
                .unwrap_or(0)
                > 0
        );
    }

    #[tokio::test]
    async fn gzip_bomb_guard_triggers() {
        let tmp = TempDir::new().unwrap();
        let gz = tmp.path().join("bomb.gz");
        // Compress 10 KiB of zeros.
        let data = vec![0u8; 10 * 1024];
        create_gz(&gz, &data);

        let dest = tmp.path().join("bomb.txt");
        let deps = make_deps();
        // Set limit to 1 KiB — well below the 10 KiB decompressed output.
        let req = GzipDecompressRequest {
            source: gz.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            max_output_bytes: 1024,
        };
        let err = handle_archive_gzip_decompress(req, &deps, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, SubstrateError::ResourceLimit { .. }));
        assert_eq!(err.code(), "SUBSTRATE_RESOURCE_LIMIT");
    }

    #[tokio::test]
    async fn dry_run_does_not_write() {
        let tmp = TempDir::new().unwrap();
        let gz = tmp.path().join("data.gz");
        create_gz(&gz, b"dry-run data");
        let dest = tmp.path().join("data.txt");
        let deps = make_deps();
        let req = GzipDecompressRequest {
            source: gz.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: true,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        };
        let resp = handle_archive_gzip_decompress(req, &deps, CancellationToken::new())
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
    /// non-existent path, so jailing a brand-new output dest directly errors.
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

    #[tokio::test]
    async fn gzip_large_stream_respects_resource_limit() {
        // Validates that a gzip input larger than max_output_bytes is rejected
        // quickly (within 5 seconds) without allocating the full stream.
        // We use 100 KiB of zeros compressed to a small .gz, then set
        // max_output_bytes = 1 KiB so the guard triggers early.
        let tmp = TempDir::new().unwrap();
        let gz = tmp.path().join("large.gz");
        let data = vec![0u8; 100 * 1024]; // 100 KiB zeros
        create_gz(&gz, &data);
        let dest = tmp.path().join("large.out");
        let deps = make_deps();

        let req = GzipDecompressRequest {
            source: gz.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            max_output_bytes: 1024, // 1 KiB limit
        };

        // Must complete within 5 seconds and must return ResourceLimit.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            handle_archive_gzip_decompress(req, &deps, CancellationToken::new()),
        )
        .await
        .expect("gzip resource-limit guard must complete within 5 seconds");

        let err = result.unwrap_err();
        assert!(
            matches!(err, SubstrateError::ResourceLimit { .. }),
            "expected ResourceLimit, got: {err:?}"
        );
    }

    // Regression for the dest-path-jail bug: a live decompress whose output
    // does not exist yet must succeed by jailing the parent directory.
    #[tokio::test]
    async fn live_decompress_to_nonexistent_dest_succeeds_with_real_jail() {
        let tmp = TempDir::new().unwrap();
        let gz = tmp.path().join("payload.gz");
        create_gz(&gz, b"substrate-decompress-dest-jail");
        let dest = tmp.path().join("payload.txt"); // does NOT exist yet

        let deps = ArchiveDeps {
            jail: Arc::new(CanonicalizeJail),
            hasher: make_deps().hasher,
            capabilities: Arc::new(substrate_domain::Capabilities::default()),
        };
        let req = GzipDecompressRequest {
            source: gz.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: false,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        };
        let resp = handle_archive_gzip_decompress(req, &deps, CancellationToken::new())
            .await
            .expect("decompress to a non-existent dest must succeed via parent-jail");
        assert!(dest.exists(), "decompressed output must be written");
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"substrate-decompress-dest-jail"
        );
        let _ = resp;
    }
}
