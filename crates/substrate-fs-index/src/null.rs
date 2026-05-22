//! Null Object `NullFsIndex` — zero-cost fallback when `fs-index` is off per ADR-0041.
//!
//! When the `fs-index` Cargo feature is not compiled in, `FsIndexFactory::build`
//! returns a `NullFsIndex`. Every method returns immediately:
//! - `lookup` returns an empty `Vec`, signalling callers to fall back to the
//!   `ignore`-crate walk path (ADR-0003 Zone B).
//! - `invalidate` is a no-op.
//! - `rebuild_root` is a no-op (completes instantly without any I/O).
//!
//! `NullFsIndex` is also used in integration tests that want to verify fallback
//! behaviour without enabling the full index machinery.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::instrument;

use substrate_domain::ports::fs_index::{CancelSignal, FsIndexPort, IndexQuery};
use substrate_domain::{JailedPath, SubstrateResult};

/// Null Object implementation of `FsIndexPort`.
///
/// All operations are instant no-ops. The adapter crate's composition root
/// emits a `tracing::debug!` at startup when this variant is selected.
#[derive(Debug, Default)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "pub(crate) documents intentional crate-internal visibility for cross-module use"
)]
pub(crate) struct NullFsIndex;

impl NullFsIndex {
    /// Constructs a new `NullFsIndex`.
    #[must_use]
    #[expect(
        dead_code,
        reason = "called only from the cfg(not(feature = \"fs-index\")) path in FsIndexFactory"
    )]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

#[async_trait]
impl FsIndexPort for NullFsIndex {
    #[instrument(skip(self, _query), fields(root = ?_query.root))]
    async fn lookup(&self, _query: &IndexQuery) -> SubstrateResult<Vec<JailedPath>> {
        // Empty result signals the caller to fall back to a full walk.
        Ok(Vec::new())
    }

    #[instrument(skip(self, path), fields(path = %path))]
    async fn invalidate(&self, path: &JailedPath) -> SubstrateResult<()> {
        tracing::trace!(path = %path, "NullFsIndex::invalidate — no-op");
        Ok(())
    }

    #[instrument(skip(self, root, _cancel), fields(root = %root))]
    async fn rebuild_root(
        &self,
        root: &JailedPath,
        _cancel: &dyn CancelSignal,
    ) -> SubstrateResult<()> {
        tracing::trace!(root = %root, "NullFsIndex::rebuild_root — no-op");
        Ok(())
    }
}
