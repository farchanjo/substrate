//! macOS-native filesystem index tier per ADR-0041 and ADR-0042.
//!
//! `MacOsBulkIndex` is the tier-1 (preferred) index implementation on macOS.
//! It uses `getattrlistbulk(2)` for batch stat — a single syscall returns
//! name + metadata for up to 1024 entries per call, reducing syscall count
//! compared to `readdir` + per-file `lstat`.
//!
//! # Safety exception
//!
//! `getattrlistbulk(2)` is called via `libc` using raw FFI. This module is the
//! sole exception to the workspace-wide `forbid(unsafe_code)` rule, as permitted
//! by ADR-0042 §macOS Native Primitive Exception and the forthcoming ADR-0044
//! (No Subprocess + SIMD / Low-Level Syscall Exception Policy). The `unsafe`
//! blocks are constrained to `getattrlistbulk.rs` (a submodule of this module)
//! and are annotated with SAFETY comments that justify every invariant.
//!
//! # Async-zone classification
//!
//! - Rebuild walk: Zone B (`spawn_blocking`); can block for seconds on large trees.
//! - `getattrlistbulk` calls within the walk: Zone B (already inside `spawn_blocking`).
//! - Snapshot swap: wait-free for readers (`ArcSwap`); no blocking.
//!
//! # TODO (future adapter wave)
//!
//! - Implement the `getattrlistbulk` inner loop in `getattrlistbulk.rs`
//!   (the submodule skeleton below contains the FFI declarations but not the
//!   parsing logic for the returned attrlist buffer).
//! - Wire `FSEvents` watcher (Layer 2) when `fs-index-watch` feature is active.

// MacOsBulkIndex is pub(crate) inside a private module; clippy::redundant_pub_crate
// fires because the enclosing `mod macos` is private, but unreachable_pub fires on
// bare `pub`. Suppress redundant_pub_crate at module level and use pub(crate).
#![expect(
    clippy::redundant_pub_crate,
    reason = "pub(crate) in private module avoids unreachable_pub; \
              MacOsBulkIndex is referenced from sibling modules in the same crate"
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

// ---- getattrlistbulk FFI skeleton ------------------------------------------
// TODO: move to a dedicated getattrlistbulk.rs submodule in the fs-query wave.
// The declarations below are stubs that will be completed when the native walk
// replaces the ignore-crate baseline in rebuild::walk_root.

/// `getattrlistbulk(2)` attribute common info struct — stubbed for ADR skeleton.
///
/// The real implementation parses the kernel-returned attrlist buffer which has
/// a variable-length packed format. A full parser will be implemented in the
/// fs-query adapter wave.
#[repr(C)]
#[allow(non_camel_case_types, dead_code)]
struct attrlist {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

// ---- MacOsBulkIndex --------------------------------------------------------

/// macOS `getattrlistbulk(2)`-accelerated index implementation.
///
/// Selected by `FsIndexFactory` when `caps.has_getattrlistbulk` is true
/// (macOS 10.10+) and the `macos-getattrlistbulk` Cargo feature is compiled in.
/// Falls back to `PortablePollingIndex` otherwise.
#[derive(Debug)]
pub(crate) struct MacOsBulkIndex {
    slot: SnapshotSlot,
    write_through: WriteThroughHandle,
}

impl MacOsBulkIndex {
    /// Constructs a new `MacOsBulkIndex`.
    #[must_use]
    #[cfg_attr(
        not(feature = "macos-getattrlistbulk"),
        expect(
            dead_code,
            reason = "called only when macos-getattrlistbulk feature is active; the call site \
                      in factory.rs is itself cfg-gated on that feature, so this constructor is \
                      genuinely dead when the feature is off"
        )
    )]
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
        reason = "called only by fs-mutation adapter when macos-getattrlistbulk feature is active"
    )]
    pub(crate) fn write_through_handle(&self) -> WriteThroughHandle {
        self.write_through.clone()
    }
}

#[async_trait]
impl FsIndexPort for MacOsBulkIndex {
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
            // `*current` dereferences the ArcSwap guard to `Arc<IndexSnapshot>`.
            // Cloning an `Arc<T>` yields another `Arc<T>`, not a `T`, so no
            // further wrapping is needed: `slot.store` accepts `Arc<IndexSnapshot>`.
            let new_snap: Arc<IndexSnapshot> = Arc::clone(&*current);
            // Build a mutable snapshot by cloning the inner value, evicting the
            // prefix, then publishing the updated snapshot.
            let mut updated = (*new_snap).clone();
            updated.evict_prefix(&path_clone);
            slot.store(Arc::new(updated));
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
            // TODO: replace with getattrlistbulk inner loop for batch metadata.
            // Currently uses the ignore-crate walk as the portable baseline.
            // The native getattrlistbulk path will be wired in the fs-query wave.
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
