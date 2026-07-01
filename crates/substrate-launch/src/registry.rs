//! `LaunchRegistry` — concrete [`LaunchPort`] adapter for the launch BC (ADR-0063..0069).
//!
//! Manages an in-memory [`DashMap`] of per-Stack entries keyed by [`StackId`] and
//! delegates **all** process spawning to an injected [`SubprocessPort`]. This crate
//! NEVER calls `tokio::process::Command`; every process operation routes through
//! the subprocess port (ADR-0063 §"in-process MVP"), so no `no_subprocess.rego`
//! exception is required.
//!
//! # MVP scope (Phase 4)
//!
//! - `init` / `list` / `trust` — scaffold, read-only catalog, TOFU bless.
//! - `up` — TOFU gate, cycle pre-spawn reject, readiness-gated topological start,
//!   required-dependency failure -> [`LaunchError::DependencyFailed`], optional
//!   (`required = false`) dependency failure -> degraded (not an error).
//! - `down` — reverse-topological cascade stop through the subprocess port.
//! - `restart` — orchestrated single-Service restart (a fresh spawn, NOT counted
//!   against the subprocess crash-loop budget).
//! - `reload` — diff into added / removed / restarted (with cascade) / edge-only.
//! - `status` / `logs` — in-memory handle snapshot and bounded event tail.
//!
//! # Deferred to Milestone 2 (stubbed)
//!
//! `on_client_disconnect = detach` returns [`LaunchError::SupervisorUnreachable`];
//! the MVP only supports in-session `shutdown` semantics. The detached supervisor,
//! control FIFO, reaper-on-boot, orphan adopt/reap, and TTL paths are Milestone 2.
//!
//! References: ADR-0063, ADR-0065, ADR-0066, ADR-0068.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use substrate_domain::errors::{SubstrateError, SubstrateResult};
use substrate_domain::launch::errors::LaunchError;
use substrate_domain::launch::event::{LaunchEvent, LaunchEventKind};
use substrate_domain::launch::profile::{
    LaunchOperatorConfig, LaunchProfile, LaunchService, ServiceName,
};
use substrate_domain::launch::stack::{StackHandle, SupervisorRegistry};
use substrate_domain::launch::state::{DisconnectPolicy, StackState};
use substrate_domain::launch::trust::TrustRecord;
use substrate_domain::ports::fs_index::CancelSignal;
use substrate_domain::ports::launch::{LaunchPort, ReloadReport, ServiceCatalogEntry};
use substrate_domain::ports::subprocess::SubprocessPort;
use substrate_domain::subprocess::state::SubprocessState;
#[cfg(test)]
use substrate_domain::subprocess::stream::Stream;
use substrate_domain::value_objects::stack_id::StackId;
use substrate_domain::value_objects::{ClientId, JobId};

use crate::dag::{restart_closure, reverse_topo};
use crate::profile_loader::{build_trust_record, load_trusted, load_untrusted, write_scaffold};
#[cfg(test)]
use crate::redaction::Redactor;
use crate::supervisor::{
    ServiceOutcome, build_request, edges_only_differ, launch_client_id, outcome_state,
    spawn_fields_differ, spawn_service, stop_service, wait_ready,
};
use crate::supervisor_registry::{open_stack_registry, read_supervisor_registry, run_blocking};
use crate::trust_store::append_bless;

/// File name of the user-scope TOFU trust store under the state root.
const TRUST_STORE_FILE: &str = "launch-trust.toml";

// ---- Per-Stack registry entry ----------------------------------------------

/// In-memory bookkeeping for one running Stack.
///
/// Holds the public [`StackHandle`], the pinned [`LaunchProfile`] (needed for
/// `down`/`reload` topology), the per-Service [`JobId`] map (for cascade stop and
/// restart), and the bounded lifecycle event ring read by `logs`.
struct StackEntry {
    /// The public, cloneable snapshot returned by `status`.
    handle: StackHandle,
    /// The Profile pinned at `up` time (or last `reload`).
    profile: LaunchProfile,
    /// Each Service's current subprocess job, for cascade stop and restart.
    job_ids: BTreeMap<ServiceName, JobId>,
    /// Bounded lifecycle/semantic event log, tailed by `logs`.
    events: Vec<LaunchEvent>,
    /// Monotonic event sequence counter.
    next_seq: u64,
}

impl StackEntry {
    /// Builds a fresh entry with every Service marked `Pending` and state `Pending`.
    fn new(
        stack_id: StackId,
        profile_path: String,
        config_hash: String,
        policy: DisconnectPolicy,
        profile: LaunchProfile,
    ) -> Self {
        let services = profile
            .services
            .keys()
            .map(|name| (name.clone(), SubprocessState::Pending))
            .collect();
        let handle = StackHandle {
            stack_id,
            profile_path,
            config_hash,
            policy,
            state: StackState::Pending,
            services,
            supervisor: None,
        };
        Self {
            handle,
            profile,
            job_ids: BTreeMap::new(),
            events: Vec::new(),
            next_seq: 0,
        }
    }

    /// Records the current per-process lifecycle state for `name`.
    fn set_service(&mut self, name: &str, state: SubprocessState) {
        self.handle.services.insert(name.to_owned(), state);
    }

    /// Appends a lifecycle event with a fresh sequence number and cursor.
    fn emit(&mut self, kind: LaunchEventKind, service: Option<&str>, message: String) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.events.push(LaunchEvent {
            stack_id: self.handle.stack_id.clone(),
            service: service.map(ToOwned::to_owned),
            kind,
            seq,
            cursor: seq.to_string(),
            stream: None,
            message,
            exit_code: None,
            timestamp: now_rfc3339(),
        });
    }

    /// Appends a redacted `Semantic` event distilled from a child output line.
    ///
    /// The line is masked through a [`Redactor`] built from `redact` BEFORE it is
    /// stored, so a secret never reaches the event log in the clear (ADR-0066
    /// redaction-at-source).
    ///
    /// MVP note: the child-output capture pipe that feeds this seam lands in a
    /// later milestone, so the method is currently exercised only by the
    /// redaction-at-source test. It is `#[cfg(test)]`-gated until that wiring lands.
    #[cfg(test)]
    fn emit_semantic(&mut self, service: &str, raw_line: &str, redact: &[String]) {
        let message = Redactor::new(&[], redact).redact_line(raw_line);
        let seq = self.next_seq;
        self.next_seq += 1;
        self.events.push(LaunchEvent {
            stack_id: self.handle.stack_id.clone(),
            service: Some(service.to_owned()),
            kind: LaunchEventKind::Semantic,
            seq,
            cursor: seq.to_string(),
            stream: Some(Stream::Stdout),
            message,
            exit_code: None,
            timestamp: now_rfc3339(),
        });
    }
}

// ---- Registry ---------------------------------------------------------------

