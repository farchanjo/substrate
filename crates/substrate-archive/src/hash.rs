//! Handler for `archive.hash` — Bucket B, Zone C (`spawn_blocking` + `Semaphore`).
//!
//! # Narrative arc (ADR-0007)
//!
//! ```text
//! USE: verify archive integrity before and after create/extract operations
//! DOES: BLAKE3 (default) or SHA-256 content digest of an archive file;
//!       read-only — no dry-run or confirmation required
//! ARGS: path (string) — archive file path within the allowlist;
//!       algorithm ("blake3"|"sha256") — hashing algorithm
//! RETURNS: {path, algorithm, digest, size_bytes}
//! NEXT: archive.tar.extract, archive.zip.extract, fs.stat
//! AVOID: hashing before calling archive.hash after every create/extract
//! ```
//!
//! # Zone classification
//!
//! Same pattern as `fs.hash`: CPU-bound work dispatched via `spawn_blocking`
//! behind a process-global `Semaphore(num_cpus)`. BLAKE3 mmap feature is
//! DISABLED per ADR-0032 (SIGBUS risk on concurrent truncation).

use std::sync::{Arc, OnceLock};

use serde::Deserialize;
use serde_json::json;
use sha2::Digest as _;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use substrate_domain::{HashPort, JailedPath, PathJailPort, SubstrateError, SubstrateResult};

use crate::hints_helpers::build_inline_hints;
use crate::response::{ArchiveDeps, ToolResponse};

/// Process-global CPU semaphore (Zone C — ADR-0003 / ADR-0017).
static HASH_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn hash_semaphore() -> &'static Arc<Semaphore> {
    HASH_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(num_cpus::get())))
}

/// Hashing algorithm selection.
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HashAlgorithm {
    /// BLAKE3 (default).
    #[default]
    Blake3,
    /// SHA-256.
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

/// Input parameters for `archive.hash`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveHashRequest {
    /// Archive file path within the allowlist.
    pub path: String,

    /// Hashing algorithm.
    #[serde(default)]
    pub algorithm: HashAlgorithm,
}

