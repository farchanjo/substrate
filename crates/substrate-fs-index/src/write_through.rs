//! Write-through invalidation handle per ADR-0041 Layer 1.
//!
//! Mutation crates (`substrate-fs-mutation`) call `WriteThroughHandle` methods
//! at atomic-rename commit time (after the `rename(2)` call succeeds, before
//! the tool response is returned to the client). This keeps the snapshot
//! consistent with in-process mutations at zero extra I/O cost: the path and
//! metadata are already known from the commit.
//!
//! `WriteThroughHandle` performs an atomic snapshot swap: it loads the current
//! snapshot, clones it, applies the targeted mutation, and stores the new
//! snapshot via `SnapshotSlot`. Because `ArcSwap::store` is wait-free for
//! readers, concurrent `fs.find` calls observe either the pre-mutation or
//! post-mutation snapshot; never a partial state.
//!
//! External mutations (files changed by processes outside substrate) are handled
//! by Layers 2 (watcher) and 3 (TTL rebuild) + the mandatory Layer 0 lstat pass.
//! This layer covers only in-process mutations.

use std::sync::Arc;

use time::OffsetDateTime;
use tracing::instrument;

use substrate_domain::JailedPath;

use crate::snapshot::{IndexEntry, IndexSnapshot, SnapshotSlot};

/// A cheaply-cloneable handle that write-through-updates the shared snapshot slot.
///
/// Cloning increments an `Arc` refcount (one pointer copy); it does not copy
/// the snapshot. Mutation adapters should hold one clone per tool handler and
/// call the appropriate `on_*` method after the commit point.
#[derive(Debug, Clone)]
pub struct WriteThroughHandle {
    slot: SnapshotSlot,
}

impl WriteThroughHandle {
    /// Constructs a new handle wrapping the given snapshot slot.
    #[must_use]
    pub const fn new(slot: SnapshotSlot) -> Self {
        Self { slot }
    }

    /// Notifies the index that a new file or directory was created at `path`.
    ///
    /// Inserts a synthetic entry with the provided metadata. Called by
    /// `fs.mkdir`, `fs.write`, `fs.copy`, `fs.symlink`, and `fs.touch`
    /// at commit time.
    #[instrument(skip(self), fields(path = %path))]
    pub fn on_create(&self, path: &JailedPath, is_file: bool, size: u64) {
        let entry = IndexEntry {
            path: path.clone(),
            mtime: OffsetDateTime::now_utc(),
            size,
            is_file,
        };
        self.apply(|snap| {
            let file_name = path
                .as_path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            snap.insert_batch_single(file_name, entry);
        });
    }

    /// Notifies the index that `path` was deleted.
    ///
    /// Evicts all entries under `path` (handles both file and directory removes).
    /// Called by `fs.remove` at commit time.
    #[instrument(skip(self), fields(path = %path))]
    pub fn on_remove(&self, path: &JailedPath) {
        let path = path.clone();
        self.apply(|snap| snap.evict_prefix(&path));
    }

    /// Notifies the index that `from` was atomically renamed to `to`.
    ///
    /// Evicts `from` entries and inserts `to` with carried-over metadata.
    /// Called by `fs.rename` at commit time.
    #[instrument(skip(self), fields(from = %from, to = %to))]
    pub fn on_rename(&self, from: &JailedPath, to: &JailedPath, is_file: bool, size: u64) {
        let from = from.clone();
        let to = to.clone();
        let entry = IndexEntry {
            path: to.clone(),
            mtime: OffsetDateTime::now_utc(),
            size,
            is_file,
        };
        self.apply(|snap| {
            snap.evict_prefix(&from);
            let file_name = to
                .as_path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            snap.insert_batch_single(file_name, entry);
        });
    }

    /// Notifies the index that `path` was modified in-place (e.g., `fs.write` to
    /// an existing file, `fs.set_permissions`).
    ///
    /// Updates the cached `mtime` and `size` for the entry.
    /// Called at commit time by `fs.write` (append/truncate) and
    /// `fs.set_permissions`.
    #[instrument(skip(self), fields(path = %path))]
    pub fn on_modify(&self, path: &JailedPath, new_size: u64) {
        let path = path.clone();
        let new_mtime = OffsetDateTime::now_utc();
        self.apply(|snap| snap.update_mtime_size(&path, new_mtime, new_size));
    }

    /// Applies a mutation function to a cloned snapshot and atomically stores
    /// the result.
    ///
    /// The load-clone-mutate-store sequence is linearisable for readers: they
    /// see either the pre-mutation or post-mutation snapshot, never a partial one.
    fn apply<F>(&self, f: F)
    where
        F: FnOnce(&mut IndexSnapshot),
    {
        let current = self.slot.load();
        let mut new_snap = (**current).clone();
        f(&mut new_snap);
        self.slot.store(Arc::new(new_snap));
    }
}
