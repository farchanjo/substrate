//! `StreamChunkObserver` — push-channel abstraction for subprocess stdout/stderr chunks per ADR-0054.
//!
//! Implementations receive each `StreamChunk` produced by the per-job dispatcher task
//! (the GoF Mediator) and the terminal sentinel on subprocess exit. The dispatcher
//! iterates `Vec<Arc<dyn StreamChunkObserver>>` and calls `on_chunk` synchronously
//! per-observer. The `NoopStreamObserver` is the null object for headless/test contexts.
//!
//! Per ADR-0054, both methods are fire-and-forget: implementations MUST NOT block
//! the tokio executor and MUST be cancel-safe.

use async_trait::async_trait;

use crate::subprocess::state::SubprocessState;
use crate::subprocess::stream::StreamChunk;
use crate::value_objects::JobId;

/// Observer port for subprocess stream chunks and terminal sentinel events.
///
/// Implementations are registered with `SubprocessRegistry` at construction time.
/// The per-job dispatcher task calls `on_chunk` for every successfully drained
/// `StreamChunk` and `on_terminal` exactly once after the child enters a terminal
/// state and all pending chunks have been flushed.
#[async_trait]
pub trait StreamChunkObserver: Send + Sync + std::fmt::Debug {
    /// Receives one stream chunk produced by the per-job dispatcher.
    ///
    /// Called once per chunk drained from the bounded per-stream mpsc channel.
    /// MUST NOT block; the dispatcher loop awaits this call inline.
    async fn on_chunk(&self, chunk: &StreamChunk);

    /// Receives the terminal sentinel after the child has exited.
    ///
    /// Called exactly once per job after all pending chunks have been delivered
    /// via `on_chunk` and just before `subprocess.result` becomes callable.
    /// The `state` parameter is the terminal `SubprocessState` (`Succeeded`,
    /// `Failed`, `TimedOut`, or `Cancelled`).
    async fn on_terminal(&self, job_id: &JobId, state: SubprocessState);
}

/// A no-op `StreamChunkObserver` for use when no client push channel is available.
///
/// Used in tests, headless contexts, and as a placeholder when a client does not
/// supply a `progressToken` on `subprocess.spawn`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopStreamObserver;

#[async_trait]
impl StreamChunkObserver for NoopStreamObserver {
    async fn on_chunk(&self, _chunk: &StreamChunk) {
        // Intentional no-op.
    }

    async fn on_terminal(&self, _job_id: &JobId, _state: SubprocessState) {
        // Intentional no-op.
    }
}
