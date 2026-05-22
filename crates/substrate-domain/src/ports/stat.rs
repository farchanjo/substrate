//! `StatPort` — inbound port for file-metadata retrieval per ADR-0042.
//!
//! Tier is selected by `StatFactory` at startup: `LinuxStatx` on kernel ≥ 4.11,
//! `MacosGetattrlist` on macOS 10.10+, or `PortableMetadata` elsewhere.
//!
//! CPU-bound; adapters implement this synchronously. Callers in async context
//! wrap invocations in `tokio::task::spawn_blocking` (Zone B per ADR-0003).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::errors::SubstrateResult;
use crate::value_objects::JailedPath;

/// File metadata snapshot returned by [`StatPort::stat`].
///
/// The full surface (inode, block count, extended attributes, birth time on
/// platforms that support it) will be expanded when the filesystem-query
/// adapter is implemented.
// TODO: expand FileStat fields in the fs-query adapter wave.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStat {
    /// Size in bytes for regular files; `0` for directories and special files.
    pub size_bytes: u64,

    /// `true` when this entry is a directory.
    pub is_dir: bool,

    /// `true` when this entry is a regular file.
    pub is_file: bool,

    /// `true` when this entry is a symbolic link.
    pub is_symlink: bool,

    /// Last modification time.
    pub modified_at: OffsetDateTime,

    /// Last access time.
    pub accessed_at: OffsetDateTime,
}

/// Inbound port for file-metadata retrieval per ADR-0042.
///
/// CPU-bound; the composition root wraps calls in `spawn_blocking` per ADR-0003 Zone B.
pub trait StatPort: Send + Sync {
    /// Returns metadata for `path` without following symlinks (`lstat` semantics).
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_NOT_FOUND` — `path` does not exist on disk.
    /// - `SUBSTRATE_PERMISSION_DENIED` — the process cannot stat `path`.
    /// - `SUBSTRATE_IO_ERROR` — hardware I/O failure.
    fn stat(&self, path: &JailedPath) -> SubstrateResult<FileStat>;
}
