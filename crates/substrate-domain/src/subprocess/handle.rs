//! `SubprocessHandle` — aggregate root for an active or completed child process.
//!
//! Mirrors `#SubprocessHandle` in `docs/arch/schemas/subprocess.cue`.
//! Stored in the `JobRegistry` under the corresponding `JobId`. The handle is
//! the authoritative record for a single spawn invocation and is updated on
//! every state transition by the `substrate-subprocess` adapter.
//!
//! References: ADR-0052 §"`SubprocessHandle`", ADR-0053 §"`ChildHandle` lifecycle".

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::subprocess::errors::SubprocessError;
use crate::subprocess::state::SubprocessState;
use crate::value_objects::{JobId, ProcessGroup};

/// Aggregate root representing an active or completed child process.
///
/// Stored in the `JobRegistry` keyed by `job_id`. The adapter crate
/// (`substrate-subprocess`) is responsible for creating and updating the handle
/// on each state transition. Domain code treats a received `SubprocessHandle`
/// as a read-only snapshot.
///
/// See `docs/arch/schemas/subprocess.cue #SubprocessHandle` and ADR-0052.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubprocessHandle {
    /// Canonical job identifier correlating this handle with the async `JobEntry`,
    /// the MCP `progressToken`, and the `correlation_id` in audit events.
    ///
    /// Per ADR-0040 triple-equality invariant: `job_id == progressToken == correlation_id`.
    pub job_id: JobId,

    /// OS process group descriptor holding the `pid` and `pgid` assigned by
    /// `setsid()` in the pre-exec hook per ADR-0053.
    pub process_group: ProcessGroup,

    /// Current lifecycle position in the subprocess state machine.
    pub state: SubprocessState,

    /// Wall-clock timestamp when the child transitioned to `Running`.
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,

    /// OS process exit status.
    ///
    /// Present only when `state` is `Succeeded` or `Failed`. `None` when the
    /// process was killed via `SIGKILL` or when `state` is still non-terminal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,

    /// Count of stdout/stderr stream chunks discarded due to bounded mpsc channel
    /// backpressure per ADR-0054.
    ///
    /// A non-zero value is surfaced in the job result hints map under the
    /// `subprocess_stream_chunks_dropped` key.
    pub stream_chunks_dropped: u64,

    /// Absolute paths of temporary files registered during this invocation.
    ///
    /// Populated when `capture_kind = TmpFile` or when transactional write
    /// intermediates are created. Cleaned up on cancel, kill, timeout, and
    /// normal exit per ADR-0033 and ADR-0053.
    pub tmp_files: Vec<PathBuf>,
}

impl SubprocessHandle {
    /// Returns `true` when the handle is in a terminal state.
    #[must_use]
    pub const fn terminal_state(&self) -> bool {
        self.state.is_terminal()
    }

    /// Attempts to transition the handle to a terminal state and record the
    /// exit code.
    ///
    /// `next_state` must be one of the terminal variants (`Succeeded`, `Failed`,
    /// `Cancelled`, `Killed`, `TimedOut`). Non-terminal targets are rejected.
    ///
    /// # Errors
    ///
    /// Returns `SubprocessError::InvalidStateTransition` when:
    /// - `next_state` is not a valid target from the current `self.state`.
    /// - `next_state` is not a terminal variant (callers must not call this to
    ///   transition to `Running`; use a direct field assignment for that).
    pub const fn mark_terminal(
        &mut self,
        next_state: SubprocessState,
        exit_code: Option<i32>,
    ) -> Result<(), SubprocessError> {
        if !next_state.is_terminal() {
            return Err(SubprocessError::InvalidStateTransition {
                from: self.state,
                to: next_state,
            });
        }
        if !self.state.can_transition_to(next_state) {
            return Err(SubprocessError::InvalidStateTransition {
                from: self.state,
                to: next_state,
            });
        }
        self.state = next_state;
        self.exit_code = exit_code;
        Ok(())
    }
}
