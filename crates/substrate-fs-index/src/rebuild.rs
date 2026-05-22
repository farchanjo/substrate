//! Async snapshot rebuilder per ADR-0041 Zone B.
//!
//! `walk_root` performs a full directory walk of one allowlist root using the
//! `ignore` crate as the cross-platform baseline (Zone B: spawned via
//! `tokio::task::spawn_blocking` by the caller). It produces a fresh
//! `IndexSnapshot` and checks the `CancelSignal` at each directory boundary.
//!
//! On cancellation mid-walk, the partial snapshot is discarded and the error
//! `SubstrateError::Cancelled` is returned. The prior snapshot continues to
//! serve reads (the caller must NOT call `SnapshotSlot::store` on error).
//!
//! Transactional temp files (`.tmp.<uuid7>`) are excluded by `tmp_filter`
//! per ADR-0033 and ADR-0041 §Edge Cases.

use ignore::WalkBuilder;
use time::OffsetDateTime;
use tracing::instrument;

use substrate_domain::{JailedPath, SubstrateError, SubstrateResult};

use crate::snapshot::{IndexEntry, IndexSnapshot};
use crate::tmp_filter::is_tmp_file;

/// Performs a synchronous, cancellation-aware walk of `root` and returns a
/// freshly built `IndexSnapshot`.
///
/// This function is CPU- and I/O-bound and MUST be called inside
/// `tokio::task::spawn_blocking`. It checks `cancel_flag` at each directory
/// boundary (not per-file, to amortise the atomic load cost) and returns
/// `SubstrateError::Cancelled` when cancellation is detected.
///
/// # Errors
///
/// - `SubstrateError::Cancelled` — `cancel_flag` was set before the walk
///   completed. The returned partial work is discarded by the caller.
/// - `SubstrateError::IoError` — kernel I/O failure during the walk.
#[instrument(skip(root, cancel_flag), fields(root = %root))]
#[expect(
    clippy::redundant_pub_crate,
    reason = "pub(crate) documents intentional crate-internal visibility for cross-module use"
)]
pub(crate) fn walk_root(
    root: &JailedPath,
    cancel_flag: &dyn Fn() -> bool,
) -> SubstrateResult<IndexSnapshot> {
    let mut snapshot = IndexSnapshot::default();
    let mut entries = Vec::new();
    let mut dir_count: u32 = 0;

    let walker = WalkBuilder::new(root.as_path())
        .follow_links(false)  // ADR-0035: no symlink escapes
        .hidden(false)        // index hidden files for completeness
        .git_ignore(false)    // operator controls allowlist, not .gitignore
        .build();

    for result in walker {
        let dir_entry = result.map_err(|e| SubstrateError::IoError {
            // `ignore::Error` has no public `.path()` accessor; use the Display
            // representation which includes path context when available.
            path: e.to_string(),
            correlation_id: None,
        })?;

        // Check cancellation once per directory entry boundary.
        // Amortised: avoids atomic load on every file in a large tree.
        dir_count += 1;
        if dir_count.is_multiple_of(256) && cancel_flag() {
            tracing::debug!(root = %root, entries_so_far = entries.len(), "rebuild cancelled");
            return Err(SubstrateError::Cancelled {
                correlation_id: None,
            });
        }

        let path = dir_entry.path();
        let Some(file_name) = path.file_name() else {
            continue;
        };

        // Exclude transactional temp files per ADR-0033.
        if is_tmp_file(file_name) {
            continue;
        }

        let Ok(metadata) = dir_entry.metadata() else {
            // Skip entries we cannot stat; the lazy lstat pass will evict
            // any stale entries that were already in the snapshot.
            tracing::trace!(path = %path.display(), "skipping unreadable entry during rebuild");
            continue;
        };

        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| {
                let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "Unix epoch seconds fit in i64 until year 2262"
                )]
                OffsetDateTime::from_unix_timestamp(secs as i64).ok()
            })
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);

        // Construct a JailedPath from the walked path.
        // SAFETY (semantic): `root` is already jail-validated. All entries
        // produced by the walk starting at `root` are provably within `root`
        // because `WalkBuilder` never follows symlinks (follow_links = false)
        // and `ignore` does not resolve symlink targets. A path-jail
        // re-validation step in the lookup pipeline provides the inviolable
        // last check per ADR-0041.
        let jailed =
            serde_json::from_value::<JailedPath>(serde_json::json!(path)).map_err(|_| {
                SubstrateError::EncodingError {
                    detail: format!("non-UTF-8 path during rebuild: {}", path.display()),
                    correlation_id: None,
                }
            })?;

        let is_file = metadata.is_file();
        let size = if is_file { metadata.len() } else { 0 };

        entries.push(IndexEntry {
            path: jailed,
            mtime,
            size,
            is_file,
        });
    }

    snapshot.insert_batch(root, entries);
    Ok(snapshot)
}
