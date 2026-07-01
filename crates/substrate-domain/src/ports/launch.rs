//! `LaunchPort` — inbound port for the launch bounded context per ADR-0063.
//!
//! Implemented by the `substrate-launch` adapter crate (behind the `launch`
//! Cargo feature). The port abstracts the launch **registry/orchestrator**, not
//! the OS: the adapter consumes an injected `Arc<dyn SubprocessPort>` for every
//! managed Service so launch never spawns processes directly. The composition
//! root wires an `Arc<dyn LaunchPort>` when the feature is active.
//!
//! Cancellation: this port uses the same [`CancelSignal`] abstraction as
//! `SubprocessPort` and `FsIndexPort`, keeping `substrate-domain` free of
//! tokio-util.
//!
//! References: ADR-0063, ADR-0064, ADR-0065, ADR-0066, ADR-0068.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::errors::SubstrateResult;
use crate::launch::errors::LaunchError;
use crate::launch::event::LaunchEvent;
use crate::launch::profile::ServiceName;
use crate::launch::stack::StackHandle;
use crate::launch::state::{DisconnectPolicy, StackState};
use crate::launch::trust::TrustRecord;
use crate::value_objects::stack_id::StackId;

// Re-export CancelSignal so callers of this port do not need to import fs_index.
pub use crate::ports::fs_index::CancelSignal;

/// Inbound port for declarative process orchestration per ADR-0063.
///
/// Adapter implementations live in `substrate-launch` (gated behind the `launch`
/// Cargo feature). Domain code and MCP tool handlers depend only on this trait.
///
/// All `async fn` methods are cancel-safe at the `await` boundary per ADR-0037:
/// adapters MUST check the [`CancelSignal`] at each `await` point using
/// `tokio::select! biased` with the work arm first.
#[async_trait]
pub trait LaunchPort: Send + Sync {
    /// Scaffolds a new `.substrate.toml` Profile and returns the written path.
    ///
    /// When `profile_path` is `None`, a default path in the current project is
    /// used. `project_type_hint` (e.g. `"rust"`, `"node"`) biases the generated
    /// service template. Spawns no process.
    ///
    /// # Errors
    ///
    /// - [`LaunchError::ConfigUntrustedDir`] — target directory is insecure.
    /// - [`LaunchError::InvalidProfile`] — a Profile already exists or is malformed.
    async fn init(
        &self,
        profile_path: Option<&str>,
        project_type_hint: Option<&str>,
    ) -> Result<String, LaunchError>;

    /// Returns the Service catalog of a Profile without any trust verdict.
    ///
    /// Read-only: parses and DAG-validates the Profile but performs no trust
    /// check (per `launch-list-no-trust-required.feature`). Spawns no process.
    ///
    /// # Errors
    ///
    /// - [`LaunchError::ConfigSymlinkRejected`] — the path is a symlink.
    /// - [`LaunchError::CycleDetected`] — the dependency graph is not a DAG.
    /// - [`LaunchError::InvalidProfile`] — the Profile fails structural validation.
    async fn list(&self, profile_path: &str) -> Result<Vec<ServiceCatalogEntry>, LaunchError>;

    /// Blesses a Profile: captures its inode/content tuple into the trust store.
    ///
    /// Performs the safe-open, `fstat`, and content-hash steps, builds a
    /// [`TrustRecord`], and appends it to the trust store. Spawns no process
    /// (per `launch-trust-blesses-profile.feature`).
    ///
    /// # Errors
    ///
    /// - [`LaunchError::ConfigSymlinkRejected`] — the path is a symlink.
    /// - [`LaunchError::ConfigUntrustedDir`] — the parent directory is insecure.
    /// - [`LaunchError::TrustStoreInsecure`] — the trust store permissions are loose.
    async fn trust(&self, profile_path: &str) -> Result<TrustRecord, LaunchError>;

    /// Brings up a Stack: validates trust + DAG, then spawns Services in topo order.
    ///
    /// This is an async Task (job bucket E). Each Service is started only after
    /// its dependencies reach readiness. Returns the [`StackHandle`] (carrying the
    /// `stack_id`) once bring-up is initiated.
    ///
    /// # Errors
    ///
    /// - [`LaunchError::ProfileNotTrusted`] — the Profile is not blessed.
    /// - [`LaunchError::CycleDetected`] — the dependency graph is not a DAG.
    /// - [`LaunchError::DependencyFailed`] — a required dependency failed readiness.
    async fn up(
        &self,
        profile_path: &str,
        on_client_disconnect: Option<DisconnectPolicy>,
        orphan_ttl_secs: Option<u32>,
        cancel: &dyn CancelSignal,
    ) -> Result<StackHandle, LaunchError>;

