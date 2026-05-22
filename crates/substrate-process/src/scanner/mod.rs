//! Platform-neutral process scanner trait.
//!
//! Concrete implementations live in `linux.rs` (procfs) and `macos.rs`
//! (sysctl), selected by `#[cfg(target_os)]` gating per ADR-0028.
//!
//! The trait is object-safe so that `substrate-mcp-server` can hold
//! `Arc<dyn ProcessScannerPort>` without knowing the concrete type.

use crate::process_info::ProcessInfo;
use substrate_domain::SubstrateResult;

/// Port trait for reading the current process table.
///
/// Implementations MUST NOT spawn subprocesses (ADR-0044). All data is
/// gathered via platform-native syscalls or pure-Rust crates.
pub trait ProcessScannerPort: Send + Sync + 'static {
    /// Returns all visible processes as a flat list.
    ///
    /// Errors are per-process; a failed read of one entry is skipped and does
    /// not cause the entire scan to fail. Only a systemic failure (e.g., `/proc`
    /// not mounted, `sysctl` permission error) returns `Err`.
    ///
    /// # Errors
    ///
    /// Returns [`substrate_domain::SubstrateError::InternalError`] when the
    /// kernel or `/proc` enumeration syscall fails with a systemic error.
    fn scan_all(&self) -> SubstrateResult<Vec<ProcessInfo>>;
}

// ---- Platform dispatch -----------------------------------------------------

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

// ---- Default constructor helpers for the composition root ------------------

/// Constructs the platform-appropriate scanner.
///
/// This function is the single point where the composition root (or tests)
/// creates a scanner without knowing which platform it is running on.
#[must_use]
pub fn default_scanner() -> std::sync::Arc<dyn ProcessScannerPort> {
    #[cfg(target_os = "linux")]
    {
        std::sync::Arc::new(linux::LinuxProcessScanner::new())
    }
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(macos::MacOsProcessScanner::new())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        compile_error!("substrate-process requires Linux or macOS");
    }
}
