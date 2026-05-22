//! `DirWalkerPort` — inbound port for native-tier directory walking per ADR-0041.
//!
//! The implementation tier is selected by `DirWalkerFactory` at startup and
//! depends on `caps.walker_tier`. Implementations live in adapter crates.

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::errors::SubstrateResult;
use crate::value_objects::JailedPath;

/// A single directory entry produced by a walk operation.
///
/// The full surface (symlink target, extended attributes, inode details) will
/// be fleshed out when the filesystem-query adapter is implemented.
// TODO: expand DirEntry fields in the fs-query adapter wave.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    /// The jailed, canonical path to this entry.
    pub path: JailedPath,

    /// `true` when this entry is a directory.
    pub is_dir: bool,

    /// Size in bytes for regular files; `None` for directories and special files.
    pub size_bytes: Option<u64>,
}

/// Walk configuration options.
///
/// The full option surface (depth limit, gitignore handling, hidden files,
/// follow symlinks) will be added when the filesystem-query adapter is
/// implemented.
// TODO: expand WalkOpts in the fs-query adapter wave per ADR-0003 Zone B contract.
#[derive(Debug, Clone, Default)]
pub struct WalkOpts {
    /// Maximum recursion depth; `None` means unlimited.
    pub max_depth: Option<usize>,
}

/// Inbound port for recursive directory walking per ADR-0041.
///
/// Produces a `Stream` of `DirEntry` results rather than a `Vec` to support
/// backpressure and early termination via stream drop. The stream is `'a` to
/// borrow from the walker and its root path argument.
///
/// This trait is synchronous (returns a stream, not an async iterator) because
/// the stream itself drives the async I/O. Adapter implementations wrap the
/// platform-native walk primitive in a `futures::stream::unfold`.
pub trait DirWalkerPort: Send + Sync {
    /// Returns a stream of directory entries under `root`.
    ///
    /// # Cancel-safety
    ///
    /// Dropping the returned stream before exhaustion cancels the walk.
    /// Implementations MUST NOT hold OS resources (e.g., open directory handles)
    /// after the stream is dropped.
    fn walk<'a>(
        &'a self,
        root: &'a JailedPath,
        opts: WalkOpts,
    ) -> BoxStream<'a, SubstrateResult<DirEntry>>;
}
