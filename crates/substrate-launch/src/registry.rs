//! `LaunchRegistry` — concrete [`LaunchPort`] adapter for the launch BC (ADR-0063..0069).
//!
//! Manages an in-memory [`DashMap`] of [`StackHandle`] entries keyed by
//! [`StackId`], and delegates all process spawning to an injected
//! [`SubprocessPort`]. This crate NEVER calls `tokio::process::Command`
//! directly; every process operation routes through the subprocess port
//! (ADR-0063 §"in-process MVP").
//!
//! Phase 2 ships a shell: [`LaunchPort`] is fully implemented with
//! "not yet implemented" sentinels in every method except [`LaunchRegistry::status`]
//! which works immediately (it simply reads the in-memory `stacks` map). Phase 4
//! replaces the stubs with real orchestration logic.
//!
//! References: ADR-0063, ADR-0065, ADR-0068.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;

use substrate_domain::errors::{SubstrateError, SubstrateResult};
use substrate_domain::launch::errors::LaunchError;
use substrate_domain::launch::event::LaunchEvent;
use substrate_domain::launch::stack::StackHandle;
use substrate_domain::launch::state::{DisconnectPolicy, StackState};
use substrate_domain::launch::trust::TrustRecord;
use substrate_domain::ports::fs_index::CancelSignal;
use substrate_domain::ports::launch::{LaunchPort, ReloadReport, ServiceCatalogEntry};
use substrate_domain::ports::subprocess::SubprocessPort;
use substrate_domain::value_objects::stack_id::StackId;

/// Concrete [`LaunchPort`] adapter managing in-session Stacks.
///
/// Constructed by the composition root and shared via `Arc<LaunchRegistry>`.
/// The injected [`SubprocessPort`] is the exclusive path for all process
/// management; this registry never touches `tokio::process::Command`.
///
/// The `stacks` [`DashMap`] is the in-memory registry for the MVP
/// (in-session Stacks only). Phase 4 adds durable state writes under
/// `state_root` for detached supervisor support (Milestone 2).
pub struct LaunchRegistry {
    /// Live Stack handles keyed by [`StackId`].
    stacks: Arc<DashMap<StackId, StackHandle>>,
    /// Injected subprocess adapter for all process operations.
    subprocess: Arc<dyn SubprocessPort>,
    /// Root directory for durable per-Stack state files (Phase 4+).
    state_root: PathBuf,
}

impl LaunchRegistry {
    /// Constructs a new `LaunchRegistry`.
    ///
    /// The `subprocess` port is injected by the composition root so this
    /// crate never depends on `substrate-subprocess` concretely (hexagonal
    /// layering, ADR-0022).
    ///
    /// # Parameters
    ///
    /// - `subprocess`: the subprocess adapter that handles all process
    ///   spawning and termination on behalf of this registry.
    /// - `state_root`: absolute directory for durable Stack state files.
    ///   Must exist and be owned by the running user. Used starting Phase 4.
    #[must_use]
    pub fn new(subprocess: Arc<dyn SubprocessPort>, state_root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            stacks: Arc::default(),
            subprocess,
            state_root,
        })
    }

    /// Returns the number of in-memory Stack entries (including terminal ones).
    #[must_use]
    pub fn stack_count(&self) -> usize {
        self.stacks.len()
    }

    /// Returns a reference to the injected subprocess port.
    ///
    /// Exposed for Phase 4 orchestration helpers that need to call spawn/cancel
    /// on behalf of a Stack's services.
    #[must_use]
    pub fn subprocess_port(&self) -> &Arc<dyn SubprocessPort> {
        &self.subprocess
    }

    /// Returns the root directory for durable per-Stack state files.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }
}

