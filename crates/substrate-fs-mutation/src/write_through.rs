//! Optional write-through updates to the filesystem index per ADR-0041 Layer 1.
//!
//! Every mutation that commits a file change MUST call the appropriate helper
//! here when the `fs-index` Cargo feature is enabled. These calls are
//! fire-and-forget: index staleness is non-fatal; the lazy lstat pass in
//! `substrate-fs-index` is the authoritative safety net.
//!
//! When `fs-index` is disabled the public API in this module is empty
//! (conditional compilation removes the bodies) so callers can use
//! `#[cfg(feature = "fs-index")]` guards around each call site or
//! call the helpers unconditionally with no-op stubs.

#[cfg(feature = "fs-index")]
use std::sync::Arc;

#[cfg(feature = "fs-index")]
use substrate_domain::{FsIndexPort, JailedPath};

/// Notifies the index that a new or updated file was committed at `path`.
///
/// Only compiled when `fs-index` is enabled. The invalidation is
/// fire-and-forget: the error, if any, is logged at WARN level and
/// swallowed — a stale index is corrected by the lazy lstat pass.
#[cfg(feature = "fs-index")]
pub fn on_upsert(index: &Arc<dyn FsIndexPort>, path: &JailedPath) {
    let index = Arc::clone(index);
    let path = path.clone();
    tokio::task::spawn(async move {
        if let Err(e) = index.invalidate(&path).await {
            tracing::warn!(
                error = %e,
                path = %path,
                "write-through index invalidate failed (non-fatal)"
            );
        }
    });
}

/// Notifies the index that a path was removed (file or empty directory).
///
/// Only compiled when `fs-index` is enabled. Fire-and-forget.
#[cfg(feature = "fs-index")]
pub fn on_remove(index: &Arc<dyn FsIndexPort>, path: &JailedPath) {
    on_upsert(index, path);
}

/// Notifies the index of a rename: evicts both `src` and `dst`.
///
/// Only compiled when `fs-index` is enabled. Fire-and-forget.
#[cfg(feature = "fs-index")]
pub fn on_rename(index: &Arc<dyn FsIndexPort>, src: &JailedPath, dst: &JailedPath) {
    on_upsert(index, src);
    on_upsert(index, dst);
}
