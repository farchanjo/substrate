//! Linux directory walker.
//!
//! # Implementation notes
//!
//! Zone B: the entire walk runs inside `tokio::task::spawn_blocking`. Entries
//! are streamed back to the async caller through a `tokio::sync::mpsc::channel`
//! with a bounded buffer of 64 items to provide back-pressure.
//!
//! Cancellation (ADR-0037): the blocking task checks the `CancellationToken`
//! every 256 entries. On cancellation, the mpsc sender is dropped which causes
//! the async receiver stream to terminate cleanly.
//!
//! # Current tier: portable `std::fs`, not `statx(2)`-accelerated
//!
//! This was originally written against `nix::dir::Dir` + `nix::sys::stat::statx`
//! for a `getdents64`/`statx` batched-syscall fast path, but neither symbol is
//! reachable in this workspace's pinned `nix = "0.30"` feature set (`dir` is
//! feature-gated and not enabled; `statx` is not exposed by this nix version at
//! all) — the code had never actually been compiled on Linux until verified in
//! a real Linux environment. `DirWalkerPort::walk` here is a portable
//! `std::fs::read_dir` + `symlink_metadata` walk in the meantime, matching
//! `WalkerFactory`'s own documented "currently delegates to legacy" tier
//! description. A real `statx`-accelerated implementation (raw
//! `libc::statx`/`getdents64`, mirroring the `substrate-policy` macOS
//! `O_NOFOLLOW_ANY` unsafe carve-out pattern) remains a Wave I
//! micro-optimisation, not done here.

use futures::stream::BoxStream;
use substrate_domain::{
    SubstrateResult,
    ports::dir_walker::{DirEntry, DirWalkerPort, WalkOpts},
    value_objects::jailed_path::JailedPath,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ---- Constants ---------------------------------------------------------------

/// mpsc channel depth — limits memory in flight between the blocking walker
/// and the async consumer.
const CHANNEL_DEPTH: usize = 64;

/// How many entries to process between `CancellationToken` checks (ADR-0037).
const CANCEL_CHECK_INTERVAL: usize = 256;

// ---- Walker ------------------------------------------------------------------

/// Linux directory walker. See module docs for the current (portable, not
/// `statx`-accelerated) implementation status.
#[derive(Debug, Default)]
pub struct LinuxStatxWalker {
    /// Cancellation token for cooperative early-exit.
    cancel: CancellationToken,
}

impl LinuxStatxWalker {
    /// Creates a new `LinuxStatxWalker` with a fresh `CancellationToken`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
        }
    }

    /// Creates a walker that shares a caller-owned `CancellationToken`.
    #[must_use]
    pub const fn with_cancel(cancel: CancellationToken) -> Self {
        Self { cancel }
    }
}

impl DirWalkerPort for LinuxStatxWalker {
    fn walk<'a>(
        &'a self,
        root: &'a JailedPath,
        opts: WalkOpts,
    ) -> BoxStream<'a, SubstrateResult<DirEntry>> {
        let root_path = root.as_path().to_path_buf();
        let cancel = self.cancel.clone();

        // Zone B: open + walk in a blocking thread; stream results via mpsc.
        let (tx, rx) = mpsc::channel::<SubstrateResult<DirEntry>>(CHANNEL_DEPTH);

        let max_depth = opts.max_depth;

        tokio::task::spawn_blocking(move || {
            walk_dir_recursive(&root_path, max_depth, 0, &tx, &cancel, &mut 0usize);
            // tx dropped here — rx will observe stream end.
        });

        // Convert the mpsc receiver into a Stream.
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Box::pin(stream)
    }
}

// ---- Recursive walker (blocking, runs in spawn_blocking) --------------------

