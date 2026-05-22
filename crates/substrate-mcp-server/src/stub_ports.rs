//! Null-object stub implementation of the filesystem-index port.
//!
//! `NullFsIndex` lets the MCP server compile and start before the concrete
//! `substrate-fs-index` adapter is fully wired. Its `lookup` returns an empty
//! result set so callers fall back to a full directory walk (ADR-0003 Zone B).
//!
//! The async job control-plane is NOT stubbed: the composition root always
//! wires `substrate_jobs::InMemoryJobRegistry` (ADR-0040), applying
//! `JobConfig::default()` when the `[jobs]` TOML section is omitted. The former
//! `NullJobRegistry` was removed because "jobs disabled" is not a valid mode —
//! it silently broke every Bucket B/C tool.
//!
//! Per ADR-0010 there is no dedicated `SUBSTRATE_NOT_IMPLEMENTED` variant in the
//! error taxonomy; the remaining stub uses `SUBSTRATE_INTERNAL_ERROR` with a
//! descriptive message.

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

use async_trait::async_trait;

use substrate_domain::{
    FsIndexPort, JailedPath, SubstrateError, SubstrateResult,
    ports::fs_index::{CancelSignal, IndexQuery},
};

// ---- NullFsIndex -------------------------------------------------------------

/// Null-object `FsIndexPort` stub; returns empty results for `lookup` and
/// `SUBSTRATE_INTERNAL_ERROR` for `rebuild_root`.
/// Replaced by `FsIndexFactory::build` in Wave D.
#[expect(
    dead_code,
    reason = "replaced by FsIndexFactory in composition; retained for testing"
)]
#[derive(Debug)]
pub(crate) struct NullFsIndex;

#[async_trait]
impl FsIndexPort for NullFsIndex {
    /// Always returns an empty result set, causing callers to fall back to a
    /// full walk via the `ignore` crate (ADR-0003 Zone B behaviour).
    async fn lookup(&self, _query: &IndexQuery) -> SubstrateResult<Vec<JailedPath>> {
        Ok(Vec::new())
    }

    async fn invalidate(&self, _path: &JailedPath) -> SubstrateResult<()> {
        // Null object: invalidation is a no-op; index has no state to evict.
        Ok(())
    }

    async fn rebuild_root(
        &self,
        _root: &JailedPath,
        _cancel: &dyn CancelSignal,
    ) -> SubstrateResult<()> {
        Err(stub_error("NullFsIndex.rebuild_root"))
    }
}

// ---- Helpers -----------------------------------------------------------------

#[expect(
    dead_code,
    reason = "only referenced by NullFsIndex, itself a dead-code-retained Wave-D scaffold"
)]
fn stub_error(method: &str) -> SubstrateError {
    SubstrateError::InternalError {
        reason: format!(
            "{method}: stub not yet implemented — replace with concrete adapter in Wave D"
        ),
        correlation_id: None,
    }
}
