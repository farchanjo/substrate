//! `PortFactory<P>` — Abstract Factory trait for capability-aware adapter selection.
//!
//! Each port (`DirWalker`, `FsWatcher`, `PathJail`, `Hash`, `Stat`, `FsIndex`) has a
//! corresponding factory implementation in the adapter crate. The factory inspects the
//! `Capabilities` snapshot and constructs the highest-ranked tier implementation per ADR-0042.
//!
//! `GoF` patterns applied:
//! - Abstract Factory: one `PortFactory<P>` per port, selects among Strategy impls.
//! - Strategy: each tier is a distinct impl of the port trait.
//! - Singleton: `OnceLock<Capabilities>` ensures probe runs exactly once.

use std::sync::Arc;

use crate::capabilities::Capabilities;

/// Abstract factory that constructs a port implementation based on detected capabilities.
///
/// Implementations live in adapter crates (never in `substrate-domain`).
/// The composition root (`substrate-mcp-server`) calls `build` once and stores the
/// resulting `Arc<P>` for the lifetime of the server process.
///
/// # Example (adapter side)
///
/// ```ignore
/// struct DirWalkerFactory;
///
/// impl PortFactory<dyn DirWalkerPort> for DirWalkerFactory {
///     fn build(&self, caps: &Capabilities) -> Arc<dyn DirWalkerPort> {
///         match caps.walker_tier {
///             WalkerTier::LinuxIouring => Arc::new(IoUringWalker::new()),
///             WalkerTier::LinuxStatx => Arc::new(StatxWalker::new()),
///             _ => Arc::new(StdfsWalker::new()),
///         }
///     }
///
///     fn chosen_tier(&self) -> &'static str { "linux-iouring" }
/// }
/// ```
pub trait PortFactory<P: ?Sized + Send + Sync + 'static>: Send + Sync {
    /// Constructs an `Arc`-wrapped port implementation appropriate for the
    /// detected capability snapshot.
    fn build(&self, caps: &Capabilities) -> Arc<P>;

    /// Returns the tier string identifier chosen during the last `build` call.
    ///
    /// Used by the startup audit event (`SUBSTRATE_CAPABILITY_TIERS_SELECTED`)
    /// to record which implementation was selected for each port.
    fn chosen_tier(&self) -> &'static str;
}