/// Concrete [`LaunchPort`] adapter managing in-session Stacks.
///
/// Constructed by the composition root and shared via `Arc<LaunchRegistry>`. The
/// injected [`SubprocessPort`] is the exclusive path for all process management.
pub struct LaunchRegistry {
    /// Live Stack entries keyed by [`StackId`].
    stacks: Arc<DashMap<StackId, StackEntry>>,
    /// Injected subprocess adapter for all process operations.
    subprocess: Arc<dyn SubprocessPort>,
    /// Root directory for durable per-Stack state files (Milestone 2) and the
    /// default Service working directory.
    state_root: PathBuf,
    /// Path of the user-scope TOFU trust store (`<state_root>/launch-trust.toml`).
    trust_store: PathBuf,
    /// Operator auto-bless policy (empty by default; never repo-controlled).
    op_config: LaunchOperatorConfig,
    /// Spawns the detached `substrate --supervise` process and awaits its durable
    /// registry on the `on_client_disconnect = detach` path (ADR-0068). The
    /// production launcher forks the current executable; tests inject a double.
    detach_launcher: Arc<dyn DetachLauncher>,
}

impl LaunchRegistry {
    /// Constructs a new `LaunchRegistry`.
    ///
    /// The `subprocess` port is injected by the composition root so this crate
    /// never depends on `substrate-subprocess` concretely (hexagonal layering,
    /// ADR-0022). The trust store is `<state_root>/launch-trust.toml`.
    ///
    /// # Parameters
    ///
    /// - `subprocess`: the subprocess adapter that handles all process spawning
    ///   and termination on behalf of this registry.
    /// - `state_root`: absolute directory for durable Stack state files and the
    ///   default Service working directory. Must exist and be owned by the user.
    #[must_use]
    pub fn new(subprocess: Arc<dyn SubprocessPort>, state_root: PathBuf) -> Arc<Self> {
        Self::with_launcher(subprocess, state_root, Arc::new(ProcessDetachLauncher))
    }

    /// Constructs a `LaunchRegistry` with an explicit [`DetachLauncher`].
    ///
    /// [`new`](Self::new) wires the production [`ProcessDetachLauncher`]; this seam
    /// lets the unit tests inject a double that never forks an OS process.
    fn with_launcher(
        subprocess: Arc<dyn SubprocessPort>,
        state_root: PathBuf,
        detach_launcher: Arc<dyn DetachLauncher>,
    ) -> Arc<Self> {
        let trust_store = state_root.join(TRUST_STORE_FILE);
        Arc::new(Self {
            stacks: Arc::default(),
            subprocess,
            state_root,
            trust_store,
            op_config: LaunchOperatorConfig::default(),
            detach_launcher,
        })
    }

    /// Returns the number of in-memory Stack entries (including terminal ones).
    #[must_use]
    pub fn stack_count(&self) -> usize {
        self.stacks.len()
    }

    /// Returns the root directory for durable per-Stack state files.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Spawns one Service and gates on its readiness, recording lifecycle events.
    async fn start_one(
        &self,
        entry: &mut StackEntry,
        name: &str,
        service: &LaunchService,
        client_id: &ClientId,
        cancel: &dyn CancelSignal,
    ) -> Result<ServiceOutcome, LaunchError> {
        let request = build_request(name, service, &self.state_root)?;
        entry.set_service(name, SubprocessState::Starting);
        entry.emit(
            LaunchEventKind::Started,
            Some(name),
            format!("starting service '{name}'"),
        );
        let handle = spawn_service(self.subprocess.as_ref(), request, cancel).await?;
        entry.job_ids.insert(name.to_owned(), handle.job_id.clone());
        let outcome = wait_ready(
            self.subprocess.as_ref(),
            client_id,
            &handle.job_id,
            cancel,
            service.health_probe.as_ref(),
        )
        .await?;
        entry.set_service(name, outcome_state(outcome));
        match outcome {
            ServiceOutcome::Ready => entry.emit(
                LaunchEventKind::Ready,
                Some(name),
                format!("service '{name}' is ready"),
            ),
            ServiceOutcome::Failed => {
                // Never became ready within its probe budget (or crashed): stop the
                // child so a service stuck in Starting is not left running/leaked.
                stop_service(self.subprocess.as_ref(), &handle.job_id).await;
                entry.emit(
                    LaunchEventKind::Crashed,
                    Some(name),
                    format!("service '{name}' failed readiness"),
                );
            },
        }
        Ok(outcome)
    }

    /// Spawns every Service in `set`, in `profile` topological order, gating each
    /// on readiness. Returns the new `(name, job_id, outcome)` triples.
    async fn spawn_set(
        &self,
        profile: &LaunchProfile,
        set: &BTreeSet<ServiceName>,
        client_id: &ClientId,
        cancel: &dyn CancelSignal,
    ) -> Result<Vec<(ServiceName, JobId, ServiceOutcome)>, LaunchError> {
        let order = profile.topological_order()?;
        let mut spawned = Vec::new();
        for name in order {
            if !set.contains(&name) {
                continue;
            }
            let Some(service) = profile.services.get(&name) else {
                continue;
            };
            let request = build_request(&name, service, &self.state_root)?;
            let handle = spawn_service(self.subprocess.as_ref(), request, cancel).await?;
            let outcome = wait_ready(
                self.subprocess.as_ref(),
                client_id,
                &handle.job_id,
                cancel,
                service.health_probe.as_ref(),
            )
            .await?;
            spawned.push((name, handle.job_id, outcome));
        }
        Ok(spawned)
    }

    /// Brings up a Stack under a detached supervisor (ADR-0068, Milestone 2).
    ///
    /// Delegates the spawn to the injected [`DetachLauncher`], which forks
    /// `substrate --supervise` and blocks until that process publishes a
    /// `supervisor.json` with a matching `config_hash`. The managed children then
    /// live in the detached supervisor — not this registry — so the recorded entry
    /// tracks no in-process job ids; it exists for `status`/`logs` reporting and is
    /// marked [`StackState::Detached`] with the supervisor snapshot attached.
    ///
    /// # Errors
    ///
    /// Returns [`LaunchError::SupervisorUnreachable`] when the supervisor process
    /// does not publish a matching registry within the readiness budget.
    async fn up_detached(
        &self,
        profile_path: &str,
        profile: LaunchProfile,
        config_hash: String,
        policy: DisconnectPolicy,
    ) -> Result<StackHandle, LaunchError> {
        let stack_id = StackId::now_v7();
        let canonical = canonical_path_string(profile_path);
        let registry = self
            .detach_launcher
            .launch_detached(&stack_id, Path::new(&canonical), &config_hash)
            .await?;
        let mut entry =
            StackEntry::new(stack_id.clone(), canonical, config_hash, policy, profile);
        entry.handle.state = StackState::Detached;
        for child in &registry.children {
            entry.set_service(&child.name, SubprocessState::Running);
        }
        entry.emit(
            LaunchEventKind::Started,
            None,
            format!("detached supervisor pid {} owns the stack", registry.supervisor_pid),
        );
        entry.handle.supervisor = Some(registry);
        let handle = entry.handle.clone();
        self.stacks.insert(stack_id, entry);
        Ok(handle)
    }
}

