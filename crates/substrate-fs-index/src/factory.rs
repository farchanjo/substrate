//! `FsIndexFactory` — capability-aware factory for `FsIndexPort` per ADR-0042.
//!
//! The factory inspects the `Capabilities` snapshot produced by `probe_capabilities()`
//! and selects the highest available tier implementation:
//!
//! Linux cascade (preferred → fallback):
//! - `linux-statx` tier: `LinuxStatxIndex` when `has_statx` is true and the
//!   `fs-index` feature is compiled in (requires kernel 4.11+).
//! - `portable` tier: `PortablePollingIndex` fallback.
//!
//! macOS cascade (preferred → fallback):
//! - `macos-bulk` tier: `MacOsBulkIndex` when `has_getattrlistbulk` is true and
//!   `macos-getattrlistbulk` Cargo feature is compiled in (macOS 10.10+).
//! - `portable` tier: `PortablePollingIndex` fallback.
//!
//! When `fs-index` is not compiled in, `build` returns `NullFsIndex` regardless
//! of capabilities. The `chosen_tier()` string is `"null"` in that case.
//!
//! The factory stores the chosen tier name in `OnceLock<&'static str>` so that
//! `chosen_tier()` returns a stable value after the first `build` call. This
//! matches the contract in ADR-0042: `build` is called exactly once by the
//! composition root.

use std::sync::Arc;
use std::sync::OnceLock;

use substrate_domain::capabilities::Capabilities;
use substrate_domain::ports::factory::PortFactory;
use substrate_domain::ports::fs_index::FsIndexPort;

#[cfg(not(feature = "fs-index"))]
use crate::null::NullFsIndex;
#[cfg(feature = "fs-index")]
use crate::polling::PortablePollingIndex;

/// Capability-aware factory for the optional filesystem index port.
///
/// Instantiated once by `substrate-mcp-server` (composition root) and stored
/// until the result of `build()` is wired into the BC adapter crates.
#[derive(Debug, Default)]
pub struct FsIndexFactory {
    chosen_tier: OnceLock<&'static str>,
}

impl FsIndexFactory {
    /// Constructs a new `FsIndexFactory`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            chosen_tier: OnceLock::new(),
        }
    }
}

impl PortFactory<dyn FsIndexPort> for FsIndexFactory {
    fn build(&self, caps: &Capabilities) -> Arc<dyn FsIndexPort> {
        // When fs-index is not compiled in, always return the Null Object.
        #[cfg(not(feature = "fs-index"))]
        {
            let _ = caps; // caps not used by the null path
            self.chosen_tier.set("null").ok();
            tracing::debug!(
                tier = "null",
                "FsIndex: feature fs-index not compiled in; using NullFsIndex"
            );
            NullFsIndex::new()
        }

        #[cfg(feature = "fs-index")]
        {
            // caps may be unused on platforms where no capability-gated tier is
            // compiled in (e.g. macOS without macos-getattrlistbulk feature).
            let _ = caps;
            // Linux tier cascade per ADR-0042.
            #[cfg(target_os = "linux")]
            {
                if caps.has_statx {
                    self.chosen_tier.set("linux-statx").ok();
                    tracing::info!(tier = "linux-statx", "FsIndex: selected linux-statx tier");
                    return crate::linux::LinuxStatxIndex::new();
                }
            }

            // macOS tier cascade per ADR-0042.
            #[cfg(all(target_os = "macos", feature = "macos-getattrlistbulk"))]
            {
                if caps.has_getattrlistbulk {
                    self.chosen_tier.set("macos-bulk").ok();
                    tracing::info!(
                        tier = "macos-bulk",
                        "FsIndex: selected macos-getattrlistbulk tier"
                    );
                    return crate::macos::MacOsBulkIndex::new();
                }
            }

            // Portable fallback tier (cross-platform).
            self.chosen_tier.set("portable").ok();
            tracing::info!(tier = "portable", "FsIndex: selected portable polling tier");
            PortablePollingIndex::new()
        }
    }

    fn chosen_tier(&self) -> &'static str {
        self.chosen_tier.get().copied().unwrap_or("unset")
    }
}
