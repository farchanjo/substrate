//! `JobState` — the async job state machine per ADR-0040.
//!
//! Terminal states (`Succeeded`, `Failed`, `Cancelled`, `TimedOut`) never regress.
//! Invalid transitions are silently ignored (no panic) per the `State` `GoF` pattern.

use serde::{Deserialize, Serialize};

/// All valid states of the async job state machine per ADR-0040.
///
/// Serialised as `snake_case` to match the CUE schema `#JobState` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Job has been submitted and is waiting for a worker to pick it up.
    Pending,
    /// A worker has claimed the job and is executing it.
    Running,
    /// The job completed successfully; its result is available via `job.result`.
    Succeeded,
    /// The job terminated with an error.
    Failed,
    /// The job was cancelled by the client or during graceful shutdown.
    Cancelled,
    /// The job exceeded its configured `jobs.timeout.<tool>_secs` limit.
    TimedOut,
}

impl JobState {
    /// Returns `true` when no further state transitions are possible.
    ///
    /// Terminal states are `Succeeded`, `Failed`, `Cancelled`, and `TimedOut`.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }

    /// Returns `true` when transitioning to `next` is a valid state-machine edge.
    ///
    /// Valid edges per ADR-0040:
    /// - `Pending` → `Running`
    /// - `Running` → `Succeeded | Failed | Cancelled | TimedOut`
    /// - Terminal → (nothing; all transitions return `false`)
    ///
    /// Callers that receive `false` MUST treat the attempted transition as a
    /// no-op. No panic is raised; this is intentional (`State` pattern).
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Running)
                | (
                    Self::Running,
                    Self::Succeeded | Self::Failed | Self::Cancelled | Self::TimedOut
                )
        )
    }
}

impl std::fmt::Display for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_are_terminal() {
        assert!(JobState::Succeeded.is_terminal());
        assert!(JobState::Failed.is_terminal());
        assert!(JobState::Cancelled.is_terminal());
        assert!(JobState::TimedOut.is_terminal());
    }

    #[test]
    fn non_terminal_states_are_not_terminal() {
        assert!(!JobState::Pending.is_terminal());
        assert!(!JobState::Running.is_terminal());
    }

    #[test]
    fn valid_transitions_accepted() {
        assert!(JobState::Pending.can_transition_to(JobState::Running));
        assert!(JobState::Running.can_transition_to(JobState::Succeeded));
        assert!(JobState::Running.can_transition_to(JobState::Failed));
        assert!(JobState::Running.can_transition_to(JobState::Cancelled));
        assert!(JobState::Running.can_transition_to(JobState::TimedOut));
    }

    #[test]
    fn terminal_regression_rejected() {
        assert!(!JobState::Succeeded.can_transition_to(JobState::Running));
        assert!(!JobState::Failed.can_transition_to(JobState::Pending));
        assert!(!JobState::Cancelled.can_transition_to(JobState::Running));
        assert!(!JobState::TimedOut.can_transition_to(JobState::Succeeded));
    }

    #[test]
    fn pending_to_terminal_without_running_rejected() {
        assert!(!JobState::Pending.can_transition_to(JobState::Succeeded));
        assert!(!JobState::Pending.can_transition_to(JobState::Cancelled));
    }
}