// ---- Detached-supervisor launcher (ADR-0068) --------------------------------

/// Seam for spawning the detached `substrate --supervise` process and awaiting its
/// durable registry, so [`LaunchRegistry::up`]'s detach path is unit-testable
/// without forking a real OS process.
#[async_trait]
trait DetachLauncher: Send + Sync {
    /// Spawns the detached supervisor for `stack_id` running `profile_path`, then
    /// blocks until its `supervisor.json` is published carrying `expected_hash`.
    ///
    /// # Errors
    ///
    /// Returns [`LaunchError::SupervisorUnreachable`] when the supervisor does not
    /// publish a matching registry within the readiness budget, or
    /// [`LaunchError::SpawnFailed`] / [`LaunchError::RegistryInsecure`] when the
    /// process or its registry directory cannot be created.
    async fn launch_detached(
        &self,
        stack_id: &StackId,
        profile_path: &Path,
        expected_hash: &str,
    ) -> Result<SupervisorRegistry, LaunchError>;
}

/// Upper bound on the wait for the detached supervisor to publish its registry.
const SUPERVISOR_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Cadence of the readiness poll for `supervisor.json`.
const SUPERVISOR_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Production [`DetachLauncher`]: forks `std::env::current_exe() --supervise …`
/// detached from this server's STDIO, then polls the durable registry directory
/// until the supervisor publishes a matching `supervisor.json`.
struct ProcessDetachLauncher;

#[async_trait]
impl DetachLauncher for ProcessDetachLauncher {
    async fn launch_detached(
        &self,
        stack_id: &StackId,
        profile_path: &Path,
        expected_hash: &str,
    ) -> Result<SupervisorRegistry, LaunchError> {
        let stack_dir = open_stack_registry(stack_id).await?;
        spawn_supervisor_process(stack_id, profile_path).await?;
        await_supervisor_ready(&stack_dir, stack_id, expected_hash).await
    }
}

/// Forks the detached supervisor process on the blocking pool (zone B, ADR-0003).
///
/// The child inherits none of this server's STDIO — the JSON-RPC channel on
/// `stdout` is sacred (ADR-0005) — and re-establishes its own session via `setsid`
/// (ADR-0068), so the spawning side detaches the descriptors and never waits on
/// the child (a dropped handle is reparented to init on this server's exit).
///
/// This forks the substrate binary itself in `--supervise` mode (ADR-0068 "same
/// binary"), NOT a managed Service — every managed Service still routes through
/// the injected `SubprocessPort`, so the no-subprocess policy (ADR-0044) is not
/// violated for the Stack's processes.
#[expect(
    clippy::disallowed_types,
    reason = "forks the substrate binary itself as its own detached supervisor \
              sidecar (ADR-0068 \"same binary\"), not a managed Service."
)]
#[expect(
    clippy::disallowed_methods,
    reason = "forks the substrate binary itself as its own detached supervisor \
              sidecar (ADR-0068 \"same binary\"), not a managed Service."
)]
async fn spawn_supervisor_process(
    stack_id: &StackId,
    profile_path: &Path,
) -> Result<(), LaunchError> {
    let exe = std::env::current_exe().map_err(|e| LaunchError::SpawnFailed { source: e })?;
    let stack_arg = stack_id.to_crockford();
    let profile_arg = profile_path.to_path_buf();
    run_blocking(move || {
        std::process::Command::new(exe)
            .arg("--supervise")
            .arg(&stack_arg)
            .arg("--profile")
            .arg(&profile_arg)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(drop)
            .map_err(|e| LaunchError::SpawnFailed { source: e })
    })
    .await
}

