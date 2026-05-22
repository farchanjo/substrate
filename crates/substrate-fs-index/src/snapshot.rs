//! In-process filesystem snapshot per ADR-0041.
//!
//! `IndexSnapshot` holds the current view of indexed paths and supports O(1)
//! prefix lookups and glob-pattern scans. The active snapshot is stored behind
//! `SnapshotSlot` (`Arc<ArcSwap<IndexSnapshot>>`) so readers never block on
//! concurrent rebuilds; writers do a single atomic store.

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use time::OffsetDateTime;

use substrate_domain::JailedPath;

// ---- IndexEntry -------------------------------------------------------------

/// A single path entry in the filesystem snapshot.
///
/// Mirrors the stat fields needed for filter application in the lookup pipeline
/// (ADR-0041 §Lookup Pipeline). `mtime` and `size` are cached from the last
/// stat call; they are re-validated by the mandatory lazy lstat pass before
/// any entry is emitted to a client.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// The jail-validated path.
    pub path: JailedPath,
    /// Modification time captured at index build or write-through update time.
    pub mtime: OffsetDateTime,
    /// File size in bytes (0 for directories).
    pub size: u64,
    /// `true` if this entry is a regular file; `false` if a directory or other.
    pub is_file: bool,
}

// ---- IndexSnapshot ----------------------------------------------------------

/// Immutable snapshot of all indexed filesystem entries for an allowlist root.
///
/// Snapshots are replaced atomically via `SnapshotSlot`; readers always see a
/// consistent view. A new snapshot is built during periodic TTL rebuilds or
/// after an `IN_Q_OVERFLOW` watcher event; the prior snapshot continues to
/// serve reads while the new one is assembled in a Zone B task.
///
/// # Lookup complexity
///
/// - `lookup_by_name`: O(log n) `BTreeMap` lookup + small linear scan over the
///   collision bucket. Name is the unqualified filename only.
/// - `lookup_by_prefix`: O(log n) range scan; returns all entries whose
///   display path starts with `prefix`.
#[derive(Debug, Default, Clone)]
pub struct IndexSnapshot {
    /// Filename-keyed index for fast glob suffix matching.
    ///
    /// Key: unqualified filename (e.g., `"lib.rs"`).
    /// Value: all entries sharing that filename across all watched roots.
    by_name: BTreeMap<String, Vec<IndexEntry>>,

    /// Root-keyed index for full-subtree enumeration.
    ///
    /// Key: a `JailedPath` representing one of the configured allowlist roots.
    /// Value: all entries under that root, in directory-first order.
    by_root: BTreeMap<String, Vec<IndexEntry>>,
}

impl IndexSnapshot {
    /// Returns all entries whose unqualified filename matches `name`.
    ///
    /// Returns an empty slice when no entries are indexed under `name`.
    #[must_use]
    pub fn lookup_by_name(&self, name: &str) -> &[IndexEntry] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }

    /// Returns all entries recorded under `root`.
    ///
    /// Returns an empty slice when `root` has not been indexed.
    #[must_use]
    pub fn lookup_by_root(&self, root: &JailedPath) -> &[IndexEntry] {
        self.by_root
            .get(&root.to_string())
            .map_or(&[], Vec::as_slice)
    }

    /// Returns the total count of path entries in this snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_root.values().map(Vec::len).sum()
    }

    /// Returns `true` when the snapshot contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a rough memory estimate in bytes.
    ///
    /// The estimate accounts for `PathBuf` storage and per-entry metadata.
    /// It does not account for `BTreeMap` node overhead (approx. 48 bytes per node).
    #[must_use]
    pub fn bytes_estimated(&self) -> usize {
        self.by_root
            .values()
            .flat_map(|v| v.iter())
            // PathBuf: heap string (~24 b struct + inline chars)
            // OffsetDateTime: 16 b; u64: 8 b; bool: 1 b; alignment: ~3 b padding
            .map(|e| e.path.as_path().as_os_str().len() + 52)
            .sum()
    }

    /// Inserts a single entry, keying it in both indexes.
    ///
    /// Used by `WriteThroughHandle` for fast single-entry updates.
    pub(crate) fn insert_batch_single(&mut self, file_name: String, entry: IndexEntry) {
        let root_key = entry
            .path
            .as_path()
            .ancestors()
            .nth(1)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.by_name
            .entry(file_name)
            .or_default()
            .push(entry.clone());
        self.by_root.entry(root_key).or_default().push(entry);
    }

    /// Updates the `mtime` and `size` of the entry matching `path`, if present.
    ///
    /// Used by `WriteThroughHandle::on_modify` for in-place file updates.
    pub(crate) fn update_mtime_size(
        &mut self,
        path: &JailedPath,
        mtime: time::OffsetDateTime,
        size: u64,
    ) {
        for bucket in self.by_root.values_mut() {
            for entry in bucket.iter_mut() {
                if entry.path == *path {
                    entry.mtime = mtime;
                    entry.size = size;
                }
            }
        }
        // by_name shares the same IndexEntry values but is a separate Vec;
        // update those too for consistency.
        for bucket in self.by_name.values_mut() {
            for entry in bucket.iter_mut() {
                if entry.path == *path {
                    entry.mtime = mtime;
                    entry.size = size;
                }
            }
        }
    }

    /// Builder entry point: insert a batch of entries under a single `root`.
    ///
    /// Called exclusively from `rebuild::walk_root` after a full walk completes.
    /// Entries whose filename matches `tmp_filter::is_tmp_file` are silently
    /// excluded per ADR-0033 and ADR-0041 §Edge Cases.
    pub(crate) fn insert_batch(&mut self, root: &JailedPath, entries: Vec<IndexEntry>) {
        let root_key = root.to_string();
        for entry in entries {
            let name = entry
                .path
                .as_path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.by_name.entry(name).or_default().push(entry.clone());
            self.by_root
                .entry(root_key.clone())
                .or_default()
                .push(entry);
        }
    }

    /// Removes all entries whose path starts with `prefix`.
    ///
    /// Used by write-through invalidation (Layer 1) and watcher-triggered
    /// partial eviction (Layer 2). Callers produce a new snapshot containing
    /// the remaining entries via `SnapshotSlot::store`.
    pub(crate) fn evict_prefix(&mut self, prefix: &JailedPath) {
        let prefix_str = prefix.to_string();
        for bucket in self.by_root.values_mut() {
            bucket.retain(|e| !e.path.as_path().starts_with(prefix_str.as_str()));
        }
        for bucket in self.by_name.values_mut() {
            bucket.retain(|e| !e.path.as_path().starts_with(prefix_str.as_str()));
        }
        // Remove now-empty buckets to avoid accumulating dead keys.
        self.by_root.retain(|_, v| !v.is_empty());
        self.by_name.retain(|_, v| !v.is_empty());
    }
}

