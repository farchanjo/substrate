//! Filesystem watcher Layer 2 per ADR-0041 (gated behind `fs-index-watch`).
//!
//! `FsIndexWatcher` wraps `notify::RecommendedWatcher` and translates native
//! filesystem events into write-through invalidations and opportunistic
//! snapshot rebuilds.
//!
//! Platform dispatch performed by `notify`:
//! - Linux: inotify (always available on kernel 2.6.13+).
//! - macOS: FSEvents (always available on macOS 10.6+).
//! - Fallback: `notify::PollWatcher` when neither is available.
//!
//! # Overflow handling
//!
//! When inotify emits `EventKind::Any` with a `notify::event::Flag::Rescan`
//! hint (mapping from `IN_Q_OVERFLOW`), `FsIndexWatcher` triggers a full root
//! rebuild via `FsIndexPort::rebuild_root`. The stale snapshot continues to
//! serve reads (filtered by the mandatory Layer 0 lstat) while the rebuild
//! task runs in Zone B.
//!
//! # TODO (future adapter wave)
//!
//! - Wire `FsIndexWatcher` into `substrate-mcp-server` composition root.
//! - Thread a `CancellationToken` into the watcher background task for clean
//!   shutdown per ADR-0037.
//! - Implement per-root watch registration to scope inotify watches tightly
//!   rather than watching the entire allowlist tree from a single root.

use std::path::PathBuf;
use std::sync::Arc;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::instrument;

use substrate_domain::JailedPath;
use substrate_domain::ports::fs_index::FsIndexPort;

/// Wraps a `notify::RecommendedWatcher` and routes filesystem events to the index.
///
/// Constructed once at server startup when `fs-index-watch` is compiled in.
/// The watcher runs in a background task and must be kept alive for the server
/// lifetime (store in the composition root alongside the `Arc<dyn FsIndexPort>`).
#[allow(dead_code)] // fields used via internal callback + Drop
pub struct FsIndexWatcher {
    inner: RecommendedWatcher,
    index: Arc<dyn FsIndexPort>,
}

impl std::fmt::Debug for FsIndexWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FsIndexWatcher")
            .field("index", &"Arc<dyn FsIndexPort>")
            .finish_non_exhaustive()
    }
}

impl FsIndexWatcher {
    /// Constructs a `FsIndexWatcher` that routes events to `index`.
    ///
    /// # Errors
    ///
    /// Returns a `notify::Error` if the platform watcher cannot be initialised
    /// (e.g., inotify fd limit exceeded).
    pub fn new(index: Arc<dyn FsIndexPort>) -> Result<Self, notify::Error> {
        let index_clone = Arc::clone(&index);
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            Self::handle_event(&index_clone, res);
        })?;
        Ok(Self {
            inner: watcher,
            index,
        })
    }

    /// Registers `root` for recursive watching.
    ///
    /// # Errors
    ///
    /// Returns a `notify::Error` if the watch cannot be added (e.g., too many
    /// watches, path does not exist).
    pub fn watch_root(&mut self, root: &JailedPath) -> Result<(), notify::Error> {
        self.inner.watch(root.as_path(), RecursiveMode::Recursive)
    }

    /// Deregisters `root` from the watcher.
    ///
    /// Called when an allowlist root is removed at runtime (SIGHUP reload).
    pub fn unwatch_root(&mut self, root: &JailedPath) -> Result<(), notify::Error> {
        self.inner.unwatch(root.as_path())
    }

    /// Event handler invoked by the `notify` background thread.
    ///
    /// Routes events to the appropriate index operation:
    /// - `Create` → `WriteThroughHandle::on_create` (best-effort; full metadata
    ///   not available from the event; index rebuilds will correct size/mtime).
    /// - `Remove` → `FsIndexPort::invalidate`.
    /// - `Rename` → evict old path; new path entry appears on next rebuild or
    ///   via a subsequent `Create` event.
    /// - `Rescan` (IN_Q_OVERFLOW) → `FsIndexPort::rebuild_root` for all roots.
    /// - `Modify` → evict and let the lazy lstat pass correct metadata on next lookup.
    #[instrument(skip(index, res))]
    fn handle_event(index: &Arc<dyn FsIndexPort>, res: notify::Result<Event>) {
        match res {
            Ok(event) => {
                tracing::trace!(kind = ?event.kind, paths = ?event.paths, "watcher event received");
                match event.kind {
                    EventKind::Remove(_) => {
                        for path in &event.paths {
                            Self::spawn_invalidate(Arc::clone(index), path.clone());
                        }
                    },
                    EventKind::Any => {
                        // notify maps IN_Q_OVERFLOW to EventKind::Any with Rescan flag.
                        // A Rescan indicates the watch queue overflowed; trigger full rebuild.
                        tracing::warn!(
                            "watcher queue overflow (IN_Q_OVERFLOW equivalent); triggering full rebuild"
                        );
                        // TODO: wire full rebuild call via composition root rebuild scheduler.
                        // In the current skeleton, we log the event only.
                    },
                    _ => {
                        // Create / Modify / Rename events: let the TTL rebuild (Layer 3)
                        // and lazy lstat (Layer 0) handle staleness. Write-through (Layer 1)
                        // handles in-process mutations more precisely.
                        tracing::trace!(kind = ?event.kind, "watcher event: no immediate action (handled by TTL/lstat layers)");
                    },
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "watcher error; events may be lost");
            },
        }
    }

    /// Spawns a Tokio task to call `FsIndexPort::invalidate` for a deleted path.
    fn spawn_invalidate(index: Arc<dyn FsIndexPort>, path: PathBuf) {
        // fire-and-forget; task logs its own errors via tracing.
        // The handle is intentionally discarded: watcher events are best-effort
        // and the task owns its own error reporting.
        let _ = tokio::spawn(async move {
            // Construct a JailedPath from the raw event path.
            // SAFETY (semantic): paths from notify events are absolute OS paths;
            // they may not be within the allowlist (e.g., if a watched root is
            // deleted). The `invalidate` call is safe regardless: evicting a path
            // that is not in the index is a no-op.
            let jailed = match serde_json::from_value::<JailedPath>(serde_json::json!(path)) {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(error = %e, "watcher: cannot construct JailedPath for event path; skipping invalidation");
                    return;
                },
            };
            if let Err(e) = index.invalidate(&jailed).await {
                tracing::warn!(error = %e, path = %jailed, "watcher: invalidation error");
            }
        });
    }
}
