//! Linux native walker — `nix::dir::Dir` (readdir_r / getdents64 family) +
//! `nix::sys::stat::statx` for per-entry metadata in a single syscall.
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
//! Raw `getdents64(2)` would shave one syscall per batch but `nix::dir::Dir`
//! already wraps `readdir_r` which uses the same kernel path. Raw getdents64 is
//! deferred as a Wave I micro-optimisation.
//!
//! # Safety justification (ADR-0042 + ADR-0044)
//!
//! No `unsafe` code is required in this module: `nix::dir::Dir` and
//! `nix::sys::stat::statx` are safe wrappers. The module-level
//! `#[allow(unsafe_code)]` below is intentionally absent.

use futures::stream::BoxStream;
use nix::{
    dir::{Dir, Type},
    fcntl::OFlag,
    sys::{
        stat::{statx, StatxFlags, StatxMask},
    },
    sys::stat::Mode,
};
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

/// How many entries to process between CancellationToken checks (ADR-0037).
const CANCEL_CHECK_INTERVAL: usize = 256;

/// Minimal `statx` attribute mask: type, mode, size, mtime.
/// Using the smallest mask reduces kernel I/O per entry.
const STATX_MASK: StatxMask = StatxMask::STATX_TYPE
    .union(StatxMask::STATX_MODE)
    .union(StatxMask::STATX_SIZE)
    .union(StatxMask::STATX_MTS);

// ---- Walker ------------------------------------------------------------------

/// Linux-native directory walker backed by `nix::dir::Dir` and `statx(2)`.
///
/// Uses `nix::dir::Dir` (readdir_r / getdents64 family) to enumerate directory
/// entries and `statx(2)` for per-entry metadata in a single syscall per entry,
/// reducing total syscall count versus the legacy `ignore`-crate path.
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
    pub fn with_cancel(cancel: CancellationToken) -> Self {
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

        let tx_clone = tx.clone();
        let max_depth = opts.max_depth;

        tokio::task::spawn_blocking(move || {
            walk_dir_recursive(
                &root_path,
                max_depth,
                0,
                &tx_clone,
                &cancel,
                &mut 0usize,
            );
            // tx_clone dropped here — rx will observe stream end.
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
    if let Some(limit) = max_depth {
        if current_depth > limit {
            return;
        }
    }

    // Open the directory.
    let dir = match Dir::open(dir_path, OFlag::O_RDONLY | OFlag::O_DIRECTORY, Mode::empty()) {
        Ok(d) => d,
        Err(err) => {
            let _ = tx.blocking_send(Err(substrate_domain::SubstrateError::IoError {
                path: dir_path.display().to_string(),
                correlation_id: None,
            }));
            tracing::debug!(%err, path = %dir_path.display(), "linux walker: cannot open dir");
            return;
        }
    };

    // Collect entries first to avoid holding the Dir handle while recursing.
    let mut subdirs: Vec<std::path::PathBuf> = Vec::new();

    for entry_result in dir.into_iter() {
        // Cooperative cancellation check every CANCEL_CHECK_INTERVAL entries.
        *counter = counter.wrapping_add(1);
        if *counter % CANCEL_CHECK_INTERVAL == 0 && cancel.is_cancelled() {
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
            }
        };

        // Skip "." and "..".
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == "." || name_str == ".." {
            continue;
        }

        let entry_path = dir_path.join(name_str.as_ref());

        // Use statx for metadata.
        let (is_dir, size_bytes) = match statx(
            nix::libc::AT_FDCWD,
            &entry_path,
            StatxFlags::AT_NO_AUTOMOUNT | StatxFlags::AT_SYMLINK_NOFOLLOW,
            STATX_MASK,
        ) {
            Ok(sx) => {
                let kind = sx.stx_mode as u32 & 0o0170000; // S_IFMT bits
                let is_dir = kind == 0o0040000; // S_IFDIR
                let is_reg = kind == 0o0100000; // S_IFREG
                let size = if is_reg { Some(sx.stx_size) } else { None };
                (is_dir, size)
            }
            Err(_) => {
                // Fallback: use the d_type from the dirent if available.
                let is_dir = entry.file_type() == Some(Type::Directory);
                (is_dir, None)
            }
        };

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
        assert!(paths.iter().any(|p| p.ends_with("beta.txt")), "beta.txt not found");
        assert!(paths.iter().any(|p| p.ends_with("gamma.txt")), "gamma.txt not found");
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
