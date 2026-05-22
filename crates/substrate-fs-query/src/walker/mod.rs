//! Walker tier implementations for the `DirWalkerPort`.
//!
//! Tier selection follows ADR-0041 and ADR-0042:
//!
//! - `legacy`: portable `ignore`-crate-based walker (all platforms, tier N).
//! - `linux`: cfg-gated; stubs that delegate to `legacy` until Wave G+ implements
//!   `statx(2)` / `getdents64` batch (TODO).
//! - `macos`: cfg-gated; stubs that delegate to `legacy` until Wave G+ implements
//!   `getattrlistbulk(2)` (TODO).

pub mod legacy;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;