impl std::fmt::Debug for LaunchRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchRegistry")
            .field("stack_count", &self.stacks.len())
            .field("state_root", &self.state_root)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl LaunchPort for LaunchRegistry {
    /// Scaffolds a `.substrate.toml` profile.
    ///
    /// Phase 4 implementation; returns `InvalidProfile` until then.
    ///
    /// # Errors
    ///
    /// Always returns [`LaunchError::InvalidProfile`] in Phase 2.
    async fn init(
        &self,
        _profile_path: Option<&str>,
        _project_type_hint: Option<&str>,
    ) -> Result<String, LaunchError> {
        Err(LaunchError::InvalidProfile {
            msg: "launch.init not yet implemented (Phase 4)".to_owned(),
        })
    }

    /// Returns the Service catalog of a profile without a trust gate.
    ///
    /// Phase 4 implementation; returns `InvalidProfile` until then.
    ///
    /// # Errors
    ///
    /// Always returns [`LaunchError::InvalidProfile`] in Phase 2.
    async fn list(&self, _profile_path: &str) -> Result<Vec<ServiceCatalogEntry>, LaunchError> {
        Err(LaunchError::InvalidProfile {
            msg: "launch.list not yet implemented (Phase 4)".to_owned(),
        })
    }

    /// Blesses a profile into the TOFU trust store.
    ///
    /// Phase 4 implementation; returns `InvalidProfile` until then.
    ///
    /// # Errors
    ///
    /// Always returns [`LaunchError::InvalidProfile`] in Phase 2.
    async fn trust(&self, _profile_path: &str) -> Result<TrustRecord, LaunchError> {
        Err(LaunchError::InvalidProfile {
            msg: "launch.trust not yet implemented (Phase 4)".to_owned(),
        })
    }

    /// Brings up a Stack: validates trust + DAG, spawns Services in topo order.
    ///
    /// Phase 4 implementation; returns `InvalidProfile` until then.
    ///
    /// # Errors
    ///
    /// Always returns [`LaunchError::InvalidProfile`] in Phase 2.
    async fn up(
        &self,
        _profile_path: &str,
        _on_client_disconnect: Option<DisconnectPolicy>,
        _orphan_ttl_secs: Option<u32>,
        _cancel: &dyn CancelSignal,
    ) -> Result<StackHandle, LaunchError> {
        Err(LaunchError::InvalidProfile {
            msg: "launch.up not yet implemented (Phase 4)".to_owned(),
        })
    }

    /// Returns the current handles for all Stacks, or one Stack when `stack_id` is set.
    ///
    /// This method is live from Phase 2: it reads the in-memory `stacks` map
    /// directly, returning `Ok(vec![])` when no Stacks have been brought up.
    ///
    /// # Errors
    ///
    /// Returns [`SubstrateError::JobNotFound`] when a specific `stack_id` is
    /// provided but no matching Stack exists (future Phase 4 behaviour; in Phase 2
    /// the map is always empty so `Some(id)` returns `Ok(vec![])`).
    async fn status(&self, stack_id: Option<&StackId>) -> SubstrateResult<Vec<StackHandle>> {
        let handles: Vec<StackHandle> = stack_id.map_or_else(
            || self.stacks.iter().map(|e| e.value().clone()).collect(),
            |id| {
                self.stacks
                    .get(id)
                    .map(|e| e.value().clone())
                    .into_iter()
                    .collect()
            },
        );
        Ok(handles)
    }

    /// Returns the event-log delta for a Stack, cursor-addressed.
    ///
    /// Phase 4 implementation; returns `InternalError` until then.
    ///
    /// # Errors
    ///
    /// Always returns [`SubstrateError::InternalError`] in Phase 2.
    async fn logs(
        &self,
        _stack_id: &StackId,
        _service: Option<&str>,
        _since: Option<&str>,
    ) -> SubstrateResult<(Vec<LaunchEvent>, Option<String>)> {
        Err(SubstrateError::InternalError {
            reason: "launch.logs not yet implemented (Phase 4)".to_owned(),
            correlation_id: None,
        })
    }

    /// Restarts one Service of a Stack via the subprocess port.
    ///
    /// Phase 4 implementation; returns `InvalidProfile` until then.
    ///
    /// # Errors
    ///
    /// Always returns [`LaunchError::InvalidProfile`] in Phase 2.
    async fn restart(
        &self,
        _stack_id: &StackId,
        _service_name: &str,
        _cancel: &dyn CancelSignal,
    ) -> Result<StackHandle, LaunchError> {
        Err(LaunchError::InvalidProfile {
            msg: "launch.restart not yet implemented (Phase 4)".to_owned(),
        })
    }

    /// Reloads a Stack against a new profile, reconciling the running graph.
    ///
    /// Phase 4 implementation; returns `InvalidProfile` until then.
    ///
    /// # Errors
    ///
    /// Always returns [`LaunchError::InvalidProfile`] in Phase 2.
    async fn reload(
        &self,
        _stack_id: &StackId,
        _profile_path: Option<&str>,
        _cancel: &dyn CancelSignal,
    ) -> Result<ReloadReport, LaunchError> {
        Err(LaunchError::InvalidProfile {
            msg: "launch.reload not yet implemented (Phase 4)".to_owned(),
        })
    }

    /// Tears down a Stack in reverse topological order.
    ///
    /// Phase 4 implementation; returns `InvalidProfile` until then.
    ///
    /// # Errors
    ///
    /// Always returns [`LaunchError::InvalidProfile`] in Phase 2.
    async fn down(
        &self,
        _stack_id: &StackId,
        _cancel: &dyn CancelSignal,
    ) -> Result<StackState, LaunchError> {
        Err(LaunchError::InvalidProfile {
            msg: "launch.down not yet implemented (Phase 4)".to_owned(),
        })
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use substrate_domain::errors::SubstrateResult;
    use substrate_domain::ports::subprocess::{
        SignalTarget, SubprocessPort, SubprocessResult, SubprocessSignalName,
    };
    use substrate_domain::subprocess::errors::SubprocessError;
    use substrate_domain::subprocess::handle::SubprocessHandle;
    use substrate_domain::subprocess::pagination::{
        SubprocessSearchRequest, SubprocessSearchResult,
    };
    use substrate_domain::subprocess::request::SubprocessRequest;
    use substrate_domain::subprocess::state::SubprocessState;
    use substrate_domain::value_objects::pagination::PageSize;
    use substrate_domain::value_objects::{ClientId, JobId};

    /// Null-Object [`SubprocessPort`] test double.
    ///
    /// Returns errors from every method; sufficient for tests that only exercise
    /// the registry's in-memory map (e.g. [`LaunchRegistry::status`]).
    struct NoopSubprocessPort;

    #[async_trait]
    impl SubprocessPort for NoopSubprocessPort {
        async fn spawn(
            &self,
            _req: SubprocessRequest,
            _cancel: &dyn CancelSignal,
        ) -> Result<SubprocessHandle, SubprocessError> {
            Err(SubprocessError::BinaryNotAllowed {
                path: "noop".to_owned(),
            })
        }

        async fn list(
            &self,
            _client_id: &ClientId,
            _state_filter: Option<&[SubprocessState]>,
            _page_cursor: Option<&str>,
            _page_size: PageSize,
        ) -> SubstrateResult<(Vec<SubprocessHandle>, Option<String>)> {
            Ok((Vec::new(), None))
        }

        async fn cancel(
            &self,
            job_id: &JobId,
            _force: bool,
        ) -> SubstrateResult<SubprocessState> {
            Err(SubstrateError::JobNotFound {
                job_id: job_id.to_string(),
                correlation_id: None,
            })
        }

        async fn result(
            &self,
            job_id: &JobId,
            _wait_ms: u32,
            _include_aggregates: bool,
        ) -> SubstrateResult<SubprocessResult> {
            Err(SubstrateError::JobNotFound {
                job_id: job_id.to_string(),
                correlation_id: None,
            })
        }

        async fn signal(
            &self,
            job_id: &JobId,
            _signal_name: SubprocessSignalName,
            _target: SignalTarget,
        ) -> SubstrateResult<()> {
            Err(SubstrateError::JobNotFound {
                job_id: job_id.to_string(),
                correlation_id: None,
            })
        }

        async fn search(
            &self,
            req: SubprocessSearchRequest,
        ) -> Result<SubprocessSearchResult, SubprocessError> {
            Err(SubprocessError::InvalidRequest {
                msg: format!("noop: job {} not found", req.job_id),
            })
        }
    }

    /// Helper: returns an `Arc<dyn SubprocessPort>` backed by the noop test double.
    fn noop_subprocess_port() -> Arc<dyn SubprocessPort> {
        Arc::new(NoopSubprocessPort)
    }

    /// Phase 2 smoke test: a freshly constructed registry returns `Ok(vec![])`.
    ///
    /// Asserts `launch-status` read-path on an empty registry.
    #[tokio::test]
    async fn status_returns_empty_on_new_registry() {
        let state_root = std::env::temp_dir().join("substrate-launch-test-phase2");
        let registry = LaunchRegistry::new(noop_subprocess_port(), state_root);
        let result = registry.status(None).await;
        assert!(result.is_ok(), "status(None) should return Ok; got {result:?}");
        #[expect(
            clippy::unwrap_used,
            reason = "test: asserted is_ok() on the line above"
        )]
        let handles = result.unwrap();
        assert!(
            handles.is_empty(),
            "new registry must have no stacks; got {} entries",
            handles.len()
        );
    }

    /// Phase 2 smoke test: `status(Some(unknown_id))` also returns `Ok(vec![])`.
    #[tokio::test]
    async fn status_with_unknown_id_returns_empty() {
        let state_root = std::env::temp_dir().join("substrate-launch-test-phase2-id");
        let registry = LaunchRegistry::new(noop_subprocess_port(), state_root);
        let id = StackId::now_v7();
        let result = registry.status(Some(&id)).await;
        assert!(result.is_ok(), "status(Some(unknown)) should return Ok; got {result:?}");
        #[expect(
            clippy::unwrap_used,
            reason = "test: asserted is_ok() on the line above"
        )]
        let handles = result.unwrap();
        assert!(handles.is_empty(), "unknown stack_id must return empty vec");
    }

    /// Phase 2: every not-implemented stub returns `InvalidProfile`.
    #[tokio::test]
    async fn not_implemented_stubs_return_invalid_profile() {
        let state_root = std::env::temp_dir().join("substrate-launch-test-phase2-stubs");
        let registry = LaunchRegistry::new(noop_subprocess_port(), state_root);

        assert!(
            matches!(
                registry.init(None, None).await,
                Err(LaunchError::InvalidProfile { .. })
            ),
            "init stub must return InvalidProfile"
        );
        assert!(
            matches!(
                registry.list("x.toml").await,
                Err(LaunchError::InvalidProfile { .. })
            ),
            "list stub must return InvalidProfile"
        );
        assert!(
            matches!(
                registry.trust("x.toml").await,
                Err(LaunchError::InvalidProfile { .. })
            ),
            "trust stub must return InvalidProfile"
        );
    }
}
