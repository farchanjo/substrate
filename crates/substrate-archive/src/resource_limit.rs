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

use std::path::Path;

use substrate_domain::{SubstrateError, SubstrateResult};

/// Default maximum output size for decompression: 100 MiB.
pub const DEFAULT_MAX_OUTPUT_BYTES: u64 = 100 * 1024 * 1024;

/// Default per-archive aggregate ceiling for extraction output: 1 GiB.
///
/// Guards against an archive whose total uncompressed payload (sum across all
/// members) would exhaust memory or disk even if no single member exceeds the
/// per-entry ceiling. Used as the `max_bytes` for the aggregate
/// [`DecompressGuard`] threaded through tar/zip extraction.
pub const DEFAULT_MAX_EXTRACT_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// Maximum input size for whole-file gzip compression: 1 GiB.
///
/// Sources larger than this are rejected with `SUBSTRATE_RESOURCE_LIMIT` rather
/// than being read fully into the heap. Streaming chunked compression keeps peak
/// memory bounded below this limit for accepted inputs.
pub const MAX_COMPRESS_INPUT_BYTES: u64 = 1024 * 1024 * 1024;

/// Free-space cushion kept above the requested write size (4 KiB).
///
/// Prevents racing to exactly zero free space, which can break filesystem
/// journal operations even when the data write itself would fit.
const FREE_SPACE_CUSHION_BYTES: u64 = 4 * 1024;

/// Checks that `path`'s filesystem has at least `required_bytes +
/// FREE_SPACE_CUSHION_BYTES` available before any extraction or compression
/// write is attempted (ADR-0033 disk-space preflight).
///
/// `path` should be an existing directory (e.g. the extraction root or the
/// destination's parent); the target file need not exist yet. The blocking
/// `statvfs(3)` syscall runs inside `spawn_blocking` per ADR-0003.
///
/// # Errors
///
/// - [`SubstrateError::StorageFull`] — available space is below the requirement.
/// - [`SubstrateError::InternalError`] — the `statvfs` call failed unexpectedly.
pub async fn check_disk_space(path: &Path, required_bytes: u64) -> SubstrateResult<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || check_disk_space_sync(&path, required_bytes))
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error in disk-space preflight: {e}"),
            correlation_id: Some(uuid::Uuid::now_v7()),
        })?
}

fn check_disk_space_sync(path: &Path, required_bytes: u64) -> SubstrateResult<()> {
    use nix::sys::statvfs::statvfs;

    let stat = statvfs(path).map_err(|e| SubstrateError::InternalError {
        reason: format!("statvfs failed for {}: {e}", path.display()),
        correlation_id: Some(uuid::Uuid::now_v7()),
    })?;

    let available: u64 = u64::from(stat.blocks_available()) * stat.block_size();
    let needed = required_bytes.saturating_add(FREE_SPACE_CUSHION_BYTES);

    if available < needed {
        return Err(SubstrateError::StorageFull {
            path: path.display().to_string(),
            correlation_id: Some(uuid::Uuid::now_v7()),
        });
    }
    Ok(())
}

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

    #[tokio::test]
    async fn disk_space_passes_for_zero_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        check_disk_space(dir.path(), 0)
            .await
            .expect("zero-byte preflight should pass on a real filesystem");
    }

    #[tokio::test]
    async fn disk_space_fails_for_implausible_request() {
        let dir = tempfile::TempDir::new().unwrap();
        // Request more bytes than any real filesystem can hold.
        let err = check_disk_space(dir.path(), u64::MAX - 1)
            .await
            .unwrap_err();
        assert!(matches!(err, SubstrateError::StorageFull { .. }));
        assert_eq!(err.code(), "SUBSTRATE_STORAGE_FULL");
    }
}
