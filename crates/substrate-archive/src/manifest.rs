//! `ArchiveManifest` — dry-run aggregate root for archive create and extract.
//!
//! When `dry_run = true`, handlers return an `ArchiveManifest` instead of
//! modifying disk. The manifest lists every entry that would be created or
//! overwritten, together with totals.
//!
//! Mirrors `#ArchiveManifest` in `docs/arch/schemas/archive_bc.cue`.

use serde::{Deserialize, Serialize};

/// A single entry that would be created or overwritten.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveEntry {
    /// Path within the archive (relative to archive root).
    pub archive_path: String,

    /// Uncompressed size in bytes.
    pub uncompressed_bytes: u64,

    /// Compression method (e.g. `"deflate"`, `"gzip"`, `"stored"`, `"none"`).
    pub compression_method: String,

    /// Last-modified timestamp in RFC 3339 format, or `null` if unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
}

/// Dry-run preview of what an extract or create operation would produce.
///
/// Aggregate root for the archive bounded context (ADR-0002).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveManifest {
    /// The list of entries that would be created or overwritten.
    pub entries: Vec<ArchiveEntry>,

    /// Total uncompressed byte count across all entries.
    pub total_uncompressed_bytes: u64,

    /// Total number of entries in the manifest.
    pub entry_count: usize,

    /// Whether the operation would overwrite any existing files.
    pub would_overwrite: bool,
}

impl ArchiveManifest {
    /// Constructs a manifest from a slice of entries.
    #[must_use]
    pub fn from_entries(entries: Vec<ArchiveEntry>) -> Self {
        let total_uncompressed_bytes = entries.iter().map(|e| e.uncompressed_bytes).sum();
        let entry_count = entries.len();
        Self {
            entries,
            total_uncompressed_bytes,
            entry_count,
            would_overwrite: false,
        }
    }
}

impl From<ArchiveManifest> for serde_json::Value {
    fn from(m: ArchiveManifest) -> Self {
        serde_json::to_value(m).unwrap_or(Self::Null)
    }
}