// ---- SnapshotSlot -----------------------------------------------------------

/// An atomically swappable slot holding the current `IndexSnapshot`.
///
/// Readers call `load()` to obtain an `Arc<IndexSnapshot>` with no lock
/// contention. The rebuild task calls `store(Arc::new(new))` to publish a
/// fresh snapshot without blocking readers.
///
/// Wrapping in an outer `Arc` allows multiple owners (e.g., the factory,
/// the write-through handle, and the watcher) to share the same slot.
pub type SnapshotSlot = Arc<ArcSwap<IndexSnapshot>>;

// ---- Unit tests -------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use arc_swap::ArcSwap;
    use time::OffsetDateTime;

    use super::{IndexEntry, IndexSnapshot, SnapshotSlot};
    use substrate_domain::JailedPath;

    fn make_jailed(s: &str) -> JailedPath {
        // SAFETY (semantic): test-only constructor; invariants not enforced.
        // `new_unchecked` is `pub(crate)` in substrate-domain, so we use the
        // Display/From path via PathBuf directly for tests in this crate.
        // In production, JailedPath values always originate from substrate-policy.
        //
        // Since new_unchecked is pub(crate) to substrate-domain we cannot call it
        // here. We use serde round-trip as a workaround to construct test values.
        let p = PathBuf::from(s);
        serde_json::from_value(serde_json::json!(p)).expect("test helper: serde round-trip")
    }

    fn make_entry(path: &str, is_file: bool) -> IndexEntry {
        IndexEntry {
            path: make_jailed(path),
            mtime: OffsetDateTime::UNIX_EPOCH,
            size: 0,
            is_file,
        }
    }

    #[test]
    fn snapshot_lookup_by_name_returns_entries() {
        let mut snap = IndexSnapshot::default();
        let root = make_jailed("/tmp/root");
        snap.insert_batch(
            &root,
            vec![
                make_entry("/tmp/root/a/lib.rs", true),
                make_entry("/tmp/root/b/lib.rs", true),
            ],
        );
        let hits = snap.lookup_by_name("lib.rs");
        assert_eq!(hits.len(), 2, "expected two entries for lib.rs");
    }

    #[test]
    fn evict_prefix_removes_entries_from_both_indexes() {
        let mut snap = IndexSnapshot::default();
        let root = make_jailed("/tmp/root");
        snap.insert_batch(
            &root,
            vec![
                make_entry("/tmp/root/keep/a.rs", true),
                make_entry("/tmp/root/evict/b.rs", true),
            ],
        );
        snap.evict_prefix(&make_jailed("/tmp/root/evict"));
        assert!(
            snap.lookup_by_name("b.rs").is_empty(),
            "evicted entry must not appear in by_name"
        );
        let root_entries = snap.lookup_by_root(&root);
        assert!(
            root_entries
                .iter()
                .all(|e| !e.path.as_path().starts_with("/tmp/root/evict")),
            "evicted prefix must not appear in by_root"
        );
    }

    #[test]
    fn snapshot_slot_atomic_swap_is_observable() {
        let slot: SnapshotSlot = Arc::new(ArcSwap::from_pointee(IndexSnapshot::default()));
        assert!(slot.load().is_empty(), "initial snapshot must be empty");

        let mut new_snap = IndexSnapshot::default();
        let root = make_jailed("/tmp/root");
        new_snap.insert_batch(&root, vec![make_entry("/tmp/root/foo.rs", true)]);
        slot.store(Arc::new(new_snap));

        assert_eq!(
            slot.load().len(),
            1,
            "post-swap snapshot must reflect the new entry"
        );
    }
}