/// Recursively walks `dir_path`, emitting `DirEntry` values through `tx`.
///
/// `counter` is shared across the full recursion so that the cancel-check
/// interval fires at global entry count, not per-directory count.
fn walk_dir_recursive(
    dir_path: &std::path::Path,
    max_depth: Option<usize>,
    current_depth: usize,
    tx: &mpsc::Sender<SubstrateResult<DirEntry>>,
    cancel: &CancellationToken,
    counter: &mut usize,
) {
    // Depth limit.
    if let Some(limit) = max_depth
        && current_depth > limit
    {
        return;
    }

    // Open the directory.
    let read_dir = match std::fs::read_dir(dir_path) {
        Ok(d) => d,
        Err(err) => {
            let _ = tx.blocking_send(Err(substrate_domain::SubstrateError::IoError {
                path: dir_path.display().to_string(),
                correlation_id: None,
            }));
            tracing::debug!(%err, path = %dir_path.display(), "linux walker: cannot open dir");
            return;
        },
    };

    // Collect entries first to avoid holding the directory handle while recursing.
    let mut subdirs: Vec<std::path::PathBuf> = Vec::new();

    for entry_result in read_dir {
        // Cooperative cancellation check every CANCEL_CHECK_INTERVAL entries.
        *counter = counter.wrapping_add(1);
        if counter.is_multiple_of(CANCEL_CHECK_INTERVAL) && cancel.is_cancelled() {
            return;
        }

        let entry = match entry_result {
            Ok(e) => e,
            Err(err) => {
                let _ = tx.blocking_send(Err(substrate_domain::SubstrateError::IoError {
                    path: dir_path.display().to_string(),
                    correlation_id: None,
                }));
                tracing::debug!(%err, "linux walker: readdir error");
                continue;
            },
        };

        let entry_path = entry.path();

        // DirEntry::metadata() is lstat-based (does not follow symlinks),
        // matching the prior statx AT_SYMLINK_NOFOLLOW semantics.
        let (is_dir, size_bytes) = entry.metadata().map_or_else(
            |_| {
                // Fallback: use the d_type from the dirent if the kernel provided it.
                let is_dir = entry.file_type().is_ok_and(|ft| ft.is_dir());
                (is_dir, None)
            },
            |meta| {
                let is_dir = meta.is_dir();
                let size = if meta.is_file() { Some(meta.len()) } else { None };
                (is_dir, size)
            },
        );

        let jailed = JailedPath::new_jailed(entry_path.clone());

        if tx
            .blocking_send(Ok(DirEntry {
                path: jailed,
                is_dir,
                size_bytes,
            }))
            .is_err()
        {
            // Receiver dropped — consumer cancelled or channel full-and-closed.
            return;
        }

        if is_dir {
            subdirs.push(entry_path);
        }
    }

    // Recurse into subdirectories.
    for subdir in subdirs {
        if cancel.is_cancelled() {
            return;
        }
        walk_dir_recursive(&subdir, max_depth, current_depth + 1, tx, cancel, counter);
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module — panics and unwraps on assertion failure are the intended behavior"
)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use tokio::runtime::Runtime;

    fn make_tree() -> TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("alpha.txt"), b"aaa").expect("write alpha");
        fs::write(dir.path().join("beta.txt"), b"bb").expect("write beta");
        fs::create_dir(dir.path().join("sub")).expect("mkdir sub");
        fs::write(dir.path().join("sub/gamma.txt"), b"g").expect("write gamma");
        dir
    }

    #[test]
    fn walks_all_entries() {
        let dir = make_tree();
        let rt = Runtime::new().expect("runtime");
        let walker = LinuxStatxWalker::new();
        let root = JailedPath::new_jailed(dir.path().to_path_buf());
        let opts = WalkOpts { max_depth: None };

        let entries: Vec<_> = rt.block_on(async {
            use futures::StreamExt;
            walker.walk(&root, opts).collect().await
        });

        // Expect at least: alpha.txt, beta.txt, sub/, sub/gamma.txt
        let paths: Vec<_> = entries
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|e| e.path.as_path().to_path_buf())
            .collect();
        assert!(
            paths.iter().any(|p| p.ends_with("alpha.txt")),
            "alpha.txt not found; got {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.ends_with("beta.txt")),
            "beta.txt not found"
        );
        assert!(
            paths.iter().any(|p| p.ends_with("gamma.txt")),
            "gamma.txt not found"
        );
    }

    #[test]
    fn respects_max_depth_zero() {
        let dir = make_tree();
        let rt = Runtime::new().expect("runtime");
        let walker = LinuxStatxWalker::new();
        let root = JailedPath::new_jailed(dir.path().to_path_buf());
        let opts = WalkOpts { max_depth: Some(0) };

        let entries: Vec<_> = rt.block_on(async {
            use futures::StreamExt;
            walker.walk(&root, opts).collect().await
        });

        // With max_depth=0, only entries directly in root should appear.
        // sub/gamma.txt must NOT appear.
        let has_deep = entries
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .any(|e| e.path.as_path().ends_with("gamma.txt"));
        assert!(!has_deep, "depth-limited walk should not descend into sub/");
    }

    #[test]
    fn files_have_size_bytes() {
        let dir = make_tree();
        let rt = Runtime::new().expect("runtime");
        let walker = LinuxStatxWalker::new();
        let root = JailedPath::new_jailed(dir.path().to_path_buf());
        let opts = WalkOpts { max_depth: None };

        let entries: Vec<_> = rt.block_on(async {
            use futures::StreamExt;
            walker.walk(&root, opts).collect().await
        });

        let alpha = entries
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .find(|e| e.path.as_path().ends_with("alpha.txt"))
            .expect("alpha.txt must be in results");
        assert_eq!(alpha.size_bytes, Some(3), "alpha.txt is 3 bytes");
    }
}