    /// Returns the current handles for all Stacks, or one Stack when `stack_id` is set.
    ///
    /// Triggers the reaper-on-boot pass for detached Stacks (a no-op for
    /// in-session Stacks in the MVP).
    ///
    /// # Errors
    ///
    /// - [`crate::errors::SubstrateError::InvalidArgument`] — malformed `stack_id`.
    async fn status(&self, stack_id: Option<&StackId>) -> SubstrateResult<Vec<StackHandle>>;

    /// Returns the event-log delta for a Stack, cursor-addressed.
    ///
    /// When `service` is `Some`, only that Service's events are returned. `since`
    /// is the opaque cursor (ADR-0008) marking the last position already read.
    /// The returned `Option<String>` is the next cursor.
    ///
    /// # Errors
    ///
    /// - [`crate::errors::SubstrateError::JobNotFound`] — no Stack with that id.
    async fn logs(
        &self,
        stack_id: &StackId,
        service: Option<&str>,
        since: Option<&str>,
    ) -> SubstrateResult<(Vec<LaunchEvent>, Option<String>)>;

    /// Restarts one Service of a Stack via the subprocess port (orchestrated).
    ///
    /// The orchestrated restart MUST NOT count against the subprocess crash-loop
    /// budget. Returns the updated [`StackHandle`].
    ///
    /// # Errors
    ///
    /// - [`LaunchError::SupervisorUnreachable`] — the Stack's supervisor is gone.
    /// - [`LaunchError::DependencyFailed`] — the restarted Service failed readiness.
    async fn restart(
        &self,
        stack_id: &StackId,
        service_name: &str,
        cancel: &dyn CancelSignal,
    ) -> Result<StackHandle, LaunchError>;

    /// Reloads a Stack against a new Profile, reconciling the running graph.
    ///
    /// Diffs the new Profile against the running [`StackHandle`] services into a
    /// [`ReloadReport`] and applies the minimal restart closure.
    ///
    /// # Errors
    ///
    /// - [`LaunchError::ProfileNotTrusted`] — the new Profile is not blessed.
    /// - [`LaunchError::CycleDetected`] — the new dependency graph is not a DAG.
    async fn reload(
        &self,
        stack_id: &StackId,
        profile_path: Option<&str>,
        cancel: &dyn CancelSignal,
    ) -> Result<ReloadReport, LaunchError>;

    /// Tears down a Stack in reverse topological order and returns its final state.
    ///
    /// Each Service is cascade-stopped via the subprocess port. The Stack ends in
    /// [`StackState::Down`].
    ///
    /// # Errors
    ///
    /// - [`LaunchError::SupervisorUnreachable`] — the Stack's supervisor is gone.
    async fn down(
        &self,
        stack_id: &StackId,
        cancel: &dyn CancelSignal,
    ) -> Result<StackState, LaunchError>;

    /// Removes a terminal (`Down`) Stack's bookkeeping entry from the registry.
    ///
    /// Purely an in-memory/local housekeeping operation: no process is
    /// signalled (the Stack is already fully torn down by [`Self::down`]).
    /// Frees `launch.status`/`launch.logs` from listing Stacks the operator no
    /// longer cares about, without requiring an MCP server restart.
    ///
    /// # Errors
    ///
    /// - [`LaunchError::SupervisorUnreachable`] — no Stack with that id is known.
    /// - [`LaunchError::StackNotTerminal`] — the Stack's state is not `Down`.
    async fn forget(&self, stack_id: &StackId) -> Result<(), LaunchError>;
}

/// Convenience alias for a boxed trait object of [`LaunchPort`].
pub type DynLaunchPort = dyn LaunchPort;

// ---- Supporting types -------------------------------------------------------

/// One entry returned by [`LaunchPort::list`]: a Service's catalog view.
///
/// Read-only projection of a `LaunchService` without trust or runtime state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceCatalogEntry {
    /// The Service alias within the Profile.
    pub name: ServiceName,
    /// The executable plus arguments in argv form.
    pub command: Vec<String>,
    /// The Services this entry depends on.
    pub depends_on: Vec<ServiceName>,
    /// Whether this Service is required (a failed dependency blocks bring-up).
    pub required: bool,
}

/// The reconciliation summary returned by [`LaunchPort::reload`].
///
/// Classifies the diff between the running Stack and the reloaded Profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadReport {
    /// Services present in the new Profile but not the running Stack.
    pub added: Vec<ServiceName>,
    /// Services present in the running Stack but not the new Profile.
    pub removed: Vec<ServiceName>,
    /// Services whose spawn-affecting fields changed and were restarted.
    pub restarted: Vec<ServiceName>,
    /// Services whose only changes were dependency edges (no restart needed).
    pub edge_only: Vec<ServiceName>,
}
