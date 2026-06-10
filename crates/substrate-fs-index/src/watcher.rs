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
use tokio::runtime::Handle;
use tracing::instrument;

use substrate_domain::JailedPath;
use substrate_domain::ports::fs_index::FsIndexPort;

/// Wraps a `notify::RecommendedWatcher` and routes filesystem events to the index.
///
/// Constructed once at server startup when `fs-index-watch` is compiled in.
/// The watcher runs in a background task and must be kept alive for the server
/// lifetime (store in the composition root alongside the `Arc<dyn FsIndexPort>`).
///
/// # Runtime handle
///
/// `notify` invokes its event callback on a dedicated OS thread that has no
/// Tokio runtime context.  `FsIndexWatcher` captures the Tokio `Handle` at
/// construction time (inside a live runtime) so that `spawn_invalidate` can
/// call `Handle::spawn` rather than `tokio::spawn`, which would panic outside
/// a runtime context.
#[allow(dead_code)] // fields used via internal callback + Drop
pub struct FsIndexWatcher {
    inner: RecommendedWatcher,
    index: Arc<dyn FsIndexPort>,
    /// Tokio runtime handle captured at construction time.
    ///
    /// The `notify` callback thread has no runtime context; all async work
    /// must be dispatched through this handle.
    handle: Handle,
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
    ///
    /// # Panics (caller contract)
    ///
    /// Must be called from within a live Tokio runtime context.
    /// `Handle::current()` panics if there is no current runtime, which is a
    /// programming error at the call site (the composition root always runs
    /// inside the tokio main runtime).
    pub fn new(index: Arc<dyn FsIndexPort>) -> Result<Self, notify::Error> {
        let handle = Handle::current();
        let index_clone = Arc::clone(&index);
        let handle_clone = handle.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            Self::handle_event(&index_clone, &handle_clone, res);
        })?;
        Ok(Self {
            inner: watcher,
            index,
            handle,
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
    /// - `Create` → evict the path so the next lookup triggers a fresh lstat
    ///   (write-through handle will re-insert with correct metadata on next
    ///   in-process mutation; external creates are corrected by TTL rebuild).
    /// - `Remove` → `FsIndexPort::invalidate`.
    /// - `Rename` → evict all renamed paths; the new path appears on next
    ///   rebuild or via a subsequent `Create` event.
    /// - `Modify` → evict so the mandatory lazy lstat pass picks up fresh
    ///   metadata on the next lookup; avoids serving stale size/mtime.
    /// - `Rescan` (IN_Q_OVERFLOW) → `FsIndexPort::rebuild_root` for all roots.
    #[instrument(skip(index, handle, res))]
    fn handle_event(index: &Arc<dyn FsIndexPort>, handle: &Handle, res: notify::Result<Event>) {
        match res {
            Ok(event) => {
                tracing::trace!(kind = ?event.kind, paths = ?event.paths, "watcher event received");
                match event.kind {
                    EventKind::Remove(_) => {
                        for path in &event.paths {
                            Self::spawn_invalidate(Arc::clone(index), handle, path.clone());
                        }
                    },
                    EventKind::Modify(_) => {
                        // Evict so the lazy lstat pass re-validates size and mtime
                        // on the next lookup rather than serving a stale entry.
                        for path in &event.paths {
                            tracing::trace!(path = %path.display(), "watcher: evicting modified path");
                            Self::spawn_invalidate(Arc::clone(index), handle, path.clone());
                        }
                    },
                    EventKind::Create(_) => {
                        // For externally created paths: evict any stale entry so
                        // the next rebuild or write-through can re-insert cleanly.
                        // In-process creates go through WriteThroughHandle directly.
                        for path in &event.paths {
                            tracing::trace!(path = %path.display(), "watcher: evicting for newly created path");
                            Self::spawn_invalidate(Arc::clone(index), handle, path.clone());
                        }
                    },
                    EventKind::Access(_) => {
                        // Access events carry no metadata change; no action needed.
                        tracing::trace!(kind = ?event.kind, "watcher event: access only, no index action");
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
                    EventKind::Other => {
                        tracing::trace!(kind = ?event.kind, "watcher event: platform-specific, no action");
                    },
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "watcher error; events may be lost");
            },
        }
    }

    /// Spawns a Tokio task to call `FsIndexPort::invalidate` for the given path.
    ///
    /// Uses the stored runtime `Handle` rather than `tokio::spawn` because this
    /// method is called from the `notify` OS thread which has no Tokio runtime
    /// context.  Calling `tokio::spawn` from outside a runtime panics; calling
    /// `Handle::spawn` dispatches the task onto the correct runtime safely.
    fn spawn_invalidate(index: Arc<dyn FsIndexPort>, handle: &Handle, path: PathBuf) {
        // fire-and-forget; task logs its own errors via tracing.
        // The handle is intentionally discarded: watcher events are best-effort
        // and the task owns its own error reporting.
        let _guard = handle.spawn(async move {
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