/// Handler for `archive.hash`.
///
/// Zone C: CPU-bound hashing dispatched via `spawn_blocking` behind
/// `Semaphore(num_cpus)`. Read-only: no dry-run or confirmation gate.
///
/// # Errors
///
/// Propagates any [`SubstrateError`] from jail validation, semaphore closure, or I/O.
#[instrument(skip(deps, _cancel), fields(path = %req.path, algorithm = %req.algorithm))]
pub async fn handle_archive_hash(
    req: ArchiveHashRequest,
    deps: &ArchiveDeps,
    _cancel: CancellationToken,
) -> SubstrateResult<ToolResponse> {
    // Jail the path.
    let raw = std::path::PathBuf::from(&req.path);
    let jail: Arc<dyn PathJailPort> = Arc::clone(&deps.jail);
    let raw_clone = raw.clone();
    let jailed: JailedPath = tokio::task::spawn_blocking(move || {
        jail.jail(&JailedPath::new_jailed(raw_clone.clone()), &raw_clone)
    })
    .await
    .map_err(|e| SubstrateError::InternalError {
        reason: format!("spawn_blocking join: {e}"),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })??;

    // Acquire CPU semaphore permit (owned — survives .await).
    let _permit = hash_semaphore()
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| SubstrateError::InternalError {
            reason: "hash semaphore closed".to_owned(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?;

    let algorithm = req.algorithm;
    let hasher: Arc<dyn HashPort> = Arc::clone(&deps.hasher);

    let (digest_hex, size_bytes) =
        tokio::task::spawn_blocking(move || -> SubstrateResult<(String, u64)> {
            match algorithm {
                HashAlgorithm::Blake3 => {
                    let digest = hasher.hash_file(&jailed)?;
                    let size = std::fs::metadata(jailed.as_path()).map_or(0, |m| m.len());
                    Ok((digest.to_hex(), size))
                },
                HashAlgorithm::Sha256 => {
                    let bytes = std::fs::read(jailed.as_path()).map_err(|e| {
                        use std::io::ErrorKind;
                        match e.kind() {
                            ErrorKind::NotFound => SubstrateError::NotFound {
                                resource: jailed.to_string(),
                                correlation_id: Some(uuid::Uuid::now_v7()),
                            },
                            ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
                                path: jailed.to_string(),
                                correlation_id: Some(uuid::Uuid::now_v7()),
                            },
                            _ => SubstrateError::IoError {
                                path: jailed.to_string(),
                                correlation_id: Some(uuid::Uuid::now_v7()),
                            },
                        }
                    })?;
                    let size = bytes.len() as u64;
                    let mut h = sha2::Sha256::new();
                    h.update(&bytes);
                    let result = h.finalize();
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
            reason: format!("spawn_blocking join: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })??;

    let algorithm_name = algorithm.to_string();
    let hints = build_inline_hints(
        Some("archive.tar.extract"),
        Some("archive.zip.extract"),
        &deps.capabilities,
        false,
    );
    let content = format!(
        "USE: verify archive integrity\nDOES: {algorithm_name} digest of '{}'\nNEXT: archive.tar.extract, archive.zip.extract\nAVOID: hashing directories — hash individual archives",
        req.path
    );
    let structured_content = json!({
        "tool": "archive.hash",
        "path": req.path,
        "algorithm": algorithm_name,
        "digest": digest_hex,
        "size_bytes": size_bytes,
        "hints": hints,
    });

    Ok(ToolResponse::with_hints(content, structured_content, hints))
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

    // A simple Blake3 hasher using the blake3 crate directly (no mmap).
    struct SimpleBlake3Hasher;
    impl substrate_domain::HashPort for SimpleBlake3Hasher {
        fn hash_file(
            &self,
            path: &JailedPath,
        ) -> SubstrateResult<substrate_domain::ports::hash::Blake3Digest> {
            let data = std::fs::read(path.as_path()).map_err(|_| SubstrateError::NotFound {
                resource: path.to_string(),
                correlation_id: Some(uuid::Uuid::now_v7()),
            })?;
            let digest = blake3::hash(&data);
            Ok(substrate_domain::ports::hash::Blake3Digest::new(
                *digest.as_bytes(),
            ))
        }

        fn hash_bytes(&self, data: &[u8]) -> substrate_domain::ports::hash::Blake3Digest {
            let digest = blake3::hash(data);
            substrate_domain::ports::hash::Blake3Digest::new(*digest.as_bytes())
        }
    }

    fn make_deps() -> ArchiveDeps {
        ArchiveDeps {
            jail: Arc::new(NoopJail),
            hasher: Arc::new(SimpleBlake3Hasher),
            capabilities: Arc::new(substrate_domain::Capabilities::default()),
        }
    }

    #[tokio::test]
    async fn blake3_hash_is_deterministic() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("archive.tar");
        std::fs::write(&path, b"substrate-archive-hash-test").unwrap();
        let deps = make_deps();
        let path_str = path.to_string_lossy().into_owned();

        let r1 = handle_archive_hash(
            ArchiveHashRequest {
                path: path_str.clone(),
                algorithm: HashAlgorithm::Blake3,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let r2 = handle_archive_hash(
            ArchiveHashRequest {
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
        let path = tmp.path().join("archive.zip");
        std::fs::write(&path, b"abc").unwrap();
        let deps = make_deps();
        let resp = handle_archive_hash(
            ArchiveHashRequest {
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
        let err = handle_archive_hash(
            ArchiveHashRequest {
                path: "/tmp/__substrate_no_such_archive_xyz".to_owned(),
                algorithm: HashAlgorithm::Blake3,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SubstrateError::NotFound { .. }));
    }

    // Proptest: identical byte content must always produce the same BLAKE3 digest.
    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(20))]
        #[test]
        fn blake3_is_deterministic_for_arbitrary_content(
            content in proptest::collection::vec(proptest::num::u8::ANY, 0..=512)
        ) {
            let d1 = blake3::hash(&content);
            let d2 = blake3::hash(&content);
            proptest::prop_assert_eq!(
                d1.as_bytes(),
                d2.as_bytes(),
                "BLAKE3 must be deterministic"
            );
        }
    }

    #[tokio::test]
    async fn blake3_and_sha256_differ_for_same_input() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("data.tar");
        std::fs::write(&path, b"substrate").unwrap();
        let deps = make_deps();
        let path_str = path.to_string_lossy().into_owned();

        let b3 = handle_archive_hash(
            ArchiveHashRequest {
                path: path_str.clone(),
                algorithm: HashAlgorithm::Blake3,
            },
            &deps,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let s2 = handle_archive_hash(
            ArchiveHashRequest {
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
}
