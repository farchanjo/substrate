//! Stack aggregate and detached-supervisor registry value objects.
//!
//! Mirrors `#Stack`, `#StackChild`, and `#SupervisorRegistry` in
//! `docs/arch/schemas/launch.cue`. A [`StackHandle`] is the running instance of
//! a Profile: the per-Service handles, the pinned config hash, and the lifecycle
//! state. The per-Service state reuses `SubprocessState` from the subprocess BC.
//!
//! References: ADR-0063 §"`#Stack`", ADR-0068 §"detached supervisor".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::launch::profile::ServiceName;
use crate::launch::state::{DisconnectPolicy, StackState};
use crate::subprocess::state::SubprocessState;
use crate::value_objects::stack_id::StackId;

/// The running instance of a Profile (the launch aggregate root).
///
/// Mirrors `#Stack` in `launch.cue` (ADR-0063). The `supervisor` field is present
/// only for a detached Stack (`policy == Detach`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackHandle {
    /// Unique identifier of this Stack instance.
    pub stack_id: StackId,
    /// Absolute, canonical path of the `.substrate.toml` this Stack pins.
    pub profile_path: String,
    /// Content hash of the Profile pinned at `launch.up` time, `^(blake3|sha256):`.
    pub config_hash: String,
    /// The resolved disconnect policy in force for this Stack instance.
    pub policy: DisconnectPolicy,
    /// Current Stack lifecycle position.
    pub state: StackState,
    /// Each Service name mapped to its current per-process lifecycle state.
    pub services: BTreeMap<ServiceName, SubprocessState>,
    /// Durable supervisor registry entry; present only for a detached Stack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor: Option<SupervisorRegistry>,
}

/// One supervised child recorded in the durable registry.
///
/// Mirrors `#StackChild` in `launch.cue` (ADR-0068). The `pgid` is the
/// process-group leader id used for cascade reap of the whole subtree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackChild {
    /// The Service alias this child materializes.
    pub name: ServiceName,
    /// OS process id of the child (`>= 2`).
    pub pid: i32,
    /// Process-group id used for cascade reap (`>= 2`).
    pub pgid: i32,
    /// Child process start-time in seconds since the Unix epoch; compared on
    /// re-attach to detect pid recycling (ADR-0068).
    pub start_epoch: u64,
}

/// The durable per-Stack state-file written atomically under the user state dir.
///
/// Mirrors `#SupervisorRegistry` in `launch.cue` (ADR-0068). It is the rendezvous
/// a fresh MCP server uses to re-attach to, adopt, or reap a detached Stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorRegistry {
    /// OS pid of the detached `substrate --supervise` process (`>= 2`).
    pub supervisor_pid: i32,
    /// Supervisor start time in seconds since the Unix epoch; distinguishes a
    /// live supervisor from a stale registry entry after pid reuse.
    pub start_epoch: u64,
    /// The disconnect policy under which the Stack was detached.
    pub policy: DisconnectPolicy,
    /// Content hash pinning the Profile the supervisor is running.
    pub config_hash: String,
    /// The supervised processes owned by this supervisor.
    pub children: Vec<StackChild>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_handle_round_trips_json() {
        let mut services = BTreeMap::new();
        services.insert("web".to_owned(), SubprocessState::Ready);
        let handle = StackHandle {
            stack_id: StackId::now_v7(),
            profile_path: "/p/.substrate.toml".to_owned(),
            config_hash: "blake3:abc".to_owned(),
            policy: DisconnectPolicy::Shutdown,
            state: StackState::Running,
            services,
            supervisor: None,
        };
        #[expect(clippy::unwrap_used, reason = "test: in-memory value serializes")]
        let json = serde_json::to_string(&handle).unwrap();
        #[expect(clippy::unwrap_used, reason = "test: round-trip deserializes")]
        let back: StackHandle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stack_id, handle.stack_id);
        assert_eq!(back.state, StackState::Running);
        // The absent supervisor must be skipped on the wire.
        assert!(!json.contains("supervisor"));
    }
}
