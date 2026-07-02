//! Handler for the `fs.hash` tool — Zone C (`spawn_blocking` + `Semaphore(num_cpus)`).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: compute a content digest for integrity verification before or after mutations
//! DOES: BLAKE3 (default) or SHA-256 hashing of a file; returns hex-encoded digest
//! ARGS: path (string) — file to hash;
//!       algorithm (string, "blake3") — "blake3" | "sha256"
//! RETURNS: {path, algorithm, digest, size_bytes}
//! NEXT: fs.stat, fs.read
//! AVOID: hashing entire directory trees inline → hash individual files
//! ```
//!
//! # Zone classification
//!
//! `HashPort::hash_file` is CPU-bound (BLAKE3 / SHA-256). The handler:
//!
//! 1. Acquires an owned permit from a process-global `Semaphore(num_cpus)`.
//! 2. Dispatches hashing inside `spawn_blocking`.
//! 3. Releases the permit when `spawn_blocking` returns.
//!
//! The `blake3` mmap feature is DISABLED per ADR-0032 (SIGBUS risk).

use std::sync::{Arc, OnceLock};

use serde::Deserialize;
use serde_json::json;
use sha2::Digest as _;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{HashPort, JailedPath, PathJailPort, SubstrateError, SubstrateResult};

use crate::hint_helpers::build_hints;
use crate::response::{FsQueryDeps, ToolResponse};

/// Process-global CPU semaphore for Zone C hashing (ADR-0003 / ADR-0017).
///
/// Sized to `num_cpus::get()` at first use; permits are acquired as owned
/// so they survive across `.await` points without holding a `MutexGuard`.
static HASH_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

/// Read-buffer size for streaming SHA-256 file hashing (64 KiB), mirroring
/// `hash_factory::Blake3Hasher`'s streaming read loop so neither hasher ever
/// buffers an entire file into memory.
const SHA256_READ_BUF_SIZE: usize = 65_536;

fn hash_semaphore() -> &'static Arc<Semaphore> {
    HASH_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(num_cpus::get())))
}

/// Hashing algorithm selection.
#[derive(Debug, Clone, Deserialize, Default, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgorithm {
    /// BLAKE3 parallel hasher (default).
    #[default]
    Blake3,
    /// SHA-256 (Zone C, CPU-bound).
    Sha256,
}

impl std::fmt::Display for HashAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blake3 => f.write_str("blake3"),
            Self::Sha256 => f.write_str("sha256"),
        }
    }
}

/// Inbound request for `fs.hash`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FsHashRequest {
    /// Path to the file to hash; must be within an allowlist root.
    pub path: String,

    /// Hashing algorithm.
    #[serde(default)]
    pub algorithm: HashAlgorithm,
}

