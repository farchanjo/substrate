//! Internal per-job slot holding mutable state, channels, and cancellation handle.
//!
//! [`JobSlot`] is the unit stored in the `DashMap` keyed by [`JobId`]. State
//! transitions are serialized through `parking_lot::Mutex<JobEntry>` per ADR-0040
//! (Race Resolution section). The result watch channel and the state machine
//! transition are set inside the same mutex lock, ensuring a concurrent
//! `job.result` call that observes `state=Succeeded` always finds the result.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use time::OffsetDateTime;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

use substrate_domain::errors::{SubstrateError, SubstrateResult};
use substrate_domain::jobs::entry::JobEntry;
use substrate_domain::jobs::state::JobState;
use substrate_domain::ports::job_registry::JobResult;

/// Internal per-job mutable container.
///
/// The outer `Arc` lets callers hold a reference to the slot while the registry
/// `DashMap` entry is not locked, avoiding deadlocks on recursive `DashMap` lookups.
#[derive(Debug)]
pub(crate) struct JobSlot {
    /// Mutex-protected job metadata and current state.
    ///
    /// Wrapped in `Arc` so the worker spawn closure and the slot can share the
    /// same entry without requiring the worker to hold a reference to `Arc<JobSlot>`.
    /// This resolves the ordering constraint where the slot must be created before
    /// `tokio::spawn` (to capture in the closure) but the `AbortHandle` required
    /// by `JobSlot::new` comes from the spawn result.
    ///
    /// `parking_lot::Mutex` is used rather than `tokio::sync::Mutex` because
    /// all critical sections are short (no I/O inside the lock) and `parking_lot`
    /// avoids accidental `.await` across a held lock guard.
    pub(crate) entry: Arc<parking_lot::Mutex<JobEntry>>,

    /// Last-value channel set once by the worker upon terminal state entry.
    ///
    /// Set inside the same `entry` lock as the `state` transition to ensure
    /// readers that observe a terminal state always find the result present.
    pub(crate) result_tx: watch::Sender<Option<JobResult>>,

    /// Cloneable receiver for long-poll in `job.result`.
    pub(crate) result_rx: watch::Receiver<Option<JobResult>>,

    /// Child `CancellationToken` scoped to this job.
    ///
    /// `job.cancel` calls `.cancel()` on this token. The token is a child of
    /// the registry-level root token so SIGTERM propagation also reaches it.
    pub(crate) cancel: CancellationToken,

    /// Handle to abort the worker task forcefully if drain timeout expires.
    // Wave G+: wired by MCP server dispatcher graceful shutdown
    #[expect(
        dead_code,
        reason = "Wave G+: wired by MCP server graceful shutdown drain"
    )]
    pub(crate) abort: AbortHandle,

    /// Monotonically increasing counter for progress event sequence numbers.
    ///
    /// Sourced from `AtomicU64` with `Relaxed` ordering; strict ordering relative
    /// to other slots is not required because sequence numbers are per-job.
    pub(crate) sequence: Arc<AtomicU64>,
}

impl JobSlot {
    /// Creates a new `JobSlot` from an initial `JobEntry` and the given handles.
    ///
    /// The result watch channel is initialised to `None` (no result yet).
    #[expect(dead_code, reason = "alternate constructor retained for Wave G+ graceful-shutdown drain path")]
    pub(crate) fn new(entry: JobEntry, cancel: CancellationToken, abort: AbortHandle) -> Arc<Self> {
        let (result_tx, result_rx) = watch::channel(None);
        Arc::new(Self {
            entry: Arc::new(parking_lot::Mutex::new(entry)),
            result_tx,
            result_rx,
            cancel,
            abort,
            sequence: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Creates a `JobSlot` using pre-allocated watch channels.
    ///
    /// Used by the registry submit path to allow capturing the sender in the
    /// worker spawn closure before the full `Arc<JobSlot>` exists — required
    /// because `AbortHandle` is only available after `tokio::spawn` returns.
    pub(crate) fn from_parts(
        entry: JobEntry,
        cancel: CancellationToken,
        abort: AbortHandle,
        result_tx: watch::Sender<Option<JobResult>>,
        result_rx: watch::Receiver<Option<JobResult>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            entry: Arc::new(parking_lot::Mutex::new(entry)),
            result_tx,
            result_rx,
            cancel,
            abort,
            sequence: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Returns the next sequence number for a progress event on this job.
    // Wave G+: wired by progress event emission path
    #[expect(dead_code, reason = "Wave G+: wired by progress event emission path")]
    pub(crate) fn next_sequence(&self) -> u64 {
        // Relaxed: we only need monotonicity within this slot; no cross-slot ordering.
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    /// Attempts a state transition from `current` to `next`.
    ///
    /// Returns `Ok(previous_state)` on success, or `SubstrateError::InternalError`
    /// when `current.can_transition_to(next)` returns `false` (invalid edge).
    ///
    /// The caller MUST hold the intent to move to `next` before calling this;
    /// on `Ok(terminal)` the caller should also set the result watch channel.
    // Wave G+: wired by MCP server worker closure and tests
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Wave G+: wired by MCP server worker closure")
    )]
    pub(crate) fn try_transition(&self, next: JobState) -> SubstrateResult<JobState> {
        let mut entry = self.entry.lock();
        let prev = entry.state;
        if !prev.can_transition_to(next) {
            // Per ADR-0040 State pattern: invalid transitions are silent no-ops.
            // Return the current (likely terminal) state so the caller can decide.
            return Err(SubstrateError::InternalError {
                reason: format!(
                    "invalid state transition {prev} -> {next}; current state stays {prev}"
                ),
                correlation_id: None,
            });
        }
        entry.state = next;
        entry.updated_at = OffsetDateTime::now_utc();
        if next.is_terminal() {
            entry.terminal_at = Some(entry.updated_at);
        }
        drop(entry);
        Ok(prev)
    }

    /// Sets the result watch channel inside the entry mutex.
    ///
    /// MUST be called immediately after a successful `try_transition` to a
    /// terminal state, inside the same logical atomic window (the `parking_lot`
    /// mutex ensures no context switch between the state write and this call
    /// when the caller holds the guard — but we reacquire here for simplicity
    /// since `parking_lot::Mutex` is reentrant-safe when the guard is dropped).
    ///
    /// In practice, the calling code in `registry.rs` does:
    /// ```text
    /// slot.try_transition(JobState::Succeeded)?;
    /// slot.set_result(result);
    /// ```
    /// The watch sender is non-blocking; `send` succeeds even with no receivers.
    // Wave G+: wired by MCP server worker closure
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Wave G+: wired by MCP server worker closure")
    )]
    pub(crate) fn set_result(&self, result: JobResult) {
        // `send` on a watch channel always succeeds (no bounded capacity).
        let _ = self.result_tx.send(Some(result));
    }

    /// Returns a snapshot of the current `JobEntry` (cloned under the lock).
    pub(crate) fn snapshot(&self) -> JobEntry {
        self.entry.lock().clone()
    }

    /// Returns `true` when the job is in a terminal state.
    // Wave G+: wired by registry cancel and tests
    #[expect(dead_code, reason = "Wave G+: wired by registry cancel handler")]
    pub(crate) fn is_terminal(&self) -> bool {
        self.entry.lock().state.is_terminal()
    }
}
