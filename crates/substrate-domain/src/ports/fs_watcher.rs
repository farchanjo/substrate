//! `FsWatcherPort` — inbound port for filesystem change notification per ADR-0041.
//!
//! Tier is selected by `FsWatcherFactory` at startup. When no kernel watcher
//! is available, `PollingWatcher` (Null Object) is used and a `tracing::warn!`
//! is emitted.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::errors::SubstrateResult;
use crate::value_objects::JailedPath;

/// A filesystem change event produced by the active watcher tier.
///
/// The full event surface (create/modify/delete/rename/overflow) will be
/// expanded when the fs-index-watch feature is implemented.
// TODO: expand WatchEvent variants in the fs-index-watch adapter wave.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchEvent {
    /// The path affected by this event.
    pub path: JailedPath,

    /// Human-readable event kind (e.g., `"create"`, `"modify"`, `"delete"`).
    pub kind: String,
}

/// Inbound port for filesystem change notification per ADR-0041.
///
/// Implemented by `InotifyWatcher`, `FanotifyWatcher`, `FsEventsWatcher`,
/// `KqueueWatcher`, and `PollingWatcher` (Null Object) in adapter crates.
#[async_trait]
pub trait FsWatcherPort: Send + Sync {
    /// Starts watching `root` and delivers events to `callback`.
    ///
    /// Returns a `WatchGuard` that stops watching when dropped.
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_INTERNAL_ERROR` — the kernel watcher could not be initialised.
    async fn watch(
        &self,
        root: &JailedPath,
        callback: Box<dyn Fn(WatchEvent) + Send + Sync + 'static>,
    ) -> SubstrateResult<WatchGuard>;
}

/// An opaque guard that cancels the watch subscription when dropped.
///
/// Dropping the guard must be idempotent; calling `drop` twice must not panic.
pub struct WatchGuard(Box<dyn FnOnce() + Send + 'static>);

impl WatchGuard {
    /// Constructs a `WatchGuard` from a cleanup closure.
    #[must_use]
    pub fn new(cleanup: impl FnOnce() + Send + 'static) -> Self {
        Self(Box::new(cleanup))
    }
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        // The Box<dyn FnOnce()> cannot be called via &mut self; we use a trick
        // to move it out. We replace with a no-op and call the original.
        let cleanup = std::mem::replace(&mut self.0, Box::new(|| {}));
        cleanup();
    }
}

impl std::fmt::Debug for WatchGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchGuard").finish_non_exhaustive()
    }
}
