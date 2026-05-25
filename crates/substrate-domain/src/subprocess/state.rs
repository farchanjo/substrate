//! `SubprocessState` — lifecycle state machine for a spawned child process.
//!
//! Mirrors `#SubprocessState` in `docs/arch/schemas/subprocess.cue`.
//! Terminal states (`Succeeded`, `Failed`, `Cancelled`, `Killed`, `TimedOut`)
//! never regress. Invalid transitions return an error (no panic) per the `GoF`
//! State pattern and ADR-0040 conventions.
//!
//! References: ADR-0052 §"`SubprocessHandle`", ADR-0053 §"`ChildHandle` lifecycle",
//! ADR-0056 §"Lifecycle Extension".

use serde::{Deserialize, Serialize};

/// All valid lifecycle states of a spawned child process per ADR-0052 and ADR-0056.
///
/// Serialized as `PascalCase` to match the CUE schema `#SubprocessState` values.
///
/// ## Lifecycle order (canonical)
///
/// `Pending` → `Starting` → `Running` → `Ready` → … → terminal
///
/// When `health_probe = None` and `restart_policy = Never`, the process transitions
/// through `Starting` atomically and the observable sequence remains
/// `Pending → Running → <terminal>` (backward-compatible per ADR-0056).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubprocessState {
    /// Subprocess has been registered but the OS child has not started yet.
    Pending,
    /// Child spawned; health probe not yet passed.
    ///
    /// Entered immediately on OS spawn when a `HealthProbe` is configured.
    /// Transitions atomically to `Ready` within the same scheduler tick when
    /// `health_probe = None`, preserving backward compatibility per ADR-0056.
    Starting,
    /// The OS child process is executing.
    Running,
    /// First successful health probe has passed; process is confirmed live.
    ///
    /// When `health_probe = None`, this state is a backward-compatible alias
    /// for `Running` and is entered atomically after `Starting` per ADR-0056.
    Ready,
    /// Transient state between child exit and supervisor re-spawn.
    ///
    /// Only reachable when `restart_policy != Never`. The supervisor task waits
    /// `backoff_ms` milliseconds in this state before forking a new child.
    Restarting,
    /// The child was cancelled by the client or graceful shutdown;
    /// SIGTERM was sufficient to stop the process.
    Cancelled,
    /// The child exceeded its configured `timeout_secs` limit;
    /// the cascade kill chain was triggered.
    TimedOut,
    /// The child did not respond to SIGTERM within `cascade_drain_secs`
    /// and was terminated with SIGKILL per ADR-0053.
    Killed,
    /// The child exited with a zero exit code.
    Succeeded,
    /// The child exited with a non-zero exit code.
    Failed,
}

impl SubprocessState {
    /// Returns `true` when no further state transitions are possible.
    ///
    /// Terminal states are `Succeeded`, `Failed`, `Cancelled`, `Killed`, and `TimedOut`.
    /// The new states `Starting`, `Ready`, and `Restarting` are non-terminal per ADR-0056.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Killed | Self::TimedOut
        )
    }

    /// Returns `true` when transitioning to `next` is a valid state-machine edge.
    ///
    /// Valid edges per ADR-0052 and ADR-0056:
    /// - `Pending` -> `Starting | Cancelled`
    /// - `Starting` -> `Running | Failed | Cancelled`
    /// - `Running` -> `Ready | Succeeded | Failed | Cancelled | Killed | TimedOut`
    /// - `Ready` -> `Restarting | Succeeded | Failed | Cancelled | Killed | TimedOut`
    /// - `Restarting` -> `Starting | Cancelled`
    /// - Terminal -> (nothing; all transitions return `false`)
    ///
    /// Callers that receive `false` MUST treat the attempted transition as a no-op.
    /// No panic is raised; this is intentional (State pattern per `GoF`).
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Starting | Self::Cancelled)
                | (Self::Starting, Self::Running | Self::Failed | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Ready
                        | Self::Succeeded
                        | Self::Failed
                        | Self::Cancelled
                        | Self::Killed
                        | Self::TimedOut
                )
                | (
                    Self::Ready,
                    Self::Restarting
                        | Self::Succeeded
                        | Self::Failed
                        | Self::Cancelled
                        | Self::Killed
                        | Self::TimedOut
                )
                | (Self::Restarting, Self::Starting | Self::Cancelled)
        )
    }
}

impl std::fmt::Display for SubprocessState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "Pending",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Ready => "Ready",
            Self::Restarting => "Restarting",
            Self::Cancelled => "Cancelled",
            Self::TimedOut => "TimedOut",
            Self::Killed => "Killed",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
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
        assert!(!SubprocessState::Starting.is_terminal());
        assert!(!SubprocessState::Running.is_terminal());
        assert!(!SubprocessState::Ready.is_terminal());
        assert!(!SubprocessState::Restarting.is_terminal());
    }

    #[test]
    fn valid_transitions_accepted() {
        // Pending edges.
        assert!(SubprocessState::Pending.can_transition_to(SubprocessState::Starting));
        assert!(SubprocessState::Pending.can_transition_to(SubprocessState::Cancelled));
        // Starting edges.
        assert!(SubprocessState::Starting.can_transition_to(SubprocessState::Running));
        assert!(SubprocessState::Starting.can_transition_to(SubprocessState::Failed));
        assert!(SubprocessState::Starting.can_transition_to(SubprocessState::Cancelled));
        // Running edges.
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::Ready));
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::Succeeded));
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::Failed));
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::Cancelled));
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::Killed));
        assert!(SubprocessState::Running.can_transition_to(SubprocessState::TimedOut));
        // Ready edges.
        assert!(SubprocessState::Ready.can_transition_to(SubprocessState::Restarting));
        assert!(SubprocessState::Ready.can_transition_to(SubprocessState::Succeeded));
        assert!(SubprocessState::Ready.can_transition_to(SubprocessState::Failed));
        assert!(SubprocessState::Ready.can_transition_to(SubprocessState::Cancelled));
        assert!(SubprocessState::Ready.can_transition_to(SubprocessState::Killed));
        assert!(SubprocessState::Ready.can_transition_to(SubprocessState::TimedOut));
        // Restarting edges.
        assert!(SubprocessState::Restarting.can_transition_to(SubprocessState::Starting));
        assert!(SubprocessState::Restarting.can_transition_to(SubprocessState::Cancelled));
    }

    #[test]
    fn terminal_regression_rejected() {
        assert!(!SubprocessState::Succeeded.can_transition_to(SubprocessState::Running));
        assert!(!SubprocessState::Killed.can_transition_to(SubprocessState::Pending));
        assert!(!SubprocessState::Cancelled.can_transition_to(SubprocessState::Running));
        assert!(!SubprocessState::Failed.can_transition_to(SubprocessState::Starting));
        assert!(!SubprocessState::TimedOut.can_transition_to(SubprocessState::Restarting));
    }

    #[test]
    fn pending_skips_to_terminal_rejected() {
        assert!(!SubprocessState::Pending.can_transition_to(SubprocessState::Succeeded));
        assert!(!SubprocessState::Pending.can_transition_to(SubprocessState::Failed));
        assert!(!SubprocessState::Pending.can_transition_to(SubprocessState::TimedOut));
        assert!(!SubprocessState::Pending.can_transition_to(SubprocessState::Killed));
        // Pending must go through Starting, not directly to Running.
        assert!(!SubprocessState::Pending.can_transition_to(SubprocessState::Running));
    }
}
