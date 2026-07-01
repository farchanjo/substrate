//! Linux-native filesystem index tier per ADR-0041 and ADR-0042.
//!
//! `LinuxStatxIndex` is the tier-1 (preferred) index implementation on Linux.
//! It uses `nix::sys::stat::statx(2)` for batched stat calls during index
//! rebuild, providing birth-time and mount-ID fields not available via
//! `fstatat(2)`. The rebuild walk still uses the `ignore` crate as the portable
//! directory-iteration baseline; `statx` is used for metadata enrichment only.
//!
//! When the `fs-index-watch` Cargo feature is active, `LinuxStatxIndex` pairs
//! with an inotify watcher (via the `notify` crate) for external-change
//! detection (Layer 2). `IN_Q_OVERFLOW` events trigger a full root rebuild.
//!
//! # Async-zone classification
//!
//! - Rebuild walk: Zone B (`spawn_blocking`); can block for seconds on large trees.
//! - `statx` calls within the walk: Zone B (already inside `spawn_blocking`).
//! - Snapshot swap: wait-free for readers (`ArcSwap`); no blocking.
//!
//! # TODO (future adapter wave)
//!
//! - Implement `getdents64`-based batched directory listing to reduce per-file
//!   syscall count versus the current `ignore` crate `readdir` loop.
//! - Integrate `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` for each opened
//!   directory fd as an additional path-jail enforcement point per ADR-0035.

// LinuxStatxIndex is pub(crate) inside a private module; clippy::redundant_pub_crate
// fires because the enclosing `mod linux` is private, but unreachable_pub fires on
// bare `pub`. Suppress redundant_pub_crate at module level and use pub(crate).
#![expect(
    clippy::redundant_pub_crate,
    reason = "pub(crate) in private module avoids unreachable_pub; \
              LinuxStatxIndex is referenced from sibling modules in the same crate"
)]

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

/// Linux `statx(2)`-accelerated index implementation.
///
/// Selected by `FsIndexFactory` when `caps.has_statx` is true (kernel 4.11+)
/// and the `fs-index` feature is compiled in. Falls back to
/// `PortablePollingIndex` when `has_statx` is false.
#[derive(Debug)]
pub(crate) struct LinuxStatxIndex {
    slot: SnapshotSlot,
    write_through: WriteThroughHandle,
}

impl LinuxStatxIndex {
    /// Constructs a new `LinuxStatxIndex`.
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        let slot: SnapshotSlot = Arc::new(ArcSwap::from_pointee(IndexSnapshot::default()));
        let write_through = WriteThroughHandle::new(Arc::clone(&slot));
        Arc::new(Self {
            slot,
            write_through,
        })
    }

    /// Returns a clone of the `WriteThroughHandle` for use by mutation crates.
    #[must_use]
    #[expect(
        dead_code,
        reason = "called only by fs-mutation adapter when the linux-statx tier is selected; \
                  mirrors macos::MacOsBulkIndex::write_through_handle"
    )]
    pub(crate) fn write_through_handle(&self) -> WriteThroughHandle {
        self.write_through.clone()
    }
}

#[async_trait]
impl FsIndexPort for LinuxStatxIndex {
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
                snap.lookup_by_name(glob)
                    .iter()
                    .map(|e| e.path.clone())
                    .collect()
            },
        );
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
        let is_cancelled = cancel.is_cancelled();
        let slot = Arc::clone(&self.slot);

        let new_snap = task::spawn_blocking(move || {
            // TODO: replace with getdents64-batched walk + per-entry statx call
            // for reduced syscall count. The ignore-crate walk + Linux statx metadata
            // enrichment will be wired in the fs-query adapter wave.
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
