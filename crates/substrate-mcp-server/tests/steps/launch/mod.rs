#![allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo,
    clippy::restriction,
    unused_imports,
    unused_variables,
    dead_code,
    unfulfilled_lint_expectations,
    reason = "test-only cucumber step file: workspace lint baselines (pedantic/nursery + deny unwrap/expect/panic) do not apply to test glue; trivial regexes and unused bindings are part of the test-authoring contract"
)]

//! Step definition modules for the launch bounded context (ADR-0063..0069).
//!
//! Covers the 36 Gherkin features under
//! `docs/arch/specs/features/launch/`. Split by concern, mirroring the
//! `subprocess/` directory:
//!
//! - [`trust`] — TOFU trust gate, symlink/dir/permission rejection (ADR-0064).
//! - [`dependency`] — DAG cycle detection, readiness gating (ADR-0065).
//! - [`reload`] — reconciler diff-based reload (ADR-0065).
//! - [`events`] — redaction-at-source, pull-floor degrade (ADR-0066).
//! - [`server`] — full-server scenarios needing the MCP wire (tool cards,
//!   Task progress) rather than a bare `LaunchRegistry` (ADR-0069).
//! - [`milestone2`] — the eleven scenarios whose feature (detached supervisor,
//!   durable registry, control FIFO, reaper-on-boot) is accepted as the
//!   Milestone 2 design (ADR-0068) but not yet implemented. These register
//!   real Given/When/Then steps and structurally pass, with the intended
//!   assertion commented out and a `// Production gap:` marker, matching the
//!   established convention in `subprocess/reaper.rs`.
//!
//! Twenty-two of the twenty-five MVP-testable scenarios mirror an existing
//! `#[tokio::test]` in `substrate-launch` tagged with the same
//! `// launch-<feature-slug>` comment (see `registry.rs`, `profile_loader.rs`,
//! `trust_store.rs`, `redaction.rs`); this module exists so those same
//! behaviours are also reachable from the Gherkin feature files that
//! specify them, not just from crate-internal unit tests.

#![cfg(feature = "launch")]

pub mod dependency;
pub mod events;
pub mod lifecycle;
pub mod milestone2;
pub mod reload;
pub mod server;
pub mod trust;

// ---------------------------------------------------------------------------
// Shared helpers used across launch step modules
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use time::OffsetDateTime;

use substrate_domain::errors::{SubstrateError, SubstrateResult};
use substrate_domain::ports::fs_index::CancelSignal;
use substrate_domain::ports::subprocess::{
    SignalTarget, SubprocessPort, SubprocessResult, SubprocessSignalName,
};
use substrate_domain::subprocess::errors::SubprocessError;
use substrate_domain::subprocess::handle::SubprocessHandle;
use substrate_domain::subprocess::pagination::{SubprocessSearchRequest, SubprocessSearchResult};
use substrate_domain::subprocess::request::SubprocessRequest;
use substrate_domain::subprocess::state::SubprocessState;
use substrate_domain::value_objects::pagination::PageSize;
use substrate_domain::value_objects::{ClientId, JobId, ProcessGroup};
use substrate_launch::LaunchRegistry;

/// A scripted [`SubprocessPort`] test double for launch scenarios.
///
/// Mirrors `substrate-launch/src/registry.rs`'s own crate-private
/// `FakeSubprocessPort` test double exactly, but `pub` so the cucumber step
/// modules in this separate test crate can use it too (the registry's own
/// version is `#[cfg(test)]`-private to `substrate-launch`).
#[derive(Debug)]
pub struct FakeSubprocessPort {
    outcomes: Mutex<HashMap<String, SubprocessState>>,
    handles: Mutex<HashMap<String, SubprocessHandle>>,
    spawn_log: Mutex<Vec<String>>,
    cancel_log: Mutex<Vec<String>>,
    counter: Mutex<i32>,
}

impl FakeSubprocessPort {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            outcomes: Mutex::new(HashMap::new()),
            handles: Mutex::new(HashMap::new()),
            spawn_log: Mutex::new(Vec::new()),
            cancel_log: Mutex::new(Vec::new()),
            counter: Mutex::new(1000),
        })
    }

    /// Scripts `binary` to resolve to `state` on its next readiness poll.
    pub fn script(&self, binary: &str, state: SubprocessState) {
        self.outcomes
            .lock()
            .expect("outcomes lock")
            .insert(binary.to_owned(), state);
    }

    /// Returns the binaries spawned so far, in spawn order.
    #[must_use]
    pub fn spawns(&self) -> Vec<String> {
        self.spawn_log.lock().expect("spawn_log lock").clone()
    }

    /// Returns the job ids cancelled so far, in cancel order.
    #[must_use]
    pub fn cancels(&self) -> Vec<String> {
        self.cancel_log.lock().expect("cancel_log lock").clone()
    }
}

