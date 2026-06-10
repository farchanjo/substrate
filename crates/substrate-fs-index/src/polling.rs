//! `PortablePollingIndex` — cross-platform fallback index per ADR-0042.
//!
//! Selected by `FsIndexFactory` when neither the Linux-statx tier nor the
//! macOS-getattrlistbulk tier is available (e.g., containers with locked-down
//! kernels, or the corresponding Cargo features are not compiled in).
//!
//! `PortablePollingIndex` uses:
//! - The `ignore` crate for directory walking (Zone B, `spawn_blocking`).
//! - `tokio::time::interval` for TTL-based periodic rebuilds (Layer 3).
//! - Write-through updates via `WriteThroughHandle` (Layer 1).
//!
//! A `tracing::info!` is emitted at construction time noting the degraded
//! performance posture relative to the native tiers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use tokio::task;
use tracing::instrument;

use substrate_domain::ports::fs_index::{CancelSignal, FsIndexPort, IndexQuery};
use substrate_domain::{JailedPath, SubstrateResult};

use crate::rebuild;
use crate::snapshot::{IndexSnapshot, SnapshotSlot};
use crate::write_through::WriteThroughHandle;

/// Cross-platform index implementation using the `ignore` crate walker.
///
/// Used as the tier-N (fallback) implementation in the capability cascade from
/// ADR-0042 when no native-primitive tier is compiled in or available at runtime.
#[derive(Debug)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "pub(crate) documents intentional crate-internal visibility for cross-module use"
)]
pub(crate) struct PortablePollingIndex {
    slot: SnapshotSlot,
    // Held here for ownership; write-through callers access it via write_through_handle()
    write_through: WriteThroughHandle,
}

impl PortablePollingIndex {
    /// Constructs a new `PortablePollingIndex` and emits a startup info message.
    ///
    /// The warning communicates to operators that the native-tier index is
    /// unavailable on this platform or kernel, and that index performance
    /// will be limited by the `ignore`-crate walk speed per ADR-0042.
    /// Wired by `FsIndexFactory::build` when the `fs-index` feature is active.
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        tracing::info!(
            tier = "portable",
            "FsIndex is using the portable polling tier; \
             native-tier (linux-statx, macos-getattrlistbulk) is unavailable. \
             Index rebuild will use the ignore-crate full walk per ADR-0042."
        );
        let slot: SnapshotSlot = Arc::new(ArcSwap::from_pointee(IndexSnapshot::default()));
        let write_through = WriteThroughHandle::new(Arc::clone(&slot));
        Arc::new(Self {
            slot,
            write_through,
        })
    }

    /// Returns a clone of the `WriteThroughHandle` for use by mutation crates.
    // Wave G+: wired by MCP server composition root (fs-mutation adapter)
    #[expect(
        dead_code,
        reason = "Wave G+: wired by fs-mutation adapter composition root"
    )]
    #[must_use]
    pub(crate) fn write_through_handle(&self) -> WriteThroughHandle {
        self.write_through.clone()
    }
}

#[async_trait]
impl FsIndexPort for PortablePollingIndex {
    #[instrument(skip(self, query), fields(root = ?query.root, glob = ?query.glob))]
    async fn lookup(&self, query: &IndexQuery) -> SubstrateResult<Vec<JailedPath>> {
        let snap = self.slot.load();
        let candidates: Vec<JailedPath> = query.glob.as_ref().map_or_else(
            || {
                snap.lookup_by_root(&query.root)
                    .iter()
                    .map(|e| e.path.clone())
                    .collect()
            },
            |glob| {
                let glob_str = glob.clone();
                snap.lookup_by_name(&glob_str)
                    .iter()
                    .map(|e| e.path.clone())
                    .collect()
            },
        );
        // Apply limit per ADR-0041. 0 = unbounded.
        let results = if query.limit == 0 {
            candidates
        } else {
            candidates.into_iter().take(query.limit).collect()
        };
        Ok(results)
    }

    #[instrument(skip(self, path), fields(path = %path))]
    async fn invalidate(&self, path: &JailedPath) -> SubstrateResult<()> {
        let path_clone = path.clone();
        let slot = Arc::clone(&self.slot);
        task::spawn_blocking(move || {
            let current = slot.load();
            let mut new_snap = (**current).clone();
            new_snap.evict_prefix(&path_clone);
            slot.store(Arc::new(new_snap));
        })
        .await
        .map_err(|e| substrate_domain::SubstrateError::InternalError {
            reason: format!("invalidate spawn_blocking panicked: {e}"),
            correlation_id: None,
        })
    }

    #[instrument(skip(self, root, cancel), fields(root = %root))]
    async fn rebuild_root(
        &self,
        root: &JailedPath,
        cancel: &dyn CancelSignal,
    ) -> SubstrateResult<()> {
        let root_clone = root.clone();
        let slot = Arc::clone(&self.slot);

        // Bridge the `&dyn CancelSignal` (non-`'static`) into the `spawn_blocking`
        // closure (which requires `'static` captures) via a shared `AtomicBool`.
        //
        // Strategy: share an `Arc<AtomicBool>` between the async context and the
        // blocking closure.  In the async context we `tokio::select!` between the
        // blocking task completing and `cancel.cancelled()` firing.  When the
        // cancellation arm wins, we set the flag so that the still-running
        // blocking walk sees it at the next 256-entry boundary check (ADR-0037).
        //
        // This replaces the previous `let is_cancelled = cancel.is_cancelled()`
        // frozen-bool pattern, which was only evaluated once at dispatch time and
        // therefore never reflected cancellations that arrived mid-walk.
        let cancel_flag = Arc::new(AtomicBool::new(cancel.is_cancelled()));
        let flag_for_closure = Arc::clone(&cancel_flag);

        let blocking_task = task::spawn_blocking(move || {
            // Re-read the live AtomicBool at each 256-entry boundary.
            // The flag is set by the cancellation arm of the select below
            // while the walk is in progress, giving true mid-walk cancellation.
            let cancel_fn = move || flag_for_closure.load(Ordering::Acquire);
            rebuild::walk_root(&root_clone, &cancel_fn)
        });

        // Drive the blocking task while concurrently watching for cancellation.
        // Per ADR-0037 the work future is the biased-first arm; the cancel arm
        // sets the flag and returns early, allowing the blocking task to drain
        // at its next checkpoint.
        let snap_result = tokio::select! {
            biased;
            result = blocking_task => {
                result.map_err(|e| substrate_domain::SubstrateError::InternalError {
                    reason: format!("rebuild spawn_blocking panicked: {e}"),
                    correlation_id: None,
                })?
            },
            () = cancel.cancelled() => {
                // Signal the blocking walk to stop at its next boundary check.
                cancel_flag.store(true, Ordering::Release);
                return Err(substrate_domain::SubstrateError::Cancelled {
                    correlation_id: None,
                });
            },
        };

        slot.store(Arc::new(snap_result?));
        Ok(())
    }
}

impl Default for PortablePollingIndex {
    fn default() -> Self {
        // Only reachable in tests; use `new()` for all production paths.
        let slot: SnapshotSlot = Arc::new(ArcSwap::from_pointee(IndexSnapshot::default()));
        let write_through = WriteThroughHandle::new(Arc::clone(&slot));
        Self {
            slot,
            write_through,
        }
    }
}
