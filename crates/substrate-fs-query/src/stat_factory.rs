//! `StatFactory` — `PortFactory<dyn StatPort>` per ADR-0042.
//!
//! Selects the file-metadata implementation tier. The portable tier uses
//! `std::fs::metadata` (no `nix` dependency). Native Linux / macOS tiers
//! are stubs delegating to portable until Wave G+ implements `statx(2)` and
//! `getattrlist(2)`.

use std::sync::{Arc, OnceLock};

use substrate_domain::ports::stat::FileStat;
use substrate_domain::value_objects::jailed_path::JailedPath;
use substrate_domain::{
    Capabilities, PortFactory, StatPort, StatTier, SubstrateError, SubstrateResult,
};

// ---- Portable statter -------------------------------------------------------

/// Cross-platform stat implementation using `std::fs::metadata`.
///
/// Does not follow symlinks (`symlink_metadata` semantics) to match the
/// `lstat` contract declared in `StatPort`.
#[derive(Debug, Default)]
pub struct PortableStatter;

impl PortableStatter {
    /// Creates a new `PortableStatter`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl StatPort for PortableStatter {
    fn stat(&self, path: &JailedPath) -> SubstrateResult<FileStat> {
        use std::io::ErrorKind;
        use std::time::UNIX_EPOCH;

        let meta = std::fs::symlink_metadata(path.as_path()).map_err(|e| {
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

        let modified_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());

        let accessed_secs = meta
            .accessed()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());

        #[expect(
            clippy::cast_possible_wrap,
            reason = "unix timestamps in the valid range fit in i64"
        )]
        let modified_at = time::OffsetDateTime::from_unix_timestamp(modified_secs as i64)
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
        #[expect(
            clippy::cast_possible_wrap,
            reason = "unix timestamps in the valid range fit in i64"
        )]
        let accessed_at = time::OffsetDateTime::from_unix_timestamp(accessed_secs as i64)
            .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);

        Ok(FileStat {
            size_bytes: meta.len(),
            is_dir: meta.is_dir(),
            is_file: meta.is_file(),
            is_symlink: meta.is_symlink(),
            modified_at,
            accessed_at,
        })
    }
}

// ---- Factory ----------------------------------------------------------------

/// Factory that selects the `StatPort` implementation tier.
#[derive(Debug, Default)]
pub struct StatFactory {
    chosen: OnceLock<&'static str>,
}

impl StatFactory {
    /// Creates a new `StatFactory`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            chosen: OnceLock::new(),
        }
    }
}

impl PortFactory<dyn StatPort> for StatFactory {
    fn build(&self, caps: &Capabilities) -> Arc<dyn StatPort> {
        // TODO: Wave G+ will add LinuxStatx and MacosGetattrlist implementations.
        let tier_name: &'static str = match caps.stat_tier {
            StatTier::LinuxStatx | StatTier::LinuxFstatat => "linux-fstatat",
            StatTier::MacosGetattrlist | StatTier::MacosFstatat => "macos-fstatat",
            StatTier::PortableMetadata => "portable-metadata",
        };
        let _ = self.chosen.set(tier_name);
        Arc::new(PortableStatter::new())
    }

    fn chosen_tier(&self) -> &'static str {
        self.chosen.get().copied().unwrap_or("portable-metadata")
    }
}