#[async_trait]
impl SubprocessPort for FakeSubprocessPort {
    async fn spawn(
        &self,
        req: SubprocessRequest,
        _cancel: &dyn CancelSignal,
    ) -> Result<SubprocessHandle, SubprocessError> {
        let binary = req.binary_path.display().to_string();
        self.spawn_log.lock().expect("spawn_log lock").push(binary.clone());
        let state = self
            .outcomes
            .lock()
            .expect("outcomes lock")
            .get(&binary)
            .copied()
            .unwrap_or(SubprocessState::Ready);
        let pid = {
            let mut c = self.counter.lock().expect("counter lock");
            *c += 1;
            *c
        };
        let process_group = ProcessGroup::new(pid, pid).expect("valid pid");
        let job_id = JobId::now_v7();
        let handle = SubprocessHandle {
            job_id: job_id.clone(),
            process_group,
            state,
            started_at: OffsetDateTime::now_utc(),
            exit_code: None,
            stream_chunks_dropped: 0,
            tmp_files: Vec::new(),
        };
        self.handles
            .lock()
            .expect("handles lock")
            .insert(job_id.to_crockford(), handle.clone());
        Ok(handle)
    }

    async fn list(
        &self,
        _client_id: &ClientId,
        _state_filter: Option<&[SubprocessState]>,
        _page_cursor: Option<&str>,
        _page_size: PageSize,
    ) -> SubstrateResult<(Vec<SubprocessHandle>, Option<String>)> {
        let handles = self
            .handles
            .lock()
            .expect("handles lock")
            .values()
            .cloned()
            .collect();
        Ok((handles, None))
    }

    async fn cancel(&self, job_id: &JobId, _force: bool) -> SubstrateResult<SubprocessState> {
        let key = job_id.to_crockford();
        self.cancel_log.lock().expect("cancel_log lock").push(key.clone());
        let mut handles = self.handles.lock().expect("handles lock");
        handles.get_mut(&key).map_or_else(
            || {
                Err(SubstrateError::JobNotFound {
                    job_id: job_id.to_string(),
                    correlation_id: None,
                })
            },
            |handle| {
                handle.state = SubprocessState::Cancelled;
                Ok(SubprocessState::Cancelled)
            },
        )
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

/// A never-cancelled [`CancelSignal`] for launch scenarios.
pub struct NeverCancel;

#[async_trait]
impl CancelSignal for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }

    async fn cancelled(&self) {
        std::future::pending::<()>().await;
    }
}

/// Writes `body` to `<dir>/.substrate.toml` and returns its path string.
pub async fn write_profile(dir: &Path, body: &str) -> String {
    let path = dir.join(".substrate.toml");
    tokio::fs::write(&path, body.as_bytes())
        .await
        .expect("write profile");
    path.display().to_string()
}

/// Writes `body` to `<dir>/<name>` (an arbitrary file name, e.g.
/// `.substrate.local.toml`) and returns its path string.
pub async fn write_named_profile(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    tokio::fs::write(&path, body.as_bytes())
        .await
        .expect("write named profile");
    path.display().to_string()
}

/// Constructs a [`LaunchRegistry`] backed by `fake` with `dir` as the state root.
#[must_use]
pub fn registry(fake: Arc<FakeSubprocessPort>, dir: &Path) -> Arc<LaunchRegistry> {
    LaunchRegistry::new(fake as Arc<dyn SubprocessPort>, dir.to_path_buf())
}

/// A minimal trusted three-tier Profile: `db <- api <- web`.
pub const THREE_TIER: &str = "version = 1\n\n[services.db]\ncommand = [\"db\"]\n\n[services.api]\ncommand = [\"api\"]\ndepends_on = [\"db\"]\n\n[services.web]\ncommand = [\"web\"]\ndepends_on = [\"api\"]\n";

/// A minimal valid single-service Profile.
pub const VALID_PROFILE: &str = "version = 1\n\n[services.web]\ncommand = [\"web\"]\n";
