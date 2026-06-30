//! `StackState` — lifecycle state machine for a whole launch Stack.
//!
//! Mirrors `#StackState` in `docs/arch/schemas/launch.cue`. This is distinct
//! from the per-Service `SubprocessState`: a Stack aggregates many child
//! processes and has its own lifecycle. `Draining` and `Down` are terminal and
//! never regress. Invalid transitions return `false` (no panic) per the `GoF`
//! State pattern and ADR-0063 conventions.
//!
//! References: ADR-0063 §"`#Stack`", ADR-0068 §"`#DisconnectPolicy`".

use serde::{Deserialize, Serialize};

/// Lifecycle position of a whole Stack, distinct from per-Service `SubprocessState`.
///
/// Serialized as `PascalCase` to match the CUE schema `#StackState` values.
///
/// ## Lifecycle order (canonical)
///
/// `Pending` → `Starting` → `Running` → (`Degraded` | `Detached`) → `Draining` → `Down`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StackState {
    /// Stack registered; bring-up has not started yet.
    Pending,
    /// Services are being spawned in topological order.
    Starting,
    /// All required Services reached `Ready`; the Stack is healthy.
    Running,
    /// One or more optional (non-required) Services failed; the Stack runs degraded.
    Degraded,
    /// The client disconnected under `policy = detach`; a detached supervisor owns it.
    Detached,
    /// Teardown in progress: Services are being cascade-stopped in reverse order.
    Draining,
    /// Terminal: every Service is stopped and the Stack instance is gone.
    Down,
}

impl StackState {
    /// Returns `true` when no further state transitions are possible.
    ///
    /// Terminal states are `Draining` and `Down`; once a Stack enters teardown it
    /// never regresses to an earlier lifecycle position.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Draining | Self::Down)
    }

    /// Returns `true` when transitioning to `next` is a valid state-machine edge.
    ///
    /// Valid edges per ADR-0063:
    /// - `Pending` -> `Starting | Draining`
    /// - `Starting` -> `Running | Degraded | Draining`
    /// - `Running` -> `Degraded | Detached | Draining`
    /// - `Degraded` -> `Running | Detached | Draining`
    /// - `Detached` -> `Running | Degraded | Draining`
    /// - `Draining` -> `Down`
    /// - `Down` -> (nothing)
    ///
    /// Callers that receive `false` MUST treat the attempted transition as a no-op.
    /// No panic is raised; this is intentional (State pattern per `GoF`).
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Pending, Self::Starting | Self::Draining)
                | (
                    Self::Starting | Self::Detached,
                    Self::Running | Self::Degraded | Self::Draining
                )
                | (Self::Running, Self::Degraded | Self::Detached | Self::Draining)
                | (Self::Degraded, Self::Running | Self::Detached | Self::Draining)
                | (Self::Draining, Self::Down)
        )
    }
}

impl std::fmt::Display for StackState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "Pending",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Degraded => "Degraded",
            Self::Detached => "Detached",
            Self::Draining => "Draining",
            Self::Down => "Down",
        };
        f.write_str(s)
    }
}

/// Disconnect policy: what happens to a Stack when the MCP client disconnects.
///
/// Mirrors `#DisconnectPolicy` in `launch.cue` (ADR-0068). `Shutdown` (the
/// default) drains and kills the Stack — zero surviving processes. `Detach`
/// keeps it alive under a detached supervisor, re-attachable later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DisconnectPolicy {
    /// Drain and kill the Stack on client disconnect. Default.
    #[default]
    Shutdown,
    /// Keep the Stack alive under a detached supervisor for later re-attach.
    Detach,
}

impl std::fmt::Display for DisconnectPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shutdown => f.write_str("shutdown"),
            Self::Detach => f.write_str("detach"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states_are_terminal() {
        assert!(StackState::Draining.is_terminal());
        assert!(StackState::Down.is_terminal());
    }

    #[test]
    fn non_terminal_states_are_not_terminal() {
        assert!(!StackState::Pending.is_terminal());
        assert!(!StackState::Starting.is_terminal());
        assert!(!StackState::Running.is_terminal());
        assert!(!StackState::Degraded.is_terminal());
        assert!(!StackState::Detached.is_terminal());
    }

    #[test]
    fn valid_transitions_accepted() {
        assert!(StackState::Pending.can_transition_to(StackState::Starting));
        assert!(StackState::Starting.can_transition_to(StackState::Running));
        assert!(StackState::Starting.can_transition_to(StackState::Degraded));
        assert!(StackState::Running.can_transition_to(StackState::Degraded));
        assert!(StackState::Running.can_transition_to(StackState::Detached));
        assert!(StackState::Running.can_transition_to(StackState::Draining));
        assert!(StackState::Degraded.can_transition_to(StackState::Running));
        assert!(StackState::Detached.can_transition_to(StackState::Running));
        assert!(StackState::Draining.can_transition_to(StackState::Down));
    }

    #[test]
    fn terminal_states_never_regress() {
        // Draining only proceeds to Down.
        assert!(!StackState::Draining.can_transition_to(StackState::Running));
        assert!(!StackState::Draining.can_transition_to(StackState::Pending));
        // Down is fully terminal.
        assert!(!StackState::Down.can_transition_to(StackState::Running));
        assert!(!StackState::Down.can_transition_to(StackState::Draining));
        assert!(!StackState::Down.can_transition_to(StackState::Pending));
    }

    #[test]
    fn disconnect_policy_defaults_to_shutdown() {
        assert_eq!(DisconnectPolicy::default(), DisconnectPolicy::Shutdown);
    }
}
