//! Decompression resource-limit guard against gzip-bomb / unbounded-input attacks.
//!
//! All decompression handlers pass bytes through [`DecompressGuard`] to enforce
//! a configurable maximum output-byte ceiling. When exceeded, the handler returns
//! `SUBSTRATE_RESOURCE_LIMIT` and the caller discards the in-progress output.
//!
//! This guard satisfies the Gherkin scenario in
//! `docs/arch/specs/features/archive/archive-gzip-large-input-resource-limit.feature`.
//!
//! Default ceiling: 100 MiB (`DEFAULT_MAX_OUTPUT_BYTES`).

use substrate_domain::{SubstrateError, SubstrateResult};

/// Default maximum output size for decompression: 100 MiB.
pub const DEFAULT_MAX_OUTPUT_BYTES: u64 = 100 * 1024 * 1024;

/// Streaming resource-limit guard for decompression output.
///
/// The caller calls [`record`](DecompressGuard::record) after each chunk is
/// written. Once the accumulated written bytes exceed `max_bytes`, the guard
/// returns an error and all further writes MUST stop.
#[derive(Debug)]
pub struct DecompressGuard {
    written: u64,
    max: u64,
}

impl DecompressGuard {
    /// Creates a new guard with a given maximum output size.
    #[must_use]
    pub const fn new(max_bytes: u64) -> Self {
        Self {
            written: 0,
            max: max_bytes,
        }
    }

    /// Creates a new guard with the default 100 MiB ceiling.
    #[must_use]
    pub const fn default_limit() -> Self {
        Self::new(DEFAULT_MAX_OUTPUT_BYTES)
    }

    /// Records `n` additional bytes written and returns an error if the ceiling
    /// is exceeded.
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_RESOURCE_LIMIT` — cumulative written bytes exceed `max_bytes`.
    pub fn record(&mut self, n: u64) -> SubstrateResult<()> {
        self.written = self.written.saturating_add(n);
        if self.written > self.max {
            return Err(SubstrateError::ResourceLimit {
                detail: format!(
                    "decompressed output ({} bytes) exceeds limit ({} bytes)",
                    self.written, self.max
                ),
                correlation_id: Some(uuid::Uuid::now_v7()),
            });
        }
        Ok(())
    }

    /// Returns the number of bytes recorded so far.
    #[must_use]
    pub const fn written(&self) -> u64 {
        self.written
    }
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

    #[test]
    fn within_limit_is_ok() {
        let mut guard = DecompressGuard::new(1024);
        assert!(guard.record(512).is_ok());
        assert!(guard.record(511).is_ok());
    }

    #[test]
    fn exceeding_limit_returns_resource_limit_error() {
        let mut guard = DecompressGuard::new(100);
        assert!(guard.record(100).is_ok());
        let err = guard.record(1).unwrap_err();
        assert!(matches!(err, SubstrateError::ResourceLimit { .. }));
        assert_eq!(err.code(), "SUBSTRATE_RESOURCE_LIMIT");
    }

    #[test]
    fn exact_limit_is_ok() {
        let mut guard = DecompressGuard::new(100);
        assert!(guard.record(100).is_ok());
    }

    #[test]
    fn large_single_chunk_exceeding_limit_is_caught() {
        let mut guard = DecompressGuard::new(512);
        let err = guard.record(1024).unwrap_err();
        assert!(matches!(err, SubstrateError::ResourceLimit { .. }));
    }
}
