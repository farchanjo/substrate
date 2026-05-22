//! `WalkerFactory` — `PortFactory<dyn DirWalkerPort>` per ADR-0042.
//!
//! Selects the highest available walker tier based on the capability snapshot
//! and platform cfg guards. Records the chosen tier in an `OnceLock` for
//! audit annotations.

use std::sync::{Arc, OnceLock};

use substrate_domain::{Capabilities, DirWalkerPort, PortFactory, WalkerTier};

use crate::walker::legacy::LegacyWalker;

/// Factory that selects the appropriate `DirWalkerPort` implementation.
///
/// Tier cascade (highest to lowest):
/// - `linux-iouring` (not yet implemented; reserved).
/// - `linux-statx` (`LinuxStatxWalker`, currently delegates to legacy).
/// - `macos-bulk` (`MacosBulkWalker`, currently delegates to legacy).
/// - `legacy` (portable `ignore`-crate walker, all platforms).
#[derive(Debug, Default)]
pub struct WalkerFactory {
    chosen: OnceLock<&'static str>,
}

impl WalkerFactory {
    /// Creates a new `WalkerFactory`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            chosen: OnceLock::new(),
        }
    }
}

impl PortFactory<dyn DirWalkerPort> for WalkerFactory {
    fn build(&self, caps: &Capabilities) -> Arc<dyn DirWalkerPort> {
        #[cfg(target_os = "linux")]
        if matches!(
            caps.walker_tier,
            WalkerTier::LinuxStatx | WalkerTier::LinuxIouring | WalkerTier::LinuxLegacy
        ) {
            let _ = self.chosen.set("linux-statx");
            return Arc::new(crate::walker::linux::LinuxStatxWalker::new());
        }

        #[cfg(target_os = "macos")]
        if matches!(
            caps.walker_tier,
            WalkerTier::MacosBulk | WalkerTier::MacosLegacy
        ) {
            let _ = self.chosen.set("macos-bulk");
            return Arc::new(crate::walker::macos::MacosBulkWalker::new());
        }

        // Suppress unused-variable warning when cfg guards above consume `caps`.
        let _ = caps;

        let _ = self.chosen.set("legacy");
        Arc::new(LegacyWalker::new())
    }

    fn chosen_tier(&self) -> &'static str {
        self.chosen.get().copied().unwrap_or("legacy")
    }
}
