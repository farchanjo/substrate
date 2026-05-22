//! `FsIndexPort` — inbound port for the optional in-process filesystem index per ADR-0041.
//!
//! Implemented by `InProcessFsIndex` in `substrate-fs-index` (adapter crate, behind
//! the `fs-index` Cargo feature). When the feature is disabled, the composition root
//! wires a `NoopFsIndex` Null Object.

use async_trait::async_trait;

use crate::errors::SubstrateResult;
use crate::value_objects::JailedPath;

// ---- Cancel signal abstraction ---------------------------------------------
//
// Design choice: define a thin `CancelSignal` trait in the domain rather than
// pulling `tokio-util` into `substrate-domain`. This keeps the domain free of
// the tokio ecosystem while allowing adapters to implement the trait using
// `tokio_util::sync::CancellationToken` internally.
//
// The alternative — adding `tokio-util` to substrate-domain deps — would be
// acceptable per ADR-0003 ecosystem norms but would introduce a dependency on
// a tokio sub-crate that has no non-async use in the domain layer. The thin
// trait approach keeps the dependency graph cleaner and makes the domain more
// portable (e.g., for unit tests without a tokio runtime).

/// An opaque signal that communicates cooperative cancellation to long-running operations.
///
/// Adapter implementations back this with `tokio_util::sync::CancellationToken`.
/// Domain code that receives a `&dyn CancelSignal` may poll `is_cancelled` for
/// point-in-time checks, or `await cancelled()` to suspend until cancellation fires.
#[async_trait]
pub trait CancelSignal: Send + Sync {
    /// Returns `true` if cancellation has already been requested.
    ///
    /// This is a cheap, non-blocking check suitable for use inside loops.
    fn is_cancelled(&self) -> bool;

    /// Resolves when this signal is cancelled.
    ///
    /// Intended for use in `tokio::select!` as a cancellation arm.
    async fn cancelled(&self);
}

// ---- Index query type -------------------------------------------------------

/// Query parameters for an index lookup per ADR-0041.
///
/// The full query surface will be expanded in subsequent waves when the
/// filesystem-query adapter is implemented.
#[derive(Debug, Clone)]
pub struct IndexQuery {
    /// Root path to scope the search within.
    pub root: JailedPath,

    /// Optional glob pattern for filename matching.
    // TODO: expand to full glob + depth + filter options in the fs-query ADR wave.
    pub glob: Option<String>,

    /// Maximum number of results to return (0 = unbounded).
    pub limit: usize,
}

// ---- Port trait -------------------------------------------------------------

/// Inbound port for the optional in-process filesystem index per ADR-0041.
///
/// When the `fs-index` feature is not compiled in, the composition root wires
/// a `NoopFsIndex` Null Object that always returns an empty result set and
/// forces the caller to fall back to the `ignore`-crate walk path (ADR-0003 Zone B).
#[async_trait]
pub trait FsIndexPort: Send + Sync {
    /// Queries the index for paths matching `query`.
    ///
    /// Returns an empty `Vec` when the index is disabled or the root has not
    /// been indexed yet, signalling the caller to fall back to a full walk.
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_INVALID_ARGUMENT` — malformed query.
    /// - `SUBSTRATE_NOT_FOUND` — root path not in the index and fallback is required.
    async fn lookup(&self, query: &IndexQuery) -> SubstrateResult<Vec<JailedPath>>;

    /// Invalidates cached entries under `path`.
    ///
    /// Called by mutation tools after committing a write to ensure subsequent
    /// `lookup` calls observe the updated state. Invalidation is best-effort;
    /// the lazy lstat pass is the inviolable safety net per ADR-0041.
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_INTERNAL_ERROR` — index state is corrupt and cannot be partially invalidated.
    async fn invalidate(&self, path: &JailedPath) -> SubstrateResult<()>;

    /// Triggers a full or incremental index rebuild for `root`.
    ///
    /// Long-running; must be driven within a `spawn_blocking` or async Zone B task.
    /// Cooperative cancellation is checked at each directory boundary via `cancel`.
    ///
    /// # Errors
    ///
    /// - `SUBSTRATE_CANCELLED` — `cancel` fired before the rebuild completed.
    /// - `SUBSTRATE_IO_ERROR` — hardware or kernel I/O failure during the walk.
    async fn rebuild_root(
        &self,
        root: &JailedPath,
        cancel: &dyn CancelSignal,
    ) -> SubstrateResult<()>;
}
