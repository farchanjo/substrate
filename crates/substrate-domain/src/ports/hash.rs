//! `HashPort` — inbound port for BLAKE3 content hashing per ADR-0032 and ADR-0042.
//!
//! Tier is selected by `HashFactory` at startup based on `caps.simd_tier`.
//! Note: the `blake3` mmap feature is DISABLED per signal-safety contract (ADR-0032)
//! to avoid `SIGBUS` on concurrent file truncation.
//!
//! This port is CPU-bound. Adapters implement the trait synchronously; callers
//! wrap invocations in `tokio::task::spawn_blocking` (Zone C per ADR-0003).

use crate::errors::SubstrateResult;
use crate::value_objects::JailedPath;

/// A 32-byte BLAKE3 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Blake3Digest([u8; 32]);

impl Blake3Digest {
    /// Wraps a raw 32-byte digest array.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the raw 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns the digest as a 64-character lowercase hex string.
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.iter().fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
    }
}

impl std::fmt::Display for Blake3Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Inbound port for BLAKE3 content hashing per ADR-0042.
///
/// CPU-bound; the composition root wraps calls in `spawn_blocking` per ADR-0003 Zone C.
pub trait HashPort: Send + Sync {
    /// Hashes the contents of `path` using BLAKE3 and returns the 32-byte digest.
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_IO_ERROR` — hardware I/O failure reading `path`.
    /// - `SUBSTRATE_NOT_FOUND` — `path` does not exist on disk.
    /// - `SUBSTRATE_PERMISSION_DENIED` — the process cannot read `path`.
    fn hash_file(&self, path: &JailedPath) -> SubstrateResult<Blake3Digest>;

    /// Hashes the provided byte slice in memory using BLAKE3.
    #[must_use]
    fn hash_bytes(&self, data: &[u8]) -> Blake3Digest;
}