/// Polls `<stack_dir>/supervisor.json` until it is published with `expected_hash`
/// or [`SUPERVISOR_READY_TIMEOUT`] elapses (then [`LaunchError::SupervisorUnreachable`]).
async fn await_supervisor_ready(
    stack_dir: &Path,
    stack_id: &StackId,
    expected_hash: &str,
) -> Result<SupervisorRegistry, LaunchError> {
    let deadline = Instant::now() + SUPERVISOR_READY_TIMEOUT;
    loop {
        if let Ok(registry) = read_supervisor_registry(stack_dir).await
            && registry.config_hash == expected_hash
        {
            return Ok(registry);
        }
        if Instant::now() >= deadline {
            return Err(LaunchError::SupervisorUnreachable {
                stack_id: stack_id.to_crockford(),
            });
        }
        tokio::time::sleep(SUPERVISOR_POLL_INTERVAL).await;
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
    async fn init(
        &self,
        profile_path: Option<&str>,
        project_type_hint: Option<&str>,
    ) -> Result<String, LaunchError> {
        let path = profile_path.map_or_else(default_profile_path, PathBuf::from);
        let hint = project_type_hint
            .map(ToOwned::to_owned)
            .or_else(|| detect_project_type(&path));
        let written = write_scaffold(&path, hint.as_deref()).await?;
        Ok(written.display().to_string())
    }

    async fn list(&self, profile_path: &str) -> Result<Vec<ServiceCatalogEntry>, LaunchError> {
        let loaded = load_untrusted(Path::new(profile_path)).await?;
        loaded.profile.validate()?;
        // DAG-validate without a trust verdict (launch-list-no-trust-required).
        let _order = loaded.profile.topological_order()?;
        let mut entries = Vec::with_capacity(loaded.profile.services.len());
        for (name, service) in &loaded.profile.services {
            entries.push(ServiceCatalogEntry {
                name: name.clone(),
                command: service.command.argv()?.to_vec(),
                depends_on: service.depends_on.clone(),
                required: service.required,
            });
        }
        Ok(entries)
    }

    async fn trust(&self, profile_path: &str) -> Result<TrustRecord, LaunchError> {
        // Safe-open + fstat + content hash, then append a bless record. No spawn
        // (launch-trust-blesses-profile).
        let loaded = load_untrusted(Path::new(profile_path)).await?;
        let canonical = std::fs::canonicalize(profile_path).map_err(|_| {
            LaunchError::ConfigUntrustedDir {
                path: profile_path.to_owned(),
            }
        })?;
        let record = build_trust_record(
            &canonical.display().to_string(),
            loaded.identity,
            &loaded.config_hash,
        );
        append_bless(&self.trust_store, record.clone()).await?;
        Ok(record)
    }

    async fn up(
        &self,
        profile_path: &str,
        on_client_disconnect: Option<DisconnectPolicy>,
        _orphan_ttl_secs: Option<u32>,
        cancel: &dyn CancelSignal,
    ) -> Result<StackHandle, LaunchError> {
        let loaded = load_trusted(Path::new(profile_path), &self.trust_store, &self.op_config).await?;
        loaded.profile.validate()?;
        let policy = on_client_disconnect.unwrap_or(loaded.profile.on_client_disconnect);
        if policy == DisconnectPolicy::Detach {
            // Hand the Stack to a detached supervisor process (ADR-0068, Milestone
            // 2) instead of bringing it up in-session; it survives this server.
            return self
                .up_detached(profile_path, loaded.profile, loaded.config_hash, policy)
                .await;
        }
        // Cycle is rejected BEFORE any process is spawned (launch-depends-on-cycle-rejected).
        let topo = loaded.profile.topological_order()?;
        let client_id = launch_client_id()?;
        let stack_id = StackId::now_v7();
        let mut entry = StackEntry::new(
            stack_id.clone(),
            canonical_path_string(profile_path),
            loaded.config_hash,
            policy,
            loaded.profile.clone(),
        );
        entry.handle.state = StackState::Starting;

        let mut ready_status: BTreeMap<ServiceName, ServiceOutcome> = BTreeMap::new();
        let mut degraded = false;
        for name in &topo {
            let service = entry.profile.services.get(name).cloned().ok_or_else(|| {
                LaunchError::InvalidProfile {
                    msg: format!("service '{name}' missing from profile"),
                }
            })?;
            if let Some(dep) = blocked_dependency(&entry.profile, &service, &ready_status) {
                return Err(LaunchError::DependencyFailed {
                    service: name.clone(),
                    dependency: dep,
                });
            }
            let outcome = self
                .start_one(&mut entry, name, &service, &client_id, cancel)
                .await?;
            ready_status.insert(name.clone(), outcome);
            if outcome == ServiceOutcome::Failed {
                degraded = true;
            }
        }

        entry.handle.state = if degraded {
            StackState::Degraded
        } else {
            StackState::Running
        };
        let handle = entry.handle.clone();
        self.stacks.insert(stack_id, entry);
        Ok(handle)
    }

    async fn status(&self, stack_id: Option<&StackId>) -> SubstrateResult<Vec<StackHandle>> {
        let handles: Vec<StackHandle> = stack_id.map_or_else(
            || self.stacks.iter().map(|e| e.value().handle.clone()).collect(),
            |id| {
                self.stacks
                    .get(id)
                    .map(|e| e.value().handle.clone())
                    .into_iter()
                    .collect()
            },
        );
        Ok(handles)
    }

    async fn logs(
        &self,
        stack_id: &StackId,
        service: Option<&str>,
        since: Option<&str>,
    ) -> SubstrateResult<(Vec<LaunchEvent>, Option<String>)> {
        let floor = since.and_then(|s| s.parse::<u64>().ok());
        let events: Vec<LaunchEvent> = {
            let entry = self
                .stacks
                .get(stack_id)
                .ok_or_else(|| SubstrateError::JobNotFound {
                    job_id: stack_id.to_crockford(),
                    correlation_id: None,
                })?;
            entry
                .events
                .iter()
                .filter(|e| floor.is_none_or(|f| e.seq > f))
                .filter(|e| service.is_none_or(|s| e.service.as_deref() == Some(s)))
                .cloned()
                .collect()
        };
        let next = events.last().map(|e| e.seq.to_string());
        Ok((events, next))
    }

    async fn restart(
        &self,
        stack_id: &StackId,
        service_name: &str,
        cancel: &dyn CancelSignal,
    ) -> Result<StackHandle, LaunchError> {
        let (service, old_job, client_id) = {
            let entry = self.stacks.get(stack_id).ok_or_else(|| {
                LaunchError::SupervisorUnreachable {
                    stack_id: stack_id.to_crockford(),
                }
            })?;
            let service = entry.profile.services.get(service_name).cloned().ok_or_else(|| {
                LaunchError::InvalidProfile {
                    msg: format!("unknown service '{service_name}'"),
                }
            })?;
            (service, entry.job_ids.get(service_name).cloned(), launch_client_id()?)
        };
        if let Some(job) = &old_job {
            stop_service(self.subprocess.as_ref(), job).await;
        }
        // An orchestrated restart is a fresh spawn; it is NOT counted against the
        // subprocess crash-loop budget, which governs restart_policy auto-respawns only.
        let request = build_request(service_name, &service, &self.state_root)?;
        let handle = spawn_service(self.subprocess.as_ref(), request, cancel).await?;
        let outcome = wait_ready(
            self.subprocess.as_ref(),
            &client_id,
            &handle.job_id,
            cancel,
            service.health_probe.as_ref(),
        )
        .await?;

        let mut entry = self.stacks.get_mut(stack_id).ok_or_else(|| {
            LaunchError::SupervisorUnreachable {
                stack_id: stack_id.to_crockford(),
            }
        })?;
        entry.job_ids.insert(service_name.to_owned(), handle.job_id);
        entry.emit(
            LaunchEventKind::Restarting,
            Some(service_name),
            format!("restarting service '{service_name}'"),
        );
        entry.set_service(service_name, outcome_state(outcome));
        Ok(entry.handle.clone())
    }

    async fn reload(
        &self,
        stack_id: &StackId,
        profile_path: Option<&str>,
        cancel: &dyn CancelSignal,
    ) -> Result<ReloadReport, LaunchError> {
        let (old_profile, stored_path, job_ids) = {
            let entry = self.stacks.get(stack_id).ok_or_else(|| {
                LaunchError::SupervisorUnreachable {
                    stack_id: stack_id.to_crockford(),
                }
            })?;
            (
                entry.profile.clone(),
                entry.handle.profile_path.clone(),
                entry.job_ids.clone(),
            )
        };
        let path = profile_path.map_or(stored_path, ToOwned::to_owned);
        let loaded = load_trusted(Path::new(&path), &self.trust_store, &self.op_config).await?;
        loaded.profile.validate()?;
        let new_profile = loaded.profile;
        // Cycle in the reloaded graph is rejected before any restart.
        let _order = new_profile.topological_order()?;

        let report = compute_reload_report(&old_profile, &new_profile);

        // Stop removed + the restart closure, dependents-first (reverse topo).
        let stop_set: BTreeSet<ServiceName> = report
            .removed
            .iter()
            .chain(report.restarted.iter())
            .cloned()
            .collect();
        for name in reverse_order_for(&old_profile, &stop_set) {
            if let Some(job) = job_ids.get(&name) {
                stop_service(self.subprocess.as_ref(), job).await;
            }
        }

        // Start added + the restart closure, dependency-first (forward topo).
        let start_set: BTreeSet<ServiceName> = report
            .added
            .iter()
            .chain(report.restarted.iter())
            .cloned()
            .collect();
        let client_id = launch_client_id()?;
        let spawned = self
            .spawn_set(&new_profile, &start_set, &client_id, cancel)
            .await?;

        self.apply_reload(stack_id, new_profile, &report, spawned);
        Ok(report)
    }

    async fn down(
        &self,
        stack_id: &StackId,
        _cancel: &dyn CancelSignal,
    ) -> Result<StackState, LaunchError> {
        let (profile, job_ids) = {
            let entry = self.stacks.get(stack_id).ok_or_else(|| {
                LaunchError::SupervisorUnreachable {
                    stack_id: stack_id.to_crockford(),
                }
            })?;
            (entry.profile.clone(), entry.job_ids.clone())
        };
        let order = reverse_topo(&profile)?;
        for name in &order {
            if let Some(job) = job_ids.get(name) {
                stop_service(self.subprocess.as_ref(), job).await;
            }
        }
        if let Some(mut entry) = self.stacks.get_mut(stack_id) {
            entry.handle.state = StackState::Draining;
            for name in &order {
                entry.set_service(name, SubprocessState::Cancelled);
            }
            entry.emit(
                LaunchEventKind::Exited,
                None,
                "stack drained and brought down".to_owned(),
            );
            entry.handle.state = StackState::Down;
        }
        Ok(StackState::Down)
    }

    async fn forget(&self, stack_id: &StackId) -> Result<(), LaunchError> {
        let state = {
            let entry = self.stacks.get(stack_id).ok_or_else(|| {
                LaunchError::SupervisorUnreachable {
                    stack_id: stack_id.to_crockford(),
                }
            })?;
            entry.handle.state
        };
        if state != StackState::Down {
            return Err(LaunchError::StackNotTerminal {
                stack_id: stack_id.to_crockford(),
                state: state.to_string(),
            });
        }
        self.stacks.remove(stack_id);
        Ok(())
    }
}

impl LaunchRegistry {
    /// Applies a reload's stop/start results to the in-memory entry.
    ///
    /// Removes dropped Services, records the new job ids and readiness states for
    /// restarted/added Services, swaps in the new Profile, and emits a reload event.
    fn apply_reload(
        &self,
        stack_id: &StackId,
        new_profile: LaunchProfile,
        report: &ReloadReport,
        spawned: Vec<(ServiceName, JobId, ServiceOutcome)>,
    ) {
        let Some(mut entry) = self.stacks.get_mut(stack_id) else {
            return;
        };
        for name in &report.removed {
            entry.handle.services.remove(name);
            entry.job_ids.remove(name);
        }
        for (name, job, outcome) in spawned {
            entry.job_ids.insert(name.clone(), job);
            entry.set_service(&name, outcome_state(outcome));
        }
        entry.profile = new_profile;
        entry.emit(
            LaunchEventKind::Restarting,
            None,
            "stack reloaded".to_owned(),
        );
    }
}

// ---- Free helpers -----------------------------------------------------------

/// Returns the first failed REQUIRED dependency of `service`, if any.
///
/// A failed dependency with `required = false` is demoted to a warning and does
/// not block (`launch-optional-dependency-fails-without-blocking`).
#[must_use]
fn blocked_dependency(
    profile: &LaunchProfile,
    service: &LaunchService,
    ready_status: &BTreeMap<ServiceName, ServiceOutcome>,
) -> Option<ServiceName> {
    service.depends_on.iter().find_map(|dep| {
        let failed = ready_status.get(dep) == Some(&ServiceOutcome::Failed);
        let required = profile.services.get(dep).is_none_or(|d| d.required);
        (failed && required).then(|| dep.clone())
    })
}

/// Classifies the diff between the running Profile and the reloaded Profile.
///
/// `restarted` contains every Service whose spawn-affecting fields changed PLUS
/// the transitive [`restart_closure`] of those changes, in new-Profile
/// topological order. `edge_only` contains Services whose only change was a
/// dependency edge (no re-spawn).
#[must_use]
pub(crate) fn compute_reload_report(old: &LaunchProfile, new: &LaunchProfile) -> ReloadReport {
    let old_keys: BTreeSet<&String> = old.services.keys().collect();
    let new_keys: BTreeSet<&String> = new.services.keys().collect();

    let added: Vec<String> = new_keys.difference(&old_keys).map(|s| (*s).clone()).collect();
    let removed: Vec<String> = old_keys.difference(&new_keys).map(|s| (*s).clone()).collect();

    let mut direct_changed: Vec<String> = Vec::new();
    let mut edge_only: Vec<String> = Vec::new();
    for name in old_keys.intersection(&new_keys) {
        let (Some(o), Some(n)) = (old.services.get(*name), new.services.get(*name)) else {
            continue;
        };
        if spawn_fields_differ(o, n) {
            direct_changed.push((*name).clone());
        } else if edges_only_differ(o, n) {
            edge_only.push((*name).clone());
        }
    }

    let mut restarted_set: BTreeSet<String> = direct_changed.iter().cloned().collect();
    restarted_set.extend(restart_closure(new, &direct_changed));
    let restarted = order_in_topo(new, &restarted_set);

    ReloadReport {
        added,
        removed,
        restarted,
        edge_only,
    }
}

/// Orders the members of `set` in `profile` forward-topological order.
///
/// Falls back to ascending-name order when the graph is not a DAG.
#[must_use]
fn order_in_topo(profile: &LaunchProfile, set: &BTreeSet<String>) -> Vec<String> {
    profile.topological_order().map_or_else(
        |_| set.iter().cloned().collect(),
        |order| order.into_iter().filter(|n| set.contains(n)).collect(),
    )
}

/// Returns the members of `set` in reverse-topological (dependents-first) order
/// of the OLD Profile, so a Service is stopped only after its dependents.
#[must_use]
fn reverse_order_for(old: &LaunchProfile, set: &BTreeSet<ServiceName>) -> Vec<ServiceName> {
    reverse_topo(old)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| set.contains(n))
        .collect()
}

