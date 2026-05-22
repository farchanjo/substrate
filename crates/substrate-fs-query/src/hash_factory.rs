//! `HashFactory` — `PortFactory<dyn HashPort>` per ADR-0042.
//!
//! Selects the BLAKE3 hashing implementation tier driven by `caps.simd_tier`.
//! The `blake3` mmap feature is DISABLED per ADR-0032 (SIGBUS risk on concurrent
//! file truncation). BLAKE3 dispatches SIMD internally via its own runtime
//! detection; the factory controls which BLAKE3 feature set is compiled in.

use std::sync::{Arc, OnceLock};

use blake3::Hasher as Blake3Hasher_;
use substrate_domain::ports::hash::Blake3Digest;
use substrate_domain::value_objects::jailed_path::JailedPath;
use substrate_domain::{
    Capabilities, HashPort, HashTier, PortFactory, SubstrateError, SubstrateResult,
};

/// BLAKE3 hasher implementation that reads the file in chunks.
///
/// The `mmap` feature is DISABLED per ADR-0032; all file reads go through
/// `std::fs::File` + `std::io::Read` in a 64 KiB buffer.
#[derive(Debug, Default)]
pub struct Blake3Hasher;

/// Read-buffer size for BLAKE3 file hashing (64 KiB).
const READ_BUF_SIZE: usize = 65_536;

impl Blake3Hasher {
    /// Creates a new `Blake3Hasher`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl HashPort for Blake3Hasher {
    fn hash_file(&self, path: &JailedPath) -> SubstrateResult<Blake3Digest> {
        use std::io::Read as _;
        let mut file = std::fs::File::open(path.as_path()).map_err(|e| {
            use std::io::ErrorKind;
            match e.kind() {
                ErrorKind::NotFound => SubstrateError::NotFound {
                    resource: path.to_string(),
                    correlation_id: None,
                },
                ErrorKind::PermissionDenied => SubstrateError::PermissionDenied {
                    path: path.to_string(),
                    correlation_id: None,
                },
                _ => SubstrateError::IoError {
                    path: path.to_string(),
                    correlation_id: None,
                },
            }
        })?;

        let mut hasher = Blake3Hasher_::new();
        let mut buf = vec![0u8; READ_BUF_SIZE];

        loop {
            let n = file.read(&mut buf).map_err(|_| SubstrateError::IoError {
                path: path.to_string(),
                correlation_id: None,
            })?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }

        let digest = hasher.finalize();
        Ok(Blake3Digest::new(*digest.as_bytes()))
    }

    fn hash_bytes(&self, data: &[u8]) -> Blake3Digest {
        let digest = blake3::hash(data);
        Blake3Digest::new(*digest.as_bytes())
    }
}

/// Factory that selects the BLAKE3 hasher tier from the capability snapshot.
#[derive(Debug, Default)]
pub struct HashFactory {
    chosen: OnceLock<&'static str>,
}

impl HashFactory {
    /// Creates a new `HashFactory`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            chosen: OnceLock::new(),
        }
    }
}

impl PortFactory<dyn HashPort> for HashFactory {
    fn build(&self, caps: &Capabilities) -> Arc<dyn HashPort> {
        // All tier paths currently use the same `Blake3Hasher` implementation.
        // blake3 internally dispatches SIMD via its own runtime detection;
        // the tier name here is diagnostic only (ADR-0042 / ADR-0043).
        let tier_name: &'static str = match caps.hash_tier {
            HashTier::Blake3Avx512 => "blake3-avx512",
            HashTier::Blake3Avx2 => "blake3-avx2",
            HashTier::Blake3Neon => "blake3-neon",
            HashTier::Blake3Sse2 => "blake3-sse2",
            HashTier::Blake3Portable => "blake3-portable",
        };
        let _ = self.chosen.set(tier_name);
        Arc::new(Blake3Hasher::new())
    }

    fn chosen_tier(&self) -> &'static str {
        self.chosen.get().copied().unwrap_or("blake3-portable")
    }
}