/// Handler for `fs.hash`.
///
/// Zone C: CPU-bound hash dispatched via `spawn_blocking` behind a
/// process-global `Semaphore(num_cpus)`.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, semaphore closure,
/// or file I/O during hashing.
#[expect(
    clippy::too_many_lines,
    reason = "handle_fs_hash orchestrates jail, semaphore acquisition, and the BLAKE3/SHA-256 \
              dispatch (including the streaming SHA-256 read loop) in one cohesive Zone-C handler"
)]
#[instrument(skip(deps, _cancel), fields(path = %req.path, algorithm = %req.algorithm))]
pub async fn handle_fs_hash(
    req: FsHashRequest,
    deps: &FsQueryDeps,
    _cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    // Jail the path against the real allowlist root (never the path itself —
    // see `FsQueryDeps::allowlist_root` doc comment for why that would make
    // the kernel dirfd confinement check a no-op).
    let raw = std::path::Path::new(&req.path).to_path_buf();
    let jail: Arc<dyn PathJailPort> = Arc::clone(&deps.jail);
    let raw_clone = raw.clone();
    let allowlist_root = deps.allowlist_root.clone();
    let jailed: JailedPath = tokio::task::spawn_blocking(move || {
        jail.jail(&allowlist_root, &raw_clone)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: None,
    })??;

    // Acquire CPU permit (owned so it survives the .await below).
    let _permit = hash_semaphore()
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| SubstrateError::InternalError {
            reason: "hash semaphore closed".to_owned(),
            correlation_id: None,
        })?;

    let algorithm = req.algorithm.clone();
    let hasher: Arc<dyn HashPort> = Arc::clone(&deps.hasher);

    // Zone C: dispatch CPU-bound work.
    let (digest_hex, size_bytes) = tokio::task::spawn_blocking(move || -> SubstrateResult<_> {
        match algorithm {
            HashAlgorithm::Blake3 => {
                let digest = hasher.hash_file(&jailed)?;
                // Get file size for metadata.
                let size = std::fs::metadata(jailed.as_path()).map_or(0, |m| m.len());
                Ok((digest.to_hex(), size))
            },
            HashAlgorithm::Sha256 => {
                // SHA-256 path: stream the file through a 64 KiB buffer instead
                // of `std::fs::read`-ing it whole, mirroring the BLAKE3 streaming
                // path in `hash_factory::Blake3Hasher::hash_file` so hashing a
                // multi-GiB file never buffers the entire content in memory.
                use std::io::Read as _;
                let map_open_err = |e: std::io::Error| {
                    use std::io::ErrorKind;
                    match e.kind() {
                        ErrorKind::NotFound => SubstrateError::NotFound {
                            resource: jailed.to_string(),
                            correlation_id: None,
                        },
                        ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
                            path: jailed.to_string(),
                            correlation_id: None,
                        },
                        _ => SubstrateError::IoError {
                            path: jailed.to_string(),
                            correlation_id: None,
                        },
                    }
                };
                let mut file = std::fs::File::open(jailed.as_path()).map_err(map_open_err)?;

                let mut hasher_sha = sha2::Sha256::new();
                let mut buf = vec![0u8; SHA256_READ_BUF_SIZE];
                let mut size: u64 = 0;
                loop {
                    let n = file.read(&mut buf).map_err(|_| SubstrateError::IoError {
                        path: jailed.to_string(),
                        correlation_id: None,
                    })?;
                    if n == 0 {
                        break;
                    }
                    hasher_sha.update(&buf[..n]);
                    size += n as u64;
                }

                let result = hasher_sha.finalize();
                let hex = result.iter().fold(String::with_capacity(64), |mut s, b| {
                    use std::fmt::Write as _;
                    let _ = write!(s, "{b:02x}");
                    s
                });
                Ok((hex, size))
            },
        }
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join error: {e}"),
        correlation_id: None,
    })??;

    let algorithm_name = req.algorithm.to_string();

    let hints = build_hints(
        Some("fs.stat"),
        Some("fs.read"),
        Some("Use fs.hash per-file; avoid hashing directory trees inline"),
        &deps.capabilities,
        false,
    );

    let content = format!(
        "USE: verify file integrity\nDOES: {algorithm_name} digest of '{}'\nNEXT: fs.stat, fs.read\nAVOID: bulk tree hashing → hash individual files",
        req.path
    );

    let structured_content = json!({
        "tool": "fs.hash",
        "path": req.path,
        "algorithm": algorithm_name,
        "digest": digest_hex,
        "size_bytes": size_bytes,
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
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
    /// `allowlist_root` argument `handle_fs_hash` passes to `jail()` is the
    /// real configured root (`deps.allowlist_root`), never a `JailedPath`
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
    async fn blake3_hash_is_deterministic() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.bin");
        std::fs::write(&path, b"substrate-hash-test").unwrap();
        let deps = make_deps();
        let path_str = path.to_string_lossy().into_owned();

        let r1 = handle_fs_hash(
            FsHashRequest {
                path: path_str.clone(),
                algorithm: HashAlgorithm::Blake3,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let r2 = handle_fs_hash(
            FsHashRequest {
                path: path_str,
                algorithm: HashAlgorithm::Blake3,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            r1.structured_content["digest"],
            r2.structured_content["digest"]
        );
    }

    #[tokio::test]
    async fn sha256_returns_64_char_hex() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.bin");
        std::fs::write(&path, b"abc").unwrap();
        let deps = make_deps();
        let resp = handle_fs_hash(
            FsHashRequest {
                path: path.to_string_lossy().into_owned(),
                algorithm: HashAlgorithm::Sha256,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let digest = resp.structured_content["digest"].as_str().unwrap();
        assert_eq!(digest.len(), 64);
    }

    #[tokio::test]
    async fn hash_missing_file_returns_not_found() {
        let deps = make_deps();
        let err = handle_fs_hash(
            FsHashRequest {
                path: "/tmp/__substrate_no_such_file_hash_xyz".to_owned(),
                algorithm: HashAlgorithm::Blake3,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SubstrateError::NotFound { .. }));
    }

    #[tokio::test]
    async fn blake3_and_sha256_differ_for_same_input() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.bin");
        std::fs::write(&path, b"substrate").unwrap();
        let deps = make_deps();
        let path_str = path.to_string_lossy().into_owned();

        let b3 = handle_fs_hash(
            FsHashRequest {
                path: path_str.clone(),
                algorithm: HashAlgorithm::Blake3,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let s2 = handle_fs_hash(
            FsHashRequest {
                path: path_str,
                algorithm: HashAlgorithm::Sha256,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_ne!(
            b3.structured_content["digest"],
            s2.structured_content["digest"]
        );
    }

    /// Regression guard for the streaming SHA-256 rewrite: a file larger than
    /// `SHA256_READ_BUF_SIZE` (64 KiB) must still hash correctly, proving the
    /// read loop drains multiple chunks rather than truncating at the first
    /// buffer fill. Cross-checked against an in-memory `sha2::Sha256` digest
    /// of the same bytes (independent of the streaming code path under test).
    #[tokio::test]
    async fn sha256_streams_file_larger_than_read_buffer() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("large.bin");
        // 3 whole buffers plus a partial one, so the loop must observe a
        // final short read before the terminating zero-length read.
        let size = SHA256_READ_BUF_SIZE * 3 + 1_234;
        let data: Vec<u8> = (0u8..=255).cycle().take(size).collect();
        std::fs::write(&path, &data).unwrap();
        let deps = make_deps();

        let resp = handle_fs_hash(
            FsHashRequest {
                path: path.to_string_lossy().into_owned(),
                algorithm: HashAlgorithm::Sha256,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        let mut expected_hasher = sha2::Sha256::new();
        expected_hasher.update(&data);
        let expected_hex = expected_hasher
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut s, b| {
                use std::fmt::Write as _;
                let _ = write!(s, "{b:02x}");
                s
            });

        assert_eq!(
            resp.structured_content["digest"].as_str().unwrap(),
            expected_hex
        );
        assert_eq!(resp.structured_content["size_bytes"], size as u64);
    }

    /// Regression test for the self-referential jail bug (FIX 1): `handle_fs_hash`
    /// must jail the requested path against `deps.allowlist_root`, not against a
    /// `JailedPath` fabricated from the path itself.
    #[tokio::test]
    async fn hash_jails_against_configured_allowlist_root() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.bin");
        std::fs::write(&path, b"substrate").unwrap();

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
        handle_fs_hash(
            FsHashRequest {
                path: path.to_string_lossy().into_owned(),
                algorithm: HashAlgorithm::Blake3,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    }
}