/// Returns the current UTC time as an RFC 3339 string (epoch on the unreachable
/// format-failure path, never a panic).
fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// Best-effort canonical absolute path string, falling back to the input.
fn canonical_path_string(path: &str) -> String {
    std::fs::canonicalize(path).map_or_else(|_| path.to_owned(), |p| p.display().to_string())
}

/// The default Profile path used by `init` when the caller passes none.
fn default_profile_path() -> PathBuf {
    PathBuf::from(".substrate.toml")
}

/// Heuristically detects the project type from sibling marker files.
fn detect_project_type(target: &Path) -> Option<String> {
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    if dir.join("Cargo.toml").exists() {
        return Some("rust".to_owned());
    }
    if dir.join("package.json").exists() {
        return Some("node".to_owned());
    }
    None
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use tempfile::TempDir;
    use time::OffsetDateTime;

    use substrate_domain::launch::stack::StackChild;
    use substrate_domain::subprocess::errors::SubprocessError;
    use substrate_domain::subprocess::handle::SubprocessHandle;
    use substrate_domain::subprocess::pagination::{
        SubprocessSearchRequest, SubprocessSearchResult,
    };
    use substrate_domain::subprocess::request::SubprocessRequest;
    use substrate_domain::subprocess::state::SubprocessState;
    use substrate_domain::value_objects::pagination::PageSize;
    use substrate_domain::value_objects::{ClientId, ProcessGroup};

    use super::*;
    use substrate_domain::ports::subprocess::{SignalTarget, SubprocessSignalName};

    /// A scripted [`SubprocessPort`] test double.
    ///
    /// Each spawned binary resolves to a scripted terminal state (default
    /// `Ready`). Spawn and cancel calls are logged so tests can assert ordering.
    struct FakeSubprocessPort {
        outcomes: Mutex<HashMap<String, SubprocessState>>,
        handles: Mutex<HashMap<String, SubprocessHandle>>,
        spawn_log: Mutex<Vec<String>>,
        cancel_log: Mutex<Vec<String>>,
        counter: Mutex<i32>,
    }

    impl FakeSubprocessPort {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                outcomes: Mutex::new(HashMap::new()),
                handles: Mutex::new(HashMap::new()),
                spawn_log: Mutex::new(Vec::new()),
                cancel_log: Mutex::new(Vec::new()),
                counter: Mutex::new(1000),
            })
        }

        /// Scripts `binary` to resolve to `state` on the next readiness poll.
        fn script(&self, binary: &str, state: SubprocessState) {
            self.outcomes.lock().unwrap().insert(binary.to_owned(), state);
        }

        fn spawns(&self) -> Vec<String> {
            self.spawn_log.lock().unwrap().clone()
        }

        fn cancels(&self) -> Vec<String> {
            self.cancel_log.lock().unwrap().clone()
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
            self.spawn_log.lock().unwrap().push(binary.clone());
            let state = self
                .outcomes
                .lock()
                .unwrap()
                .get(&binary)
                .copied()
                .unwrap_or(SubprocessState::Ready);
            let pid = {
                let mut c = self.counter.lock().unwrap();
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
                .unwrap()
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
            let handles = self.handles.lock().unwrap().values().cloned().collect();
            Ok((handles, None))
        }

        async fn cancel(&self, job_id: &JobId, _force: bool) -> SubstrateResult<SubprocessState> {
            let key = job_id.to_crockford();
            self.cancel_log.lock().unwrap().push(key.clone());
            let mut handles = self.handles.lock().unwrap();
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
        ) -> SubstrateResult<substrate_domain::ports::subprocess::SubprocessResult> {
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

    /// A never-cancelled [`CancelSignal`] for tests.
    struct NeverCancel;

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
    async fn write_profile(dir: &Path, body: &str) -> String {
        let path = dir.join(".substrate.toml");
        tokio::fs::write(&path, body.as_bytes())
            .await
            .expect("write profile");
        path.display().to_string()
    }

    fn registry(fake: Arc<FakeSubprocessPort>, dir: &Path) -> Arc<LaunchRegistry> {
        LaunchRegistry::new(fake as Arc<dyn SubprocessPort>, dir.to_path_buf())
    }

    /// A scripted [`DetachLauncher`] double: it returns a synthetic supervisor
    /// registry (success) or fails, without ever forking an OS process. The
    /// success registry echoes the caller's `expected_hash` so the readiness match
    /// the production launcher enforces is preserved.
    struct FakeDetachLauncher {
        /// `Some(child_names)` succeeds; `None` fails with `SupervisorUnreachable`.
        children: Option<Vec<ServiceName>>,
        /// `(stack_id, expected_hash)` recorded per call for assertions.
        calls: Mutex<Vec<(String, String)>>,
    }

    impl FakeDetachLauncher {
        fn succeeding(children: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                children: Some(children.iter().map(|s| (*s).to_owned()).collect()),
                calls: Mutex::new(Vec::new()),
            })
        }

        fn failing() -> Arc<Self> {
            Arc::new(Self {
                children: None,
                calls: Mutex::new(Vec::new()),
            })
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl DetachLauncher for FakeDetachLauncher {
        async fn launch_detached(
            &self,
            stack_id: &StackId,
            _profile_path: &Path,
            expected_hash: &str,
        ) -> Result<SupervisorRegistry, LaunchError> {
            self.calls
                .lock()
                .unwrap()
                .push((stack_id.to_crockford(), expected_hash.to_owned()));
            let Some(names) = &self.children else {
                return Err(LaunchError::SupervisorUnreachable {
                    stack_id: stack_id.to_crockford(),
                });
            };
            let children = names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let n = i32::try_from(i).unwrap();
                    StackChild {
                        name: name.clone(),
                        pid: 5000 + n,
                        pgid: 5000 + n,
                        start_epoch: 1_770_000_001,
                    }
                })
                .collect();
            Ok(SupervisorRegistry {
                supervisor_pid: 4242,
                start_epoch: 1_770_000_000,
                policy: DisconnectPolicy::Detach,
                config_hash: expected_hash.to_owned(),
                children,
            })
        }
    }

    fn registry_with_launcher(
        fake: Arc<FakeSubprocessPort>,
        dir: &Path,
        launcher: Arc<dyn DetachLauncher>,
    ) -> Arc<LaunchRegistry> {
        LaunchRegistry::with_launcher(fake as Arc<dyn SubprocessPort>, dir.to_path_buf(), launcher)
    }

    const THREE_TIER: &str = "version = 1\n\n[services.db]\ncommand = [\"db\"]\n\n[services.api]\ncommand = [\"api\"]\ndepends_on = [\"db\"]\n\n[services.web]\ncommand = [\"web\"]\ndepends_on = [\"api\"]\n";

    #[tokio::test]
    async fn up_starts_services_in_topological_order() {
        // launch-up-readiness-gated-start
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg
            .up(&profile, None, None, &NeverCancel)
            .await
            .expect("up succeeds");

        assert_eq!(handle.state, StackState::Running);
        assert_eq!(fake.spawns(), vec!["db", "api", "web"]);
        assert_eq!(handle.services.get("web"), Some(&SubprocessState::Ready));
    }

    #[tokio::test]
    async fn required_dependency_failure_fails_dependents() {
        // launch-dependency-readiness-timeout-fails-dependents
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        fake.script("db", SubprocessState::Failed);
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let err = reg
            .up(&profile, None, None, &NeverCancel)
            .await
            .expect_err("required dep failure");
        match err {
            LaunchError::DependencyFailed { service, dependency } => {
                assert_eq!(service, "api");
                assert_eq!(dependency, "db");
            },
            other => panic!("expected DependencyFailed, got {other:?}"),
        }
        // api must not be spawned once its required dependency db failed.
        assert!(!fake.spawns().contains(&"api".to_owned()));
    }

    #[tokio::test]
    async fn probe_gated_service_waits_its_budget_then_fails_and_stops() {
        // Regression guard for the readiness-gating bug: a service whose subprocess
        // never leaves `Starting` (its health probe never passes) must NOT be reported
        // Ready. It must wait roughly its per-probe budget, then fail readiness and be
        // stopped — not resolve Ready in microseconds off a freshly spawned child.
        //
        // A LogPattern probe with the minimum timeout_ms (1000) keeps the launch
        // readiness budget near ~1s so the test stays fast while still exercising the
        // real budget path (not the old fixed 1s no-op ceiling).
        let body = "version = 1\n\n[services.web]\ncommand = [\"web\"]\n\n\
                    [services.web.health_probe]\nkind = \"LogPattern\"\n\
                    regex = \"ready\"\ntimeout_ms = 1000\n";
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        // The subprocess stays Starting forever — the probe never confirms readiness.
        fake.script("web", SubprocessState::Starting);
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), body).await;

        reg.trust(&profile).await.expect("trust");
        let started = std::time::Instant::now();
        let handle = reg
            .up(&profile, None, None, &NeverCancel)
            .await
            .expect("up completes (single required service degrades, not errors)");
        let elapsed = started.elapsed();

        // It genuinely waited the probe budget rather than instantly marking Ready.
        assert!(
            elapsed >= std::time::Duration::from_secs(1),
            "readiness must gate on the probe budget; waited only {elapsed:?}"
        );
        assert_eq!(handle.state, StackState::Degraded);
        assert_eq!(handle.services.get("web"), Some(&SubprocessState::Failed));
        // The service that never became ready was stopped, not left running.
        assert!(
            !fake.cancels().is_empty(),
            "a service that fails readiness must be stopped"
        );
    }

    #[tokio::test]
    async fn optional_dependency_failure_does_not_block() {
        // launch-optional-dependency-fails-without-blocking
        let body = "version = 1\n\n[services.cache]\ncommand = [\"cache\"]\nrequired = false\n\n[services.web]\ncommand = [\"web\"]\ndepends_on = [\"cache\"]\n";
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        fake.script("cache", SubprocessState::Failed);
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), body).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg
            .up(&profile, None, None, &NeverCancel)
            .await
            .expect("optional dep failure must not error");

        assert_eq!(handle.state, StackState::Degraded);
        assert_eq!(handle.services.get("web"), Some(&SubprocessState::Ready));
        assert_eq!(handle.services.get("cache"), Some(&SubprocessState::Failed));
        assert!(fake.spawns().contains(&"web".to_owned()));
    }

    #[tokio::test]
    async fn cycle_is_rejected_before_any_spawn() {
        // launch-depends-on-cycle-rejected
        let body = "version = 1\n\n[services.a]\ncommand = [\"a\"]\ndepends_on = [\"b\"]\n\n[services.b]\ncommand = [\"b\"]\ndepends_on = [\"a\"]\n";
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), body).await;

        reg.trust(&profile).await.expect("trust");
        let err = reg
            .up(&profile, None, None, &NeverCancel)
            .await
            .expect_err("cycle rejected");
        assert!(matches!(err, LaunchError::CycleDetected { .. }), "got {err:?}");
        assert!(fake.spawns().is_empty(), "no service may spawn on a cyclic graph");
    }

    #[tokio::test]
    async fn detach_policy_spawns_supervisor_and_returns_detached() {
        // Milestone 2: an explicit detach request hands the Stack to a detached
        // supervisor process and reports it as `Detached`, NOT an error.
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let launcher = FakeDetachLauncher::succeeding(&["db", "api", "web"]);
        let reg = registry_with_launcher(fake.clone(), dir.path(), launcher.clone());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg
            .up(&profile, Some(DisconnectPolicy::Detach), None, &NeverCancel)
            .await
            .expect("detached up succeeds");

        assert_eq!(handle.state, StackState::Detached);
        assert_eq!(handle.policy, DisconnectPolicy::Detach);
        assert!(handle.supervisor.is_some(), "detached handle carries the supervisor registry");
        assert_eq!(handle.services.get("web"), Some(&SubprocessState::Running));
        // The detached supervisor process owns the children; the in-process
        // subprocess port spawns nothing for a detach bring-up.
        assert!(
            fake.spawns().is_empty(),
            "detach delegates spawning to the supervisor process, not the in-session port"
        );
        assert_eq!(launcher.call_count(), 1, "the supervisor is launched exactly once");
    }

    #[tokio::test]
    async fn detach_supervisor_failure_returns_supervisor_unreachable() {
        // A supervisor that never publishes a matching registry within the budget
        // is a legitimate `SupervisorUnreachable` — no longer an unconditional one.
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let launcher = FakeDetachLauncher::failing();
        let reg = registry_with_launcher(fake.clone(), dir.path(), launcher);
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let err = reg
            .up(&profile, Some(DisconnectPolicy::Detach), None, &NeverCancel)
            .await
            .expect_err("a supervisor that never comes up is unreachable");
        assert!(matches!(err, LaunchError::SupervisorUnreachable { .. }), "got {err:?}");
        assert!(fake.spawns().is_empty());
    }

    #[tokio::test]
    async fn down_cascade_stops_every_service() {
        // launch-disconnect-shutdown-kills-stack (in-session variant)
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg.up(&profile, None, None, &NeverCancel).await.expect("up");
        let state = reg.down(&handle.stack_id, &NeverCancel).await.expect("down");

        assert_eq!(state, StackState::Down);
        assert_eq!(fake.cancels().len(), 3, "every service must be cancelled");
        let after = reg
            .status(Some(&handle.stack_id))
            .await
            .expect("status")
            .pop()
            .expect("stack present");
        assert_eq!(after.state, StackState::Down);
        assert_eq!(after.services.get("db"), Some(&SubprocessState::Cancelled));
    }

    #[tokio::test]
    async fn forget_removes_terminal_stack_from_registry() {
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg.up(&profile, None, None, &NeverCancel).await.expect("up");
        reg.down(&handle.stack_id, &NeverCancel).await.expect("down");

        reg.forget(&handle.stack_id).await.expect("forget a Down stack");

        let after = reg.status(Some(&handle.stack_id)).await.expect("status");
        assert!(after.is_empty(), "forgotten stack must not appear in status");
    }

    #[tokio::test]
    async fn forget_rejects_non_terminal_stack() {
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg.up(&profile, None, None, &NeverCancel).await.expect("up");

        let err = reg
            .forget(&handle.stack_id)
            .await
            .expect_err("forget on a Running stack must be rejected");
        assert!(matches!(err, LaunchError::StackNotTerminal { .. }), "got {err:?}");

        let after = reg
            .status(Some(&handle.stack_id))
            .await
            .expect("status")
            .pop()
            .expect("stack still present");
        assert_eq!(after.state, StackState::Running, "stack must be untouched");
    }

    #[tokio::test]
    async fn reload_cascade_restarts_changed_and_dependents_only() {
        // launch-reload-cascade-restart
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg.up(&profile, None, None, &NeverCancel).await.expect("up");
        let before = fake.spawns().len();

        // Change only api's args, then re-bless the edited profile.
        let edited = "version = 1\n\n[services.db]\ncommand = [\"db\"]\n\n[services.api]\ncommand = [\"api\"]\nargs = [\"--v2\"]\ndepends_on = [\"db\"]\n\n[services.web]\ncommand = [\"web\"]\ndepends_on = [\"api\"]\n";
        write_profile(dir.path(), edited).await;
        reg.trust(&profile).await.expect("re-trust edited profile");

        let report = reg
            .reload(&handle.stack_id, None, &NeverCancel)
            .await
            .expect("reload");

        assert!(report.restarted.contains(&"api".to_owned()), "api restarted");
        assert!(report.restarted.contains(&"web".to_owned()), "web cascade-restarted");
        assert!(!report.restarted.contains(&"db".to_owned()), "db untouched");

        let delta: Vec<String> = fake.spawns()[before..].to_vec();
        assert_eq!(delta, vec!["api", "web"], "only api and web re-spawn");
    }

    #[tokio::test]
    async fn reload_metadata_only_restarts_nothing() {
        // launch-reload-metadata-only-no-restart
        let base = "version = 1\n\n[services.app]\ncommand = [\"app\"]\n\n[services.app.restart_policy]\nkind = \"OnFailure\"\nmax_retries = 3\nbackoff_ms = 1000\n";
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), base).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg.up(&profile, None, None, &NeverCancel).await.expect("up");
        let before = fake.spawns().len();

        let edited = "version = 1\n\n[services.app]\ncommand = [\"app\"]\n\n[services.app.restart_policy]\nkind = \"OnFailure\"\nmax_retries = 5\nbackoff_ms = 1000\n";
        write_profile(dir.path(), edited).await;
        reg.trust(&profile).await.expect("re-trust");

        let report = reg
            .reload(&handle.stack_id, None, &NeverCancel)
            .await
            .expect("reload");

        assert!(report.restarted.is_empty(), "metadata-only change restarts nothing");
        assert_eq!(fake.spawns().len(), before, "no child re-spawns");
    }

    #[tokio::test]
    async fn restart_single_service_respawns_and_stays_running() {
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg.up(&profile, None, None, &NeverCancel).await.expect("up");
        let before = fake.spawns().len();

        let updated = reg
            .restart(&handle.stack_id, "api", &NeverCancel)
            .await
            .expect("restart api");

        assert_eq!(updated.services.get("api"), Some(&SubprocessState::Ready));
        assert_eq!(fake.spawns().len(), before + 1, "api re-spawned once");
        assert_eq!(fake.cancels().len(), 1, "old api job cancelled");
    }

    #[tokio::test]
    async fn logs_return_lifecycle_events_and_honor_cursor() {
        // launch-event-pull-floor-no-subscribe: events readable via logs polling.
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg.up(&profile, None, None, &NeverCancel).await.expect("up");

        let (events, cursor) = reg
            .logs(&handle.stack_id, None, None)
            .await
            .expect("logs");
        assert!(!events.is_empty(), "lifecycle events must be present");
        assert!(events.iter().any(|e| e.kind == LaunchEventKind::Started));
        assert!(events.iter().any(|e| e.kind == LaunchEventKind::Ready));

        // A since-cursor at the last seq yields no further events.
        let last_seq = cursor.expect("cursor present");
        let (tail, _) = reg
            .logs(&handle.stack_id, None, Some(&last_seq))
            .await
            .expect("tail");
        assert!(tail.is_empty(), "no events strictly after the last cursor");
    }

    #[tokio::test]
    async fn logs_filter_by_service() {
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        reg.trust(&profile).await.expect("trust");
        let handle = reg.up(&profile, None, None, &NeverCancel).await.expect("up");

        let (events, _) = reg
            .logs(&handle.stack_id, Some("db"), None)
            .await
            .expect("logs db");
        assert!(events.iter().all(|e| e.service.as_deref() == Some("db")));
        assert!(!events.is_empty());
    }

    #[tokio::test]
    async fn list_does_not_require_trust() {
        // launch-list-no-trust-required
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        let catalog = reg.list(&profile).await.expect("list without trust");
        assert_eq!(catalog.len(), 3);
        assert!(fake.spawns().is_empty(), "list spawns nothing");
    }

    #[tokio::test]
    async fn trust_blesses_without_spawning() {
        // launch-trust-blesses-profile
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;

        let record = reg.trust(&profile).await.expect("trust");
        assert!(record.content.starts_with("blake3:"));
        assert!(fake.spawns().is_empty(), "trust spawns nothing");
        assert!(
            tokio::fs::try_exists(dir.path().join(TRUST_STORE_FILE))
                .await
                .unwrap_or(false),
            "trust store written"
        );
    }

    #[tokio::test]
    async fn init_writes_scaffold() {
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let target = dir.path().join(".substrate.toml");

        let written = reg
            .init(Some(&target.display().to_string()), Some("rust"))
            .await
            .expect("init");
        assert!(written.ends_with(".substrate.toml"));
        let body = tokio::fs::read_to_string(&target).await.expect("read back");
        assert!(body.contains("cargo"), "rust hint seeds a cargo command");
    }

    #[tokio::test]
    async fn status_empty_on_new_registry() {
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake, dir.path());
        let handles = reg.status(None).await.expect("status");
        assert!(handles.is_empty());
    }

    #[tokio::test]
    async fn semantic_event_is_redacted_at_source() {
        // launch-event-redaction-at-source: a per-service redact pattern masks a
        // secret before it reaches the event log.
        let dir = TempDir::new().expect("tempdir");
        let fake = FakeSubprocessPort::new();
        let reg = registry(fake.clone(), dir.path());
        let profile = write_profile(dir.path(), THREE_TIER).await;
        reg.trust(&profile).await.expect("trust");
        let handle = reg.up(&profile, None, None, &NeverCancel).await.expect("up");

        // Drive the redaction seam directly: a Semantic event built from a line
        // carrying a secret must store only the masked form.
        {
            let mut entry = reg.stacks.get_mut(&handle.stack_id).expect("entry");
            entry.emit_semantic("api", "token=s3cr3t-value here", &["s3cr3t-value".to_owned()]);
        }
        let (events, _) = reg
            .logs(&handle.stack_id, Some("api"), None)
            .await
            .expect("logs");
        let semantic = events
            .iter()
            .find(|e| e.kind == LaunchEventKind::Semantic)
            .expect("semantic event present");
        assert!(!semantic.message.contains("s3cr3t-value"), "secret must be redacted");
        assert!(semantic.message.contains("[REDACTED]"));
    }
}
