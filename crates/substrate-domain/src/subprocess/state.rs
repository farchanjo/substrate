//! `SubprocessState` — lifecycle state machine for a spawned child process.
//!
//! Mirrors `#SubprocessState` in `docs/arch/schemas/subprocess.cue`.
//! Terminal states (`Succeeded`, `Failed`, `Cancelled`, `Killed`, `TimedOut`)
//! never regress. Invalid transitions return an error (no panic) per the `GoF`
//! State pattern and ADR-0040 conventions.
//!
//! References: ADR-0052 §"`SubprocessHandle`", ADR-0053 §"`ChildHandle` lifecycle".

use serde::{Deserialize, Serialize};

/// All valid lifecycle states of a spawned child process per ADR-0052.
///
/// Serialized as `PascalCase` to match the CUE schema `#SubprocessState` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubprocessState {
    /// Subprocess has been registered but the OS child has not started yet.
    Pending,
    /// The OS child process is executing.
    Running,
    /// The child was cancelled by the client or graceful shutdown;
    /// SIGTERM was sufficient to stop the process.
    Cancelled,
    /// The child did not respond to SIGTERM within `cascade_drain_secs`
    /// and was terminated with SIGKILL per ADR-0053.
    Killed,
    /// The child exited with a zero exit code.
    Succeeded,
    /// The child exited with a non-zero exit code.
    Failed,
    /// The child exceeded its configured `timeout_secs` limit;
    /// the cascade kill chain was triggered.
    TimedOut,
}

impl SubprocessState {
    /// Returns `true` when no further state transitions are possible.
    ///
    /// Terminal states are `Succeeded`, `Failed`, `Cancelled`, `Killed`, and `TimedOut`.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Killed | Self::TimedOut
        )
    }

    /// Returns `true` when transitioning to `next` is a valid state-machine edge.
    ///
    /// Valid edges per ADR-0052:
    /// - `Pending` -> `Running`
    /// - `Pending` -> `Cancelled` (cancel received before OS spawn)
    /// - `Running` -> `Succeeded | Failed | Cancelled | Killed | TimedOut`
    /// - Terminal -> (nothing; all transitions return `false`)
    ///
    /// Callers that receive `false` MUST treat the attempted transition as a no-op.
    /// No panic is raised; this is intentional (State pattern per `GoF`).
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Succeeded
                        | Self::Failed
                        | Self::Cancelled
                        | Self::Killed
                        | Self::TimedOut
                )
        )
    }
}

impl std::fmt::Display for SubprocessState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Cancelled => "Cancelled",
            Self::Killed => "Killed",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
            Self::TimedOut => "TimedOut",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_are_terminal() {
        assert!(SubprocessState::Succeeded.is_terminal());
        assert!(SubprocessState::Failed.is_terminal());
        assert!(SubprocessState::Cancelled.is_terminal());
        assert!(SubprocessState::Killed.is_terminal());
        assert!(SubprocessState::TimedOut.is_terminal());
    }

    #[test]
    fn non_terminal_states_are_not_terminal() {
        assert!(!SubprocessState::Pending.is_terminal());
        assert!(!SubprocessState::Running.is_terminal());
    }

    #[test]
    fn valid_transitions_accepted() {
        assert!(SubprocessState::Pending.can_transition_to(SubprocessState::Running));
        assert!(SubprocessState::Pending.can_transition_to(SubprocessState::Cancelled));
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::Succeeded));
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::Failed));
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::Cancelled));
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::Killed));
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::TimedOut));
    }

    #[test]
    fn terminal_regression_rejected() {
        assert!(!SubprocessState::Succeeded.can_transition_to(SubprocessState::Running));
        assert!(!SubprocessState::Killed.can_transition_to(SubprocessState::Pending));
        assert!(!SubprocessState::Cancelled.can_transition_to(SubprocessState::Running));
    }

    #[test]
    fn pending_skips_to_terminal_rejected() {
        assert!(!SubprocessState::Pending.can_transition_to(SubprocessState::Succeeded));
        assert!(!SubprocessState::Pending.can_transition_to(SubprocessState::Failed));
        assert!(!SubprocessState::Pending.can_transition_to(SubprocessState::TimedOut));
        assert!(!SubprocessState::Pending.can_transition_to(SubprocessState::Killed));
    }
}
