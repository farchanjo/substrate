//! Portable `ignore`-crate-based directory walker — walker tier N.
//!
//! This is the universal fallback walker used on all platforms when no
//! native-tier implementation is available. It respects `.gitignore` files
//! and supports max-depth limiting.
//!
//! Zone B: `ignore::WalkBuilder` is a synchronous iterator; callers MUST
//! wrap `DirWalkerPort::walk` usage in `spawn_blocking`.

use futures::stream::{self, BoxStream};
use ignore::WalkBuilder;
use substrate_domain::{
    SubstrateResult,
    ports::dir_walker::{DirEntry, DirWalkerPort, WalkOpts},
    value_objects::jailed_path::JailedPath,
};

/// Portable directory walker backed by the `ignore` crate.
///
/// Selected when no native tier (`linux-statx`, `macos-bulk`) is available.
/// Emits `SubstrateResult<DirEntry>` stream entries; errors on individual
/// entries are forwarded rather than aborting the walk.
#[derive(Debug, Default)]
pub struct LegacyWalker;

impl LegacyWalker {
    /// Creates a new `LegacyWalker`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DirWalkerPort for LegacyWalker {
    /// Walks `root` with `opts.max_depth` limiting.
    ///
    /// The returned stream drives a synchronous `ignore::WalkBuilder` iterator
    /// inside the stream's poll function. Callers in async context MUST use
    /// `tokio::task::spawn_blocking` to avoid blocking the executor.
    fn walk<'a>(
        &'a self,
        root: &'a JailedPath,
        opts: WalkOpts,
    ) -> BoxStream<'a, SubstrateResult<DirEntry>> {
        let root_path = root.as_path().to_path_buf();
        let mut builder = WalkBuilder::new(&root_path);
        if let Some(depth) = opts.max_depth {
            builder.max_depth(Some(depth));
        }
        builder.follow_links(false).hidden(false);

        let entries: Vec<SubstrateResult<DirEntry>> = builder
            .build()
            .map(|result| {
                result
                    .map_err(|e| substrate_domain::SubstrateError::IoError {
                        path: e.to_string(),
                        correlation_id: None,
                    })
                    .map(|entry| {
                        let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
                        let size_bytes = entry
                            .metadata()
                            .ok()
                            .filter(std::fs::Metadata::is_file)
                            .map(|m| m.len());
                        DirEntry {
                            path: JailedPath::new_jailed(entry.path().to_path_buf()),
                            is_dir,
                            size_bytes,
                        }
                    })
            })
            .collect();

        Box::pin(stream::iter(entries))
    }
}
