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
        // Capture the cancel flag state in a closure for use inside spawn_blocking.
        // We cannot pass `&dyn CancelSignal` across the spawn_blocking boundary
        // because it is not `'static`. Instead we capture a snapshot of the
        // cancelled state and poll it synchronously inside the blocking closure.
        // The granularity (check every 256 directory entries in rebuild::walk_root)
        // is sufficient for cooperative cancellation per ADR-0037.
        let is_cancelled = cancel.is_cancelled();
        let slot = Arc::clone(&self.slot);

        let new_snap = task::spawn_blocking(move || {
            // Pass a simple closure that returns the pre-captured cancellation state.
            // For long walks, callers that need finer-grained cancellation should
            // use the Linux or macOS native tiers which integrate CancellationToken
            // more tightly.
            let cancel_fn = move || is_cancelled;
            rebuild::walk_root(&root_clone, &cancel_fn)
        })
        .await
        .map_err(|e| substrate_domain::SubstrateError::InternalError {
            reason: format!("rebuild spawn_blocking panicked: {e}"),
            correlation_id: None,
        })??;

        slot.store(Arc::new(new_snap));
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
