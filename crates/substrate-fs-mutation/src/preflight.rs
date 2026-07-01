//! Preflight checks executed before any disk-write operation.
//!
//! Currently implements the `statvfs`-based disk-space guard per ADR-0033.
//! Returns [`SubstrateError::StorageFull`] when available bytes on the target
//! filesystem are insufficient for the requested write.

use std::path::Path;

use substrate_domain::{SubstrateError, SubstrateResult};

/// Minimum free-space cushion kept above the requested write size (4 KiB).
///
/// Prevents racing to exactly zero free space, which can break filesystem
/// journal operations even if the data write itself would fit.
const FREE_SPACE_CUSHION_BYTES: u64 = 4 * 1024;

/// Checks that `path`'s filesystem has at least `required_bytes +
/// FREE_SPACE_CUSHION_BYTES` available before any write is attempted.
///
/// `path` should be the parent directory of the target file; the file need not
/// exist yet. Errors from `statvfs(3)` are mapped to
/// [`SubstrateError::InternalError`] — they are non-fatal for the security model
/// but indicate an unexpected OS condition that should be logged.
///
/// # Errors
///
/// - [`SubstrateError::StorageFull`] — available space is less than `required_bytes`.
/// - [`SubstrateError::InternalError`] — `statvfs` call failed unexpectedly.
#[allow(
    clippy::cast_possible_truncation,
    reason = "statvfs arithmetic stays within u64"
)]
pub async fn check_disk_space(path: &Path, required_bytes: u64) -> SubstrateResult<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || check_disk_space_sync(&path, required_bytes))
        .await
        .map_err(|e| SubstrateError::InternalError {
            reason: format!("spawn_blocking join error in preflight: {e}"),
            correlation_id: None,
        })?
}

fn check_disk_space_sync(path: &Path, required_bytes: u64) -> SubstrateResult<()> {
    use nix::sys::statvfs::statvfs;

    let stat = statvfs(path).map_err(|e| SubstrateError::InternalError {
        reason: format!("statvfs failed for {}: {e}", path.display()),
        correlation_id: None,
    })?;

    let available: u64 = stat.blocks_available() * stat.block_size();
    let needed = required_bytes.saturating_add(FREE_SPACE_CUSHION_BYTES);

    if available < needed {
        return Err(SubstrateError::StorageFull {
            path: path.display().to_string(),
            correlation_id: None,
        });
    }
    Ok(())
}

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
    use tempfile::TempDir;

    /// Disk-space check succeeds on a real (non-empty) temp dir with 0 bytes required.
    #[tokio::test]
    async fn passes_for_zero_bytes() {
        let dir = TempDir::new().expect("tempdir");
        check_disk_space(dir.path(), 0)
            .await
            .expect("should pass for 0 bytes");
    }

    /// A trivially small write (1 byte) also passes on a real filesystem.
    #[tokio::test]
    async fn passes_for_one_byte() {
        let dir = TempDir::new().expect("tempdir");
        check_disk_space(dir.path(), 1)
            .await
            .expect("should pass for 1 byte");
    }
}
