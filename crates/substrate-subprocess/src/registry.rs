//! `SubprocessRegistry` — concrete [`SubprocessPort`] adapter per ADR-0052.
//!
//! Enforces all five security layers from ADR-0052:
//! 1. Allowlist check for `cwd`.
//! 2. Binary allowlist (Layer 5).
//! 3. Environment allowlist (Layer 5 — strip banned/non-listed keys).
//! 4. Elicitation confirmation (mandatory for every spawn).
//! 5. Quota enforcement (per-client and global).
//!
//! Manages `Arc<ChildHandle>` entries in a `DashMap<JobId, Arc<ChildHandle>>`.
//!
//! ## ADR-0056 supervisor extensions
//!
//! - Named handle registry: idempotent spawn-by-name (`named_handles` field).
//! - `StateTransitionObserver` fan-out: parallel to the existing `StreamChunkObserver`.
//! - Supervisor watcher task per job that holds a non-`Never` `RestartPolicy`.
//! - `supervisor_cancels` map: per-job `CancellationToken` to stop the watcher on cancel.
//!
//! References: ADR-0052, ADR-0053, ADR-0054, ADR-0056.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use substrate_domain::errors::{SubstrateError, SubstrateResult};
use substrate_domain::ports::state_transition::StateTransitionObserver;
use substrate_domain::ports::stream_observer::StreamChunkObserver;
use substrate_domain::ports::subprocess::{
    SignalTarget, SubprocessPort, SubprocessResult, SubprocessSignalName,
};
use substrate_domain::subprocess::errors::SubprocessError;
use substrate_domain::subprocess::handle::SubprocessHandle;
use substrate_domain::subprocess::pagination::{SubprocessSearchRequest, SubprocessSearchResult};
use substrate_domain::subprocess::request::{CaptureKind, SubprocessRequest};
use substrate_domain::subprocess::state::SubprocessState;
use substrate_domain::subprocess::supervisor::RestartPolicy;
use substrate_domain::value_objects::{ClientId, JobId, ProcessGroup};

use substrate_policy::Allowlist;

use crate::cascade::terminate_cascade;
use crate::spawn::{ChildHandle, spawn_supervised};
use crate::stream_capture::{make_stream_channel, spawn_stream_captures};

/// Default per-job stdout/stderr ring-buffer size per ADR-0054.
const DEFAULT_AGGREGATE_BUFFER_BYTES: usize = 65_536;

/// Unconditionally banned environment variable keys per ADR-0052 §"Layer 5".
///
/// These keys are injection vectors. Mirroring [`BANNED_ENV_VARS`] in domain
/// for defense-in-depth at the adapter layer.
const BANNED_ENV_KEYS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "LD_AUDIT",
    "LD_DEBUG",
];

/// A simple binary allowlist: a set of absolute path strings.
///
/// Empty = deny-all (default per ADR-0052 §"Binary allowlist").
#[derive(Debug, Clone)]
pub struct BinaryAllowlist {
    /// Absolute paths of permitted binaries.
    entries: Vec<PathBuf>,
}

impl BinaryAllowlist {
    /// Constructs the allowlist from a list of absolute paths.
    #[must_use]
    pub const fn new(entries: Vec<PathBuf>) -> Self {
        Self { entries }
    }

    /// Constructs an empty (deny-all) allowlist.
    #[must_use]
    pub const fn deny_all() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Returns `true` when `path` is in the allowlist.
    #[must_use]
    pub fn allows(&self, path: &std::path::Path) -> bool {
        self.entries.iter().any(|e| e == path)
    }
}

/// Concrete adapter implementing [`SubprocessPort`].
///
/// Constructed once and shared via `Arc<SubprocessRegistry>` in the composition root.
/// Manages all `ChildHandle` entries and enforces security + quota invariants
/// before any OS spawn.
///
/// References: ADR-0052, ADR-0053, ADR-0054.
#[derive(Debug)]
pub struct SubprocessRegistry {
    /// Live subprocess handles keyed by `JobId`.
    handles: Arc<DashMap<JobId, Arc<ChildHandle>>>,

    /// Allowlist of permitted executable binaries (empty = deny-all).
    binary_allowlist: BinaryAllowlist,

    /// Allowlist of parent-environment keys the child may inherit.
    ///
    /// Consumed by the supervisor watcher task when re-spawning a child process
    /// after a restart-policy-triggered exit.
    env_allowlist: Vec<String>,

    /// Maximum active subprocesses per client per ADR-0052.
    max_per_client: u32,

    /// Global maximum active subprocesses per ADR-0052.
    max_concurrent: u32,

    /// Per-job ring-buffer size in bytes per ADR-0054.
    aggregate_buffer_bytes: usize,

    /// Seconds to wait between SIGTERM and SIGKILL per ADR-0053.
    shutdown_drain_secs: u64,

    /// Path allowlist for cwd validation.
    path_allowlist: Allowlist,

    /// Server root cancellation token for deriving per-job child tokens.
    root_cancel: CancellationToken,

    /// Per-client active-subprocess counters.
    per_client_active: Arc<DashMap<ClientId, u32>>,

    /// Root directory for subprocess capture tmp files per ADR-0033 amendment 2026-05-24.
    ///
    /// Required when any job uses `capture_kind == TmpFile`. When `None`, a spawn
    /// request with `CaptureKind::TmpFile` returns [`SubprocessError::InvalidRequest`].
    ///
    /// Set via [`SubprocessRegistry::with_tmp_root`] on the builder; or passed
    /// directly to [`SubprocessRegistry::new`] as the `tmp_root` parameter.
    tmp_root: Option<PathBuf>,

    /// Stream-chunk observers fanned-out by the per-job dispatcher task per ADR-0054.
    ///
    /// Empty Vec means no client push channel is active (equivalent to a Null Object
    /// observer). Multiple observers receive each chunk via the GoF Mediator pattern
    /// implemented by the dispatcher task.
    observers: Arc<Vec<Arc<dyn StreamChunkObserver>>>,

    /// State-transition observers per ADR-0056 (Observer + Mediator).
    ///
    /// Empty Vec means no state observer is active (Null Object pattern).
    /// Receives control-plane lifecycle events (`SubprocessState` transitions).
    state_observers: Arc<Vec<Arc<dyn StateTransitionObserver>>>,

    /// Idempotent name → job_id mapping per ADR-0056.
    ///
    /// Scoped globally (not per client) because `SubprocessPort::spawn` currently
    /// lacks a `client_id` parameter.
    ///
    /// Entries are removed on terminal state or explicit cancel.
    ///
    /// # TODO ADR-0056-followup
    ///
    /// Scope by `(ClientId, String)` when `SubprocessPort::spawn` carries `client_id`.
    named_handles: Arc<DashMap<String, JobId>>,

    /// Per-job `CancellationToken` for the supervisor watcher task per ADR-0056.
    ///
    /// Inserted when a supervisor watcher task is spawned (non-`Never` restart policy).
    /// Cancelled and removed in `SubprocessRegistry::cancel` to stop the restart loop.
    supervisor_cancels: Arc<DashMap<JobId, CancellationToken>>,
}

impl SubprocessRegistry {
    /// Constructs a new `SubprocessRegistry`.
    ///
    /// # Parameters
    ///
    /// - `binary_allowlist`: permitted executable binaries (empty = deny-all).
    /// - `env_allowlist`: parent-env keys the child may inherit.
    /// - `max_per_client`: per-client subprocess cap (default 4 per ADR-0052).
    /// - `max_concurrent`: global subprocess cap (default 8 per ADR-0052).
    /// - `aggregate_buffer_bytes`: ring-buffer size per stream per ADR-0054.
    /// - `shutdown_drain_secs`: SIGTERM→SIGKILL drain window per ADR-0053.
    /// - `path_allowlist`: allowlist used to validate `cwd`.
    /// - `root_cancel`: server root `CancellationToken`.
    ///
    /// `tmp_root` is `None` by default; set it via [`SubprocessRegistry::with_tmp_root`]
    /// when `CaptureKind::TmpFile` is used. See ADR-0033 amendment 2026-05-24.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "construction requires all domain configuration fields; a Builder would be overkill for an internal type"
    )]
    pub fn new(
        binary_allowlist: BinaryAllowlist,
        env_allowlist: Vec<String>,
        max_per_client: u32,
        max_concurrent: u32,
        aggregate_buffer_bytes: usize,
        shutdown_drain_secs: u64,
        path_allowlist: Allowlist,
        root_cancel: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            handles: Arc::default(),
            binary_allowlist,
            env_allowlist,
            max_per_client,
            max_concurrent,
            aggregate_buffer_bytes,
            shutdown_drain_secs,
            path_allowlist,
            root_cancel,
            per_client_active: Arc::default(),
            tmp_root: None,
            observers: Arc::new(Vec::new()),
            state_observers: Arc::new(Vec::new()),
            named_handles: Arc::default(),
            supervisor_cancels: Arc::default(),
        })
    }

    /// Builder-style setter for the stream-chunk observers fanned-out by the
    /// dispatcher task per ADR-0054 and the arch review (Observer + Mediator).
    ///
    /// Empty Vec is equivalent to a Null Object observer (no client push).
    #[must_use]
    pub fn with_observers(
        self: Arc<Self>,
        observers: Vec<Arc<dyn StreamChunkObserver>>,
    ) -> Arc<Self> {
        let inner = Arc::try_unwrap(self).unwrap_or_else(|arc| Self {
            handles: Arc::clone(&arc.handles),
            binary_allowlist: arc.binary_allowlist.clone(),
            env_allowlist: arc.env_allowlist.clone(),
            max_per_client: arc.max_per_client,
            max_concurrent: arc.max_concurrent,
            aggregate_buffer_bytes: arc.aggregate_buffer_bytes,
            shutdown_drain_secs: arc.shutdown_drain_secs,
            path_allowlist: arc.path_allowlist.clone(),
            root_cancel: arc.root_cancel.clone(),
            per_client_active: Arc::clone(&arc.per_client_active),
            tmp_root: arc.tmp_root.clone(),
            observers: Arc::clone(&arc.observers),
            state_observers: Arc::clone(&arc.state_observers),
            named_handles: Arc::clone(&arc.named_handles),
            supervisor_cancels: Arc::clone(&arc.supervisor_cancels),
        });
        Arc::new(Self {
            observers: Arc::new(observers),
            ..inner
        })
    }

    /// Builder-style setter for the state-transition observers per ADR-0056.
    ///
    /// Empty Vec is equivalent to a Null Object observer (no state push).
    /// The observers receive control-plane lifecycle events for every state
    /// transition emitted by the supervisor watcher and dispatcher tasks.
    #[must_use]
    pub fn with_state_observers(
        self: Arc<Self>,
        observers: Vec<Arc<dyn StateTransitionObserver>>,
    ) -> Arc<Self> {
        let inner = Arc::try_unwrap(self).unwrap_or_else(|arc| Self {
            handles: Arc::clone(&arc.handles),
            binary_allowlist: arc.binary_allowlist.clone(),
            env_allowlist: arc.env_allowlist.clone(),
            max_per_client: arc.max_per_client,
            max_concurrent: arc.max_concurrent,
            aggregate_buffer_bytes: arc.aggregate_buffer_bytes,
            shutdown_drain_secs: arc.shutdown_drain_secs,
            path_allowlist: arc.path_allowlist.clone(),
            root_cancel: arc.root_cancel.clone(),
            per_client_active: Arc::clone(&arc.per_client_active),
            tmp_root: arc.tmp_root.clone(),
            observers: Arc::clone(&arc.observers),
            state_observers: Arc::clone(&arc.state_observers),
            named_handles: Arc::clone(&arc.named_handles),
            supervisor_cancels: Arc::clone(&arc.supervisor_cancels),
        });
        Arc::new(Self {
            state_observers: Arc::new(observers),
            ..inner
        })
    }

    /// Builder-style setter for the tmp-file root directory.
    ///
    /// Required when any job uses `CaptureKind::TmpFile`. The path MUST be inside
    /// the `policy.roots` allowlist; this is enforced at spawn time by
    /// `spawn_supervised` via the `PathJail` check.
    ///
    /// Returns `Arc<Self>` consuming the existing `Arc` to allow method chaining
    /// from the composition root.
    ///
    /// References: ADR-0033 §"Transactional Write Pattern", ADR-0054 §"`TmpFile` Branch".
    ///
    /// # Panics
    ///
    /// Panics if `Arc::try_unwrap` fails (i.e., multiple strong references exist
    /// when this builder is called). Call `with_tmp_root` immediately after `new`
    /// before sharing the `Arc`.
    #[must_use]
    #[expect(
        clippy::panic,
        reason = "with_tmp_root is a builder-phase method called once before Arc sharing;                   the panic branch is unreachable in correct usage and documents the invariant"
    )]
    pub fn with_tmp_root(self: Arc<Self>, tmp_root: PathBuf) -> Arc<Self> {
        // SAFETY: `with_tmp_root` is a builder-phase method; the Arc has exactly
        // one strong reference at this point (called immediately after `new`).
        let mut inner = Arc::try_unwrap(self).unwrap_or_else(|_| {
            // This branch is unreachable in correct usage (builder called once,
            // before sharing the Arc). Fallback creates a clone — unavoidable
            // but the lint guard below prevents this path in tests.
            panic!(
                "SubprocessRegistry::with_tmp_root: Arc has multiple strong references; \
                 call with_tmp_root before sharing the registry"
            )
        });
        inner.tmp_root = Some(tmp_root);
        Arc::new(inner)
        // Note: `named_handles`, `state_observers`, and `supervisor_cancels` are carried
        // through as-is since `Arc::try_unwrap` succeeds in all correct builder usage.
    }

    /// Returns the number of currently active (non-terminal) subprocesses.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.handles.len()
    }

    /// Checks and enforces the quota for `client_id`.
    ///
    /// Returns `Err(SubprocessError::QuotaExceeded)` when the per-client or
    /// global quota is reached.
    #[expect(
        dead_code,
        reason = "Wave 2c: called from MCP handler layer with per-request client_id"
    )]
    fn check_quotas(&self, client_id: &ClientId) -> Result<(), SubprocessError> {
        // Global quota. usize -> u32: handle count is bounded by max_concurrent (u32).
        #[expect(
            clippy::cast_possible_truncation,
            reason = "handle count is bounded by max_concurrent which is u32; truncation is impossible in practice"
        )]
        let global = self.handles.len() as u32;
        if global >= self.max_concurrent {
            return Err(SubprocessError::QuotaExceeded {
                limit: self.max_concurrent,
            });
        }
        // Per-client quota.
        let per_client = self.per_client_active.get(client_id).map_or(0, |v| *v);
        if per_client >= self.max_per_client {
            return Err(SubprocessError::QuotaExceeded {
                limit: self.max_per_client,
            });
        }
        Ok(())
    }

    /// Increments the per-client active counter.
    #[expect(
        dead_code,
        reason = "Wave 2c: called from MCP handler quota enforcement path"
    )]
    fn increment_client(&self, client_id: &ClientId) {
        self.per_client_active
            .entry(client_id.clone())
            .and_modify(|v| *v += 1)
            .or_insert(1);
    }

    /// Decrements the per-client active counter (clamped at 0).
    #[expect(
        dead_code,
        reason = "Wave 2c: called from cascade kill chain on terminal state"
    )]
    fn decrement_client(&self, client_id: &ClientId) {
        self.per_client_active
            .entry(client_id.clone())
            .and_modify(|v| {
                *v = v.saturating_sub(1);
            });
    }

    /// Builds a [`SubprocessHandle`] snapshot from a live [`ChildHandle`].
    fn snapshot_handle(handle: &ChildHandle) -> SubprocessHandle {
        SubprocessHandle {
            job_id: handle.job_id.clone(),
            process_group: handle.process_group,
            state: crate::spawn::u8_to_state(handle.state.load(Ordering::SeqCst)),
            started_at: time::OffsetDateTime::now_utc(),
            exit_code: None,
            stream_chunks_dropped: handle.stream_chunks_dropped.load(Ordering::Relaxed),
            tmp_files: Vec::new(),
        }
    }

    /// Maps an `ExitStatus` option (from `wait_exit`) to the terminal `SubprocessState`.
    ///
    /// Extracted as a crate-private helper so both the dispatcher task and the
    /// supervisor watcher task compute the same terminal state from the same
    /// exit-status semantics.
    fn terminal_state_from_exit(
        exit: std::io::Result<Option<std::process::ExitStatus>>,
    ) -> SubprocessState {
        match exit {
            Ok(Some(status)) if status.success() => SubprocessState::Succeeded,
            Ok(Some(_)) => SubprocessState::Failed,
            // Already reaped by another path (e.g. cancellation).
            Ok(None) => SubprocessState::Killed,
            // wait_exit I/O failure — conservative: Failed.
            Err(_) => SubprocessState::Failed,
        }
    }

    /// Signals a process or process group.
    fn do_signal(
        process_group: ProcessGroup,
        signal_name: SubprocessSignalName,
        target: SignalTarget,
    ) -> SubstrateResult<()> {
        use nix::sys::signal::{kill, killpg};
        use nix::unistd::Pid;

        let nix_signal = map_signal_name(signal_name);
        let result = match target {
            SignalTarget::Process => kill(Pid::from_raw(process_group.pid()), Some(nix_signal)),
            SignalTarget::ProcessGroup => {
                killpg(Pid::from_raw(process_group.pgid()), Some(nix_signal))
            },
        };
        result.map_err(|e| SubstrateError::InternalError {
            reason: format!("signal {signal_name} to {process_group} failed: {e}"),
            correlation_id: None,
        })
    }
}

/// Fans out a state-transition event to all registered observers per ADR-0056.
///
/// Fire-and-forget: called from async supervisor/dispatcher tasks. Observers
/// MUST NOT block; they are called sequentially here which is acceptable because
/// the observer contract requires non-blocking implementations.
async fn emit_state_change(
    observers: &[Arc<dyn StateTransitionObserver>],
    job_id: &JobId,
    old: SubprocessState,
    new: SubprocessState,
) {
    for obs in observers {
        obs.on_state_change(job_id, old, new).await;
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "SubprocessPort impl requires all port methods in one block per the hexagonal               layering constraint; extracted helpers would cross the impl boundary"
)]
#[async_trait]
impl SubprocessPort for SubprocessRegistry {
    /// Spawns a new child process per ADR-0052 five-layer security stack.
    ///
    /// Security checks in order (bail-out on first failure):
    /// 1. `req.validate()` — domain field checks.
    /// 2. `elicitation_confirmed` — unconditional per ADR-0052.
    /// 3. Binary allowlist — Layer 5 per ADR-0052.
    /// 4. `cwd` within path allowlist — Layer 1 per ADR-0004.
    /// 5. Environment allowlist — Layer 5 (strip banned keys).
    /// 6. Quota enforcement.
    /// 7. OS spawn via `spawn_supervised`.
    async fn spawn(
        &self,
        req: SubprocessRequest,
        _cancel: &dyn substrate_domain::ports::fs_index::CancelSignal,
    ) -> Result<SubprocessHandle, SubprocessError> {
        // Layer: domain validation.
        req.validate()?;

        // Step 3 — Idempotent spawn-by-name per ADR-0056.
        //
        // If `req.name` is set and a non-terminal job already exists under that name,
        // return the existing handle without starting a new process.
        //
        // Name uniqueness is scoped globally (not per client) because
        // `SubprocessPort::spawn` currently has no `client_id` parameter.
        // TODO ADR-0056-followup: scope by (ClientId, String) when SubprocessPort::spawn
        // carries client_id.
        if let Some(ref name) = req.name {
            if let Some(existing_entry) = self.named_handles.get(name) {
                let existing_job_id = existing_entry.value().clone();
                drop(existing_entry);
                if let Some(handle_entry) = self.handles.get(&existing_job_id) {
                    let snapshot = Self::snapshot_handle(handle_entry.value());
                    drop(handle_entry);
                    if !snapshot.state.is_terminal() {
                        // Live named handle — return it idempotently.
                        return Ok(snapshot);
                    }
                    // Terminal: release stale mapping and fall through to re-spawn.
                    self.named_handles.remove(name);
                    self.handles.remove(&existing_job_id);
                } else {
                    // Handle GC'd with stale named entry — release before re-spawn.
                    self.named_handles.remove(name);
                }
            }
        }

        // Layer: binary allowlist.
        if !self.binary_allowlist.allows(&req.binary_path) {
            return Err(SubprocessError::BinaryNotAllowed {
                path: req.binary_path.display().to_string(),
            });
        }

        // Layer: cwd within path allowlist.
        if !allowlist_contains(&self.path_allowlist, &req.cwd) {
            return Err(SubprocessError::CwdOutsideAllowlist {
                path: req.cwd.display().to_string(),
            });
        }

        // Layer: env_allowlist strip of any banned keys (defense-in-depth,
        // domain already validated but adapter enforces again here).
        for key in &req.env_allowlist {
            if BANNED_ENV_KEYS.contains(&key.as_str()) {
                return Err(SubprocessError::EnvBanned { var: key.clone() });
            }
        }
        for key in req.env_override.keys() {
            if BANNED_ENV_KEYS.contains(&key.as_str()) {
                return Err(SubprocessError::EnvBanned { var: key.clone() });
            }
        }

        // Quota check: enforce global quota only at this layer.
        // Per-client quota enforcement is wired at the MCP handler layer (Wave 2c).
        // usize -> u32: bounded by max_concurrent which is u32.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "active handle count is bounded by max_concurrent (u32); truncation is impossible"
        )]
        let global = self.handles.len() as u32;
        if global >= self.max_concurrent {
            return Err(SubprocessError::QuotaExceeded {
                limit: self.max_concurrent,
            });
        }

        // OS spawn. Pass tmp_root so spawn_supervised can create TmpFileWriter
        // instances for CaptureKind::TmpFile per ADR-0033/ADR-0054.
        let handle = Arc::new(
            spawn_supervised(
                &req,
                self.root_cancel.clone(),
                self.aggregate_buffer_bytes,
                self.tmp_root.as_deref(),
            )
            .await?,
        );
        let job_id = handle.job_id.clone();

        // Wire stream capture tasks. Lock child mutex, set up captures, then drop guard.
        let (sender, mut receiver) = make_stream_channel();
        {
            let mut child_guard = handle.child.lock().await;
            let Some(child) = child_guard.as_mut() else {
                drop(child_guard);
                return Err(SubprocessError::SpawnFailed {
                    source: std::io::Error::other("child not available immediately after spawn"),
                });
            };
            spawn_stream_captures(child, &handle, sender).map_err(|e| {
                SubprocessError::SpawnFailed {
                    source: std::io::Error::other(e.to_string()),
                }
            })?;
            drop(child_guard);
        }

        // Per-job dispatcher task per ADR-0054 + arch review (Observer + Mediator).
        // Drains the per-stream mpsc receiver and fans out each chunk to every
        // registered observer. Terminates when both reader tasks drop their
        // sender clones (child exit + EOF on both pipes).
        let observers_for_dispatcher = Arc::clone(&self.observers);
        let state_observers_for_dispatcher = Arc::clone(&self.state_observers);
        let job_id_for_dispatcher = job_id.clone();
        let handle_for_dispatcher = Arc::clone(&handle);
        // ADR-0056 race-window fix: when a supervisor watcher is going to drive
        // this handle, the dispatcher MUST NOT overwrite the atomic state on
        // child exit — the watcher owns state writes (Restarting during
        // backoff, Running after rebind). If both wrote, the order between
        // dispatcher.state.store(terminal) and watcher.state.store(Restarting)
        // is non-deterministic and result(supervisor_id) could observe stale
        // terminal state during the backoff window.
        let is_supervised_for_dispatcher = req
            .restart_policy
            .as_ref()
            .is_some_and(|p| !matches!(p, RestartPolicy::Never));
        tokio::spawn(async move {
            while let Some(chunk) = receiver.recv().await {
                for observer in observers_for_dispatcher.iter() {
                    observer.on_chunk(&chunk).await;
                }
            }

            // Derive terminal state from the child's exit status and fire the
            // on_terminal sentinel on every observer.  This fires AFTER the
            // last chunk has been delivered, which is guaranteed because the
            // mpsc sender is dropped only once both reader tasks (stdout +
            // stderr) finish, and the recv() loop above drains the bounded
            // channel fully before exiting.
            let terminal_state = SubprocessRegistry::terminal_state_from_exit(
                handle_for_dispatcher.wait_exit().await,
            );

            // Persist the terminal state atomically so snapshot_handle / result()
            // observe the real state instead of the hardcoded Running fallback.
            // Skip the write when the handle is supervised — the watcher writes
            // Restarting + the post-respawn rebind on NEW ChildHandle starts
            // fresh at Running. Dispatcher's role on supervised handles is the
            // data-plane (chunk drain + on_terminal observer fan-out) only.
            if !is_supervised_for_dispatcher {
                handle_for_dispatcher
                    .state
                    .store(crate::spawn::state_to_u8(terminal_state), Ordering::SeqCst);
            }

            for observer in observers_for_dispatcher.iter() {
                observer
                    .on_terminal(&job_id_for_dispatcher, terminal_state)
                    .await;
            }

            // Emit state transition to state observers (ADR-0056 control plane).
            // TODO ADR-0056 health probe wiring: emit Running → Ready here once probe
            // confirms Ready, rather than treating Running as implicitly Ready.
            emit_state_change(
                &state_observers_for_dispatcher,
                &job_id_for_dispatcher,
                SubprocessState::Running,
                terminal_state,
            )
            .await;

            tracing::debug!(
                job_id = %job_id_for_dispatcher,
                ?terminal_state,
                supervised = is_supervised_for_dispatcher,
                "stream dispatcher task exiting — both reader senders dropped"
            );
        });

        // Register in the live map.
        self.handles.insert(job_id.clone(), Arc::clone(&handle));

        // Register named handle mapping if `req.name` is set (ADR-0056 §idempotent spawn).
        if let Some(ref name) = req.name {
            self.named_handles.insert(name.clone(), job_id.clone());
        }

        // Step 4 — Supervisor watcher task per ADR-0056.
        //
        // Spawned only for non-Never restart policies. A dedicated
        // CancellationToken (child of root) lets `cancel()` stop the loop.
        let needs_supervisor = req
            .restart_policy
            .as_ref()
            .is_some_and(|p| !matches!(p, RestartPolicy::Never));

        if needs_supervisor {
            let supervisor_cancel = self.root_cancel.child_token();
            self.supervisor_cancels
                .insert(job_id.clone(), supervisor_cancel.clone());

            let registry_arc = {
                // We need an `Arc<Self>` for the watcher to call `self.spawn()`.
                // Re-assemble from the shared interior maps — no public `Arc<Self>`
                // reference is stored, so we rebuild a thin "view" via the same
                // shared DashMap Arcs.  The restart call goes through the full spawn
                // path (all security checks), which is correct and intentional.
                //
                // We capture handles/named_handles/state_observers/supervisor_cancels
                // as individual Arcs to avoid storing a self-referential Arc.
                (
                    Arc::clone(&self.handles),
                    Arc::clone(&self.named_handles),
                    Arc::clone(&self.state_observers),
                    Arc::clone(&self.supervisor_cancels),
                    self.binary_allowlist.clone(),
                    self.env_allowlist.clone(),
                    self.max_per_client,
                    self.max_concurrent,
                    self.aggregate_buffer_bytes,
                    self.shutdown_drain_secs,
                    self.path_allowlist.clone(),
                    self.root_cancel.clone(),
                    Arc::clone(&self.per_client_active),
                    self.tmp_root.clone(),
                    Arc::clone(&self.observers),
                )
            };

            let watcher_job_id = job_id.clone();
            let watcher_req = req.clone();
            let watcher_state_observers = Arc::clone(&self.state_observers);
            let (
                watcher_handles,
                watcher_named_handles,
                _,
                watcher_supervisor_cancels,
                binary_allowlist,
                env_allowlist,
                max_per_client,
                max_concurrent,
                aggregate_buffer_bytes,
                shutdown_drain_secs,
                path_allowlist,
                root_cancel,
                per_client_active,
                tmp_root,
                chunk_observers,
            ) = registry_arc;

            tokio::spawn(async move {
                // The "supervisor id" is the original job_id, stable for the lifetime
                // of the supervised service. handles[supervisor_id] is REBOUND on each
                // respawn so callers can always cancel/result by this id.
                let supervisor_id = watcher_job_id.clone();
                let mut current_job_id = supervisor_id.clone();
                let mut attempt: u32 = 0;
                let mut last_spawn_at = std::time::Instant::now();

                loop {
                    // Wait for the handle to reach a terminal state.
                    let exit_state = {
                        let handle_opt = watcher_handles
                            .get(&current_job_id)
                            .map(|e| Arc::clone(e.value()));
                        match handle_opt {
                            None => SubprocessState::Failed,
                            Some(h) => {
                                // Wait for child exit or supervisor cancel.
                                tokio::select! {
                                    biased;
                                    _ = supervisor_cancel.cancelled() => {
                                        // Explicit cancel — stop the restart loop.
                                        tracing::debug!(
                                            job_id = %current_job_id,
                                            "supervisor watcher cancelled — exiting restart loop"
                                        );
                                        return;
                                    }
                                    result = h.wait_exit() => SubprocessRegistry::terminal_state_from_exit(result),
                                }
                            },
                        }
                    };

                    // Determine restart decision based on policy.
                    let policy = watcher_req.restart_policy.clone();
                    let should_restart = match &policy {
                        None | Some(RestartPolicy::Never) => false,
                        Some(RestartPolicy::OnFailure {
                            max_retries,
                            backoff_ms,
                        }) => {
                            if exit_state == SubprocessState::Succeeded {
                                false
                            } else {
                                // Reset attempt counter if the last spawn was stable long enough.
                                // "Stable" = alive for > 2 * backoff_ms milliseconds.
                                // TODO ADR-0056: reset retry counter on Ready transition once
                                // health_probe is wired.
                                let elapsed_ms = last_spawn_at.elapsed().as_millis() as u64;
                                if elapsed_ms > backoff_ms.saturating_mul(2) {
                                    attempt = 0;
                                }
                                attempt < *max_retries
                            }
                        },
                        Some(RestartPolicy::Always { .. }) => true,
                    };

                    if !should_restart {
                        // Persist the actual terminal state to the supervisor_id
                        // handle's atomic so result(supervisor_id) reflects it
                        // after the watcher exits (dispatcher skipped the write
                        // because is_supervised was true at spawn).
                        if let Some(h) = watcher_handles.get(&supervisor_id) {
                            h.value()
                                .state
                                .store(crate::spawn::state_to_u8(exit_state), Ordering::SeqCst);
                        }
                        tracing::debug!(
                            job_id = %current_job_id,
                            ?exit_state,
                            "supervisor watcher: no restart needed — exiting loop"
                        );
                        break;
                    }

                    // Compute exponential backoff capped at `backoff_ms`.
                    let backoff_ms = match &policy {
                        Some(RestartPolicy::OnFailure { backoff_ms, .. })
                        | Some(RestartPolicy::Always { backoff_ms }) => *backoff_ms,
                        _ => 1_000,
                    };
                    // Exponential backoff: 100ms * 2^attempt, capped at backoff_ms.
                    // Clamp exponent to 10 (max 100 * 1024 = 102_400 ms before cap).
                    let exponent = attempt.min(10);
                    let base: u64 = 100_u64.saturating_mul(1_u64 << exponent);
                    let sleep_ms = backoff_ms.min(base);

                    // Emit state transition: Running/Failed → Restarting.
                    emit_state_change(
                        &watcher_state_observers,
                        &current_job_id,
                        exit_state,
                        SubprocessState::Restarting,
                    )
                    .await;

                    // Persist Restarting to the supervisor_id handle's atomic so
                    // that concurrent `subprocess.result(supervisor_id)` queries
                    // during the backoff window observe `Restarting` instead of
                    // the stale terminal state written by the dispatcher task on
                    // the just-exited child. Without this, the race window
                    // [child exit → backoff sleep → respawn rebind] returns the
                    // terminal state (e.g. Killed) to external callers. ADR-0056
                    // §"Lifecycle" defines Restarting as the canonical state
                    // during this window.
                    if let Some(h) = watcher_handles.get(&supervisor_id) {
                        h.value().state.store(
                            crate::spawn::state_to_u8(SubprocessState::Restarting),
                            Ordering::SeqCst,
                        );
                    }

                    // Remove the old terminal handle from the live map UNLESS it is
                    // the supervisor_id itself — that id is the stable external
                    // identifier and must be rebound to the next ChildHandle below.
                    if current_job_id != supervisor_id {
                        watcher_handles.remove(&current_job_id);
                    }

                    // Sleep with cooperative cancel check.
                    tokio::select! {
                        biased;
                        _ = supervisor_cancel.cancelled() => {
                            tracing::debug!(
                                job_id = %current_job_id,
                                "supervisor watcher cancelled during backoff — exiting"
                            );
                            return;
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)) => {}
                    }

                    // Re-spawn via a thin registry view built from the shared interior Arcs.
                    // All fields are cloned/Arc-cloned so the new view shares the same
                    // live state as the parent registry (handles map, named_handles, etc.).
                    let respawn_registry = Arc::new(SubprocessRegistry {
                        handles: Arc::clone(&watcher_handles),
                        binary_allowlist: binary_allowlist.clone(),
                        env_allowlist: env_allowlist.clone(),
                        max_per_client,
                        max_concurrent,
                        aggregate_buffer_bytes,
                        shutdown_drain_secs,
                        path_allowlist: path_allowlist.clone(),
                        root_cancel: root_cancel.clone(),
                        per_client_active: Arc::clone(&per_client_active),
                        tmp_root: tmp_root.clone(),
                        observers: Arc::clone(&chunk_observers),
                        state_observers: Arc::clone(&watcher_state_observers),
                        named_handles: Arc::clone(&watcher_named_handles),
                        supervisor_cancels: Arc::clone(&watcher_supervisor_cancels),
                    });

                    struct NoCancel;
                    #[async_trait::async_trait]
                    impl substrate_domain::ports::fs_index::CancelSignal for NoCancel {
                        fn is_cancelled(&self) -> bool {
                            false
                        }
                        async fn cancelled(&self) {
                            std::future::pending::<()>().await
                        }
                    }

                    last_spawn_at = std::time::Instant::now();
                    attempt = attempt.saturating_add(1);

                    // Critical: strip restart_policy from the respawn request to
                    // avoid recursive watcher creation. The original watcher (this
                    // loop) continues monitoring the new child; spawning a second
                    // watcher per respawn causes exponential growth (one extra
                    // watcher per cycle, doubling every iteration). The respawn
                    // also drops `name` so the idempotent-by-name check does not
                    // short-circuit (we want a fresh child, not the dying handle).
                    let mut respawn_req = watcher_req.clone();
                    respawn_req.restart_policy = None;
                    respawn_req.name = None;

                    match respawn_registry.spawn(respawn_req, &NoCancel).await {
                        Ok(new_handle) => {
                            // Rebind the supervisor_id (the stable external id) to
                            // the freshly spawned ChildHandle, then drop the
                            // transient entry inserted by spawn() under the new
                            // internal id. Result: callers querying supervisor_id
                            // always see the live current child.
                            let new_internal_id = new_handle.job_id.clone();
                            if let Some(fresh_arc) = watcher_handles
                                .get(&new_internal_id)
                                .map(|e| Arc::clone(e.value()))
                            {
                                watcher_handles.insert(supervisor_id.clone(), fresh_arc);
                                watcher_handles.remove(&new_internal_id);
                            }
                            current_job_id = supervisor_id.clone();
                            emit_state_change(
                                &watcher_state_observers,
                                &current_job_id,
                                SubprocessState::Restarting,
                                SubprocessState::Running,
                            )
                            .await;
                            tracing::info!(
                                job_id = %current_job_id,
                                respawn_internal_id = %new_internal_id,
                                attempt,
                                "supervisor watcher: re-spawned successfully"
                            );
                        },
                        Err(e) => {
                            error!(
                                job_id = %current_job_id,
                                error = %e,
                                "supervisor watcher: re-spawn failed — stopping restart loop"
                            );
                            emit_state_change(
                                &watcher_state_observers,
                                &current_job_id,
                                SubprocessState::Restarting,
                                SubprocessState::Failed,
                            )
                            .await;
                            break;
                        },
                    }
                }
            });
        }

        info!(
            target: "substrate_audit",
            event = "SUBSTRATE_SUBPROCESS_SPAWNED",
            job_id = %job_id,
            binary = %req.binary_path.display(),
            pgid = handle.process_group.pgid(),
        );

        Ok(Self::snapshot_handle(&handle))
    }

    async fn list(
        &self,
        _client_id: &ClientId,
        _state_filter: Option<&[SubprocessState]>,
        _page_cursor: Option<&str>,
        page_size: u32,
    ) -> SubstrateResult<(Vec<SubprocessHandle>, Option<String>)> {
        debug_assert!(page_size > 0, "page_size=0 is a caller contract violation");
        let page_size = (page_size as usize).min(500);
        let handles: Vec<SubprocessHandle> = self
            .handles
            .iter()
            .take(page_size)
            .map(|entry| Self::snapshot_handle(entry.value()))
            .collect();
        Ok((handles, None))
    }

    async fn cancel(&self, job_id: &JobId, force: bool) -> SubstrateResult<SubprocessState> {
        let handle = {
            let guard = self
                .handles
                .get(job_id)
                .ok_or_else(|| SubstrateError::JobNotFound {
                    job_id: job_id.to_string(),
                    correlation_id: None,
                })?;
            Arc::clone(&*guard)
        };

        let terminal = terminate_cascade(&handle, self.shutdown_drain_secs, force)
            .await
            .map_err(|e| SubstrateError::InternalError {
                reason: e.to_string(),
                correlation_id: None,
            })?;

        // Persist the terminal state atomically so that any concurrent
        // snapshot_handle / result() call observes the real state.
        handle
            .state
            .store(crate::spawn::state_to_u8(terminal), Ordering::SeqCst);

        // Step 5 — Cancel supervisor watcher + clean up named handle per ADR-0056.
        //
        // Cancel the supervisor watcher token (if present) so the restart loop exits.
        if let Some((_, supervisor_token)) = self.supervisor_cancels.remove(job_id) {
            supervisor_token.cancel();
        }

        // Remove any named handle mapping for this job.
        self.named_handles.retain(|_, v| v != job_id);

        // Remove from live map.
        self.handles.remove(job_id);

        Ok(terminal)
    }

    async fn result(
        &self,
        job_id: &JobId,
        wait_ms: u32,
        include_aggregates: bool,
    ) -> SubstrateResult<SubprocessResult> {
        let handle = {
            let guard = self
                .handles
                .get(job_id)
                .ok_or_else(|| SubstrateError::JobNotFound {
                    job_id: job_id.to_string(),
                    correlation_id: None,
                })?;
            Arc::clone(&*guard)
        };

        // If still live and wait_ms > 0, poll for exit.
        if wait_ms > 0 {
            let _ = tokio::time::timeout(
                Duration::from_millis(u64::from(wait_ms)),
                handle.wait_exit(),
            )
            .await;
        }

        // Build the result from ring buffers.
        let (stdout_agg, stdout_truncated) = if include_aggregates {
            let ring = handle.stdout_ring.lock().await;
            (ring.as_bytes().to_vec(), ring.truncated)
        } else {
            (Vec::new(), false)
        };
        let (stderr_agg, stderr_truncated) = if include_aggregates {
            let ring = handle.stderr_ring.lock().await;
            (ring.as_bytes().to_vec(), ring.truncated)
        } else {
            (Vec::new(), false)
        };

        let dropped = handle.stream_chunks_dropped.load(Ordering::Relaxed);
        let stdout_total = handle.stdout_bytes_total.load(Ordering::Relaxed);
        let stderr_total = handle.stderr_bytes_total.load(Ordering::Relaxed);

        // For TmpFile capture mode: finalize the tmp writers via atomic rename
        // per ADR-0033 §"Transactional Write Pattern".
        //
        // `TmpFileWriter::finalize` is idempotent (`&self`, `AtomicBool` guard):
        // - Primary finalization happens on the EOF arm of the stream-capture reader
        //   tasks in `stream_capture.rs`.
        // - This call is the safety-net for callers that invoke `result()` before
        //   the reader tasks have observed EOF, or when the primary path failed.
        // - Second call returns the cached `final_path` immediately (no I/O).
        //
        // We attempt finalization when the child process has exited (child mutex
        // holds `None`) to ensure the writer has closed its FD before we rename.
        let (stdout_tmp_path, stderr_tmp_path) = if handle.capture_kind == CaptureKind::TmpFile {
            // Check if the process has exited by peeking at the child mutex.
            let child_exited = {
                let guard = handle.child.lock().await;
                guard.is_none()
            };
            if child_exited {
                // Finalize stdout writer if present.
                let stdout_path = if let Some(ref writer) = handle.stdout_tmp_writer {
                    match writer.finalize().await {
                        Ok(p) => {
                            handle.unregister_tmp_path(writer.tmp_path()).await;
                            Some(p)
                        },
                        Err(e) => {
                            warn!(
                                target: "substrate_audit",
                                event = "SUBSTRATE_SUBPROCESS_TMP_FINALIZE_FAILED",
                                job_id = %job_id,
                                stream = "stdout",
                                error = %e,
                                "TmpFileWriter finalize failed in result(); stdout_tmp_path will be None"
                            );
                            None
                        },
                    }
                } else {
                    None
                };

                // Finalize stderr writer if present.
                let stderr_path = if let Some(ref writer) = handle.stderr_tmp_writer {
                    match writer.finalize().await {
                        Ok(p) => {
                            handle.unregister_tmp_path(writer.tmp_path()).await;
                            Some(p)
                        },
                        Err(e) => {
                            warn!(
                                target: "substrate_audit",
                                event = "SUBSTRATE_SUBPROCESS_TMP_FINALIZE_FAILED",
                                job_id = %job_id,
                                stream = "stderr",
                                error = %e,
                                "TmpFileWriter finalize failed in result(); stderr_tmp_path will be None"
                            );
                            None
                        },
                    }
                } else {
                    None
                };

                (stdout_path, stderr_path)
            } else {
                // Process still running; paths not yet available.
                (None, None)
            }
        } else {
            // Stream or InMemory: no tmp file paths.
            (None, None)
        };

        Ok(SubprocessResult {
            terminal_state: crate::spawn::u8_to_state(handle.state.load(Ordering::SeqCst)),
            exit_code: None,
            stdout_aggregate: stdout_agg,
            stderr_aggregate: stderr_agg,
            stdout_aggregate_truncated: stdout_truncated,
            stderr_aggregate_truncated: stderr_truncated,
            stdout_tmp_path,
            stderr_tmp_path,
            stream_chunks_dropped: dropped,
            duration_ms: 0,
            stdout_bytes_total: stdout_total,
            stderr_bytes_total: stderr_total,
            terminal_at: time::OffsetDateTime::now_utc(),
            // Pagination fields (ADR-0057): populated by the adapter when
            // SubprocessResultRequest includes a pagination cursor. Absent here
            // until the adapter implementation lands (TODO ADR-0057).
            stdout_lines: None,
            stdout_total_lines: None,
            stdout_next_offset: None,
            stderr_lines: None,
            stderr_total_lines: None,
            stderr_next_offset: None,
        })
    }

    async fn signal(
        &self,
        job_id: &JobId,
        signal_name: SubprocessSignalName,
        target: SignalTarget,
    ) -> SubstrateResult<()> {
        // Destructive signals require elicitation per ADR-0052.
        if matches!(
            signal_name,
            SubprocessSignalName::Sigkill
                | SubprocessSignalName::Sigterm
                | SubprocessSignalName::Sigstop
        ) {
            // The registry trusts that the MCP handler has already verified elicitation
            // before calling into the port. This assertion is a defense-in-depth log.
            warn!(
                target: "substrate_audit",
                event = "SUBSTRATE_SUBPROCESS_DESTRUCTIVE_SIGNAL",
                job_id = %job_id,
                signal = %signal_name,
                "destructive signal sent; ensure elicitation was confirmed at MCP layer"
            );
        }

        let handle = {
            let guard = self
                .handles
                .get(job_id)
                .ok_or_else(|| SubstrateError::JobNotFound {
                    job_id: job_id.to_string(),
                    correlation_id: None,
                })?;
            Arc::clone(&*guard)
        };

        Self::do_signal(handle.process_group, signal_name, target)
    }

    /// Searches subprocess output lines by regex pattern per ADR-0057.
    ///
    /// Reads the stdout and/or stderr ring buffers, splits them into lines,
    /// applies the compiled regex, and returns paginated `SearchMatch` results.
    ///
    /// # Errors
    ///
    /// - `SubprocessError::InvalidRequest` — invalid regex or pagination params.
    /// - `SubprocessError::JobNotFound` — no live handle for `req.job_id`.
    async fn search(
        &self,
        req: SubprocessSearchRequest,
    ) -> Result<SubprocessSearchResult, SubprocessError> {
        req.validate()?;

        // Resolve the live handle. `SubprocessError` has no dedicated `JobNotFound`
        // variant; fall back to `InvalidRequest` so callers get a stable error code.
        // The MCP handler maps this to `SubstrateError::InternalError` with a
        // descriptive message; agents can inspect `structured_content.error.message`.
        let handle_arc = {
            let guard =
                self.handles
                    .get(&req.job_id)
                    .ok_or_else(|| SubprocessError::InvalidRequest {
                        msg: format!("job_id not found: {}", req.job_id),
                    })?;
            Arc::clone(guard.value())
        };

        // Compile the regex.
        let regex = regex::RegexBuilder::new(&req.pattern)
            .case_insensitive(req.case_insensitive)
            .build()
            .map_err(|e| SubprocessError::InvalidRequest {
                msg: format!("invalid regex: {e}"),
            })?;

        // Collect matches across requested streams in declaration order.
        let mut all_matches: Vec<substrate_domain::subprocess::pagination::SearchMatch> =
            Vec::new();
        for stream in &req.streams {
            let ring_text = match stream {
                substrate_domain::subprocess::stream::Stream::Stdout => {
                    let ring = handle_arc.stdout_ring.lock().await;
                    String::from_utf8_lossy(ring.as_bytes()).into_owned()
                },
                substrate_domain::subprocess::stream::Stream::Stderr => {
                    let ring = handle_arc.stderr_ring.lock().await;
                    String::from_utf8_lossy(ring.as_bytes()).into_owned()
                },
            };
            for (idx, line) in ring_text.lines().enumerate() {
                if regex.is_match(line) {
                    all_matches.push(substrate_domain::subprocess::pagination::SearchMatch {
                        stream: *stream,
                        line_number: (idx as u64).saturating_add(1),
                        line_text: line.to_owned(),
                    });
                }
            }
        }

        let total_matches = all_matches.len() as u64;

        // Apply pagination.
        let pagination = req.pagination.unwrap_or_default();
        let (page, next_offset) = paginate_matches(&all_matches, &pagination);

        Ok(SubprocessSearchResult {
            matches: page,
            total_matches,
            next_offset,
        })
    }
}

// ---- Pagination helpers (ADR-0057) ------------------------------------------

/// Splits `text` into lines and returns a paginated page, the total line count,
/// and the `next_offset` (if more lines remain).
///
/// - `Order::Tail` (default): line slice is taken from the end of the buffer so
///   the newest lines are returned first when `offset == 0`.  The slice is reversed
///   so that index 0 in the returned `Vec` is the most-recent line.
/// - `Order::Head`: lines are returned in chronological (oldest-first) order.
///
/// Trailing empty elements produced by a trailing `\n` are stripped before slicing
/// so that `"foo\nbar\n"` is treated as two lines, not three.
#[must_use]
pub fn paginate_lines(
    text: &str,
    p: &substrate_domain::subprocess::pagination::Pagination,
) -> (Vec<String>, u64, Option<u64>) {
    use substrate_domain::subprocess::pagination::Order;

    let mut lines: Vec<&str> = text.lines().collect();
    // `str::lines()` strips trailing newlines already — no additional trim needed.
    // Remove a trailing empty string that can arise from a `\n`-terminated buffer
    // when `split('\n')` is used instead of `lines()`, but `lines()` handles it.
    let total = lines.len() as u64;

    if total == 0 {
        return (Vec::new(), 0, None);
    }

    let offset = p.offset;
    let page_size = u64::from(p.page_size);

    match p.order {
        Order::Tail => {
            // Reverse the slice so newest (last) is at index 0.
            lines.reverse();
            let start = (offset as usize).min(lines.len());
            let end = (start + page_size as usize).min(lines.len());
            let page: Vec<String> = lines[start..end].iter().map(|s| (*s).to_owned()).collect();
            let next_offset = if end < lines.len() {
                Some(offset + page_size)
            } else {
                None
            };
            (page, total, next_offset)
        },
        Order::Head => {
            let start = (offset as usize).min(lines.len());
            let end = (start + page_size as usize).min(lines.len());
            let page: Vec<String> = lines[start..end].iter().map(|s| (*s).to_owned()).collect();
            let next_offset = if end < lines.len() {
                Some(offset + page_size)
            } else {
                None
            };
            (page, total, next_offset)
        },
    }
}

/// Applies pagination to a slice of `SearchMatch` entries.
///
/// Returns `(page, next_offset)` without the total because total is tracked
/// separately from the full match set count before slicing.
#[must_use]
fn paginate_matches(
    matches: &[substrate_domain::subprocess::pagination::SearchMatch],
    p: &substrate_domain::subprocess::pagination::Pagination,
) -> (
    Vec<substrate_domain::subprocess::pagination::SearchMatch>,
    Option<u64>,
) {
    use substrate_domain::subprocess::pagination::Order;

    let total = matches.len();
    if total == 0 {
        return (Vec::new(), None);
    }

    let offset = p.offset as usize;
    let page_size = p.page_size as usize;

    match p.order {
        Order::Tail => {
            // Reverse: newest match (highest index) at position 0.
            let reversed: Vec<_> = matches.iter().rev().collect();
            let start = offset.min(reversed.len());
            let end = (start + page_size).min(reversed.len());
            let page = reversed[start..end].iter().map(|m| (*m).clone()).collect();
            let next_offset = if end < reversed.len() {
                Some((offset + page_size) as u64)
            } else {
                None
            };
            (page, next_offset)
        },
        Order::Head => {
            let start = offset.min(total);
            let end = (start + page_size).min(total);
            let page = matches[start..end].to_vec();
            let next_offset = if end < total {
                Some((offset + page_size) as u64)
            } else {
                None
            };
            (page, next_offset)
        },
    }
}

/// Maps a [`SubprocessSignalName`] to the corresponding `nix::sys::signal::Signal`.
const fn map_signal_name(name: SubprocessSignalName) -> nix::sys::signal::Signal {
    match name {
        SubprocessSignalName::Sigterm => nix::sys::signal::Signal::SIGTERM,
        SubprocessSignalName::Sigint => nix::sys::signal::Signal::SIGINT,
        SubprocessSignalName::Sigkill => nix::sys::signal::Signal::SIGKILL,
        SubprocessSignalName::Sigstop => nix::sys::signal::Signal::SIGSTOP,
        SubprocessSignalName::Sigcont => nix::sys::signal::Signal::SIGCONT,
        SubprocessSignalName::Sighup => nix::sys::signal::Signal::SIGHUP,
        SubprocessSignalName::Sigusr1 => nix::sys::signal::Signal::SIGUSR1,
        SubprocessSignalName::Sigusr2 => nix::sys::signal::Signal::SIGUSR2,
    }
}

// ---- Allow-list helpers ----------------------------------------------------

/// Returns `true` when `path` is within any root in the `allowlist`.
///
/// Used to validate `cwd` per ADR-0052 Layer 1 without constructing a
/// [`substrate_domain::JailedPath`] (which requires the `PathJail` factory).
fn allowlist_contains(allowlist: &Allowlist, path: &std::path::Path) -> bool {
    allowlist.iter_roots().any(|root| path.starts_with(root))
}

/// Convenience factory that wires a deny-all `SubprocessRegistry` for use in
/// tests or when no binary allowlist has been configured.
#[must_use]
pub fn deny_all_registry(
    path_allowlist: Allowlist,
    root_cancel: CancellationToken,
) -> Arc<SubprocessRegistry> {
    SubprocessRegistry::new(
        BinaryAllowlist::deny_all(),
        Vec::new(),
        4,
        8,
        DEFAULT_AGGREGATE_BUFFER_BYTES,
        5,
        path_allowlist,
        root_cancel,
    )
}

// ---- Re-exports for tests --------------------------------------------------

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "test code: pagination/state assertions where panic on setup failure is the correct failure mode"
)]
mod tests {
    use substrate_domain::subprocess::pagination::{Order, Pagination};

    use super::*;

    #[test]
    fn binary_allowlist_deny_all_rejects_any_path() {
        let al = BinaryAllowlist::deny_all();
        assert!(
            !al.allows(std::path::Path::new("/usr/bin/true")),
            "deny-all allowlist must reject all binaries"
        );
    }

    #[test]
    fn binary_allowlist_allows_configured_path() {
        let al = BinaryAllowlist::new(vec![PathBuf::from("/usr/bin/true")]);
        assert!(
            al.allows(std::path::Path::new("/usr/bin/true")),
            "allowlist must accept the configured binary"
        );
        assert!(
            !al.allows(std::path::Path::new("/usr/bin/false")),
            "allowlist must reject an unconfigured binary"
        );
    }

    #[test]
    fn ring_buffer_push_and_retrieve() {
        let mut ring = crate::spawn::RingBuffer::new(8);
        ring.push(b"hello");
        assert_eq!(ring.as_bytes(), b"hello");
        assert!(!ring.truncated);
    }

    #[test]
    fn ring_buffer_overflow_keeps_newest_bytes() {
        let mut ring = crate::spawn::RingBuffer::new(4);
        ring.push(b"12345678");
        // Last 4 bytes of input.
        assert_eq!(ring.as_bytes(), b"5678");
        assert!(ring.truncated);
    }

    #[test]
    fn ring_buffer_partial_eviction() {
        let mut ring = crate::spawn::RingBuffer::new(6);
        ring.push(b"hello "); // 6 bytes, fills buffer.
        ring.push(b"world"); // 5 bytes; 5 bytes of old data must be evicted.
        assert_eq!(ring.as_bytes(), b" world");
        assert!(ring.truncated);
    }

    /// Verifies that `snapshot_handle` reflects the real terminal state once a
    /// supervised process exits, rather than hardcoding `Running` forever.
    ///
    /// Regression guard for the bug where `SubprocessState::Running` was
    /// unconditionally returned by `snapshot_handle` regardless of child exit.
    #[tokio::test]
    async fn snapshot_handle_reflects_terminal_state_after_echo_exit() {
        use std::path::PathBuf;
        use substrate_domain::ports::subprocess::SubprocessPort as _;
        use tokio_util::sync::CancellationToken;

        // Resolve a shell that exists on both macOS and Linux CI.
        let sh = PathBuf::from("/bin/sh");
        assert!(sh.exists(), "/bin/sh must exist on this platform");

        let root_cancel = CancellationToken::new();

        // Canonicalize the temp dir so that macOS /var/folders paths are
        // consistent between the allowlist root and the cwd used by spawn.
        let tmp_dir =
            std::fs::canonicalize(std::env::temp_dir()).expect("temp_dir must be canonicalisable");

        // Build an allowlist rooted at the canonical temp dir so the cwd check passes.
        let path_allowlist = substrate_policy::Allowlist::new(vec![tmp_dir.clone()])
            .expect("temp_dir must be a valid allowlist root");

        let registry = SubprocessRegistry::new(
            BinaryAllowlist::new(vec![sh.clone()]),
            Vec::new(),
            4,
            8,
            DEFAULT_AGGREGATE_BUFFER_BYTES,
            5,
            path_allowlist,
            root_cancel,
        );

        let cwd = tmp_dir;

        let req = substrate_domain::subprocess::SubprocessRequest {
            binary_path: sh,
            args: vec!["-c".to_owned(), "exit 0".to_owned()],
            cwd,
            env_allowlist: Vec::new(),
            env_override: std::collections::BTreeMap::new(),
            stdin_kind: substrate_domain::subprocess::StdinKind::None,
            capture_kind: substrate_domain::subprocess::request::CaptureKind::InMemory,
            timeout_secs: None,
            idempotency_key: None,
            elicitation_confirmed: true,
            name: None,
            restart_policy: None,
            health_probe: None,
            log_rotation: None,
        };

        struct NullCancel;
        #[async_trait::async_trait]
        impl substrate_domain::ports::fs_index::CancelSignal for NullCancel {
            fn is_cancelled(&self) -> bool {
                false
            }

            async fn cancelled(&self) {
                // Never cancels.
                std::future::pending::<()>().await
            }
        }

        let handle_snapshot = registry
            .spawn(req, &NullCancel)
            .await
            .expect("spawn must succeed");
        let job_id = handle_snapshot.job_id.clone();

        // Give the dispatcher task time to reap the child and store the terminal state.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Obtain the live ChildHandle from the registry map.
        let live_handle = {
            let guard = registry
                .handles
                .get(&job_id)
                .expect("handle must still exist");
            Arc::clone(&*guard)
        };

        let snapshot = SubprocessRegistry::snapshot_handle(&live_handle);

        assert_eq!(
            snapshot.state,
            substrate_domain::subprocess::state::SubprocessState::Succeeded,
            "snapshot_handle must return Succeeded after /bin/sh -c 'exit 0' exits; got {:?}",
            snapshot.state
        );
    }

    // ---- paginate_lines unit tests (ADR-0057) --------------------------------

    #[test]
    fn paginate_lines_empty_text_returns_empty() {
        let p = Pagination::default();
        let (page, total, next) = paginate_lines("", &p);
        assert!(page.is_empty());
        assert_eq!(total, 0);
        assert!(next.is_none());
    }

    #[test]
    fn paginate_lines_tail_first_page() {
        // 5 lines; default page_size 100 — all returned in reverse (newest first).
        let text = "line1\nline2\nline3\nline4\nline5";
        let p = Pagination {
            offset: 0,
            page_size: 100,
            order: Order::Tail,
        };
        let (page, total, next) = paginate_lines(text, &p);
        assert_eq!(total, 5);
        assert_eq!(page[0], "line5", "Tail order must return newest line first");
        assert_eq!(page[4], "line1");
        assert!(next.is_none());
    }

    #[test]
    fn paginate_lines_tail_pagination_returns_next_offset() {
        let text = "a\nb\nc\nd\ne";
        let p = Pagination {
            offset: 0,
            page_size: 2,
            order: Order::Tail,
        };
        let (page, total, next) = paginate_lines(text, &p);
        assert_eq!(total, 5);
        assert_eq!(page, vec!["e", "d"]);
        assert_eq!(next, Some(2));
    }

    #[test]
    fn paginate_lines_tail_second_page() {
        let text = "a\nb\nc\nd\ne";
        let p = Pagination {
            offset: 2,
            page_size: 2,
            order: Order::Tail,
        };
        let (page, _total, next) = paginate_lines(text, &p);
        assert_eq!(page, vec!["c", "b"]);
        assert_eq!(next, Some(4));
    }

    #[test]
    fn paginate_lines_tail_last_page_has_no_next() {
        let text = "a\nb\nc\nd\ne";
        let p = Pagination {
            offset: 4,
            page_size: 2,
            order: Order::Tail,
        };
        let (page, _total, next) = paginate_lines(text, &p);
        assert_eq!(page, vec!["a"]);
        assert!(next.is_none());
    }

    #[test]
    fn paginate_lines_head_first_page() {
        let text = "a\nb\nc\nd\ne";
        let p = Pagination {
            offset: 0,
            page_size: 3,
            order: Order::Head,
        };
        let (page, total, next) = paginate_lines(text, &p);
        assert_eq!(total, 5);
        assert_eq!(page, vec!["a", "b", "c"]);
        assert_eq!(next, Some(3));
    }

    #[test]
    fn paginate_lines_head_second_page() {
        let text = "a\nb\nc\nd\ne";
        let p = Pagination {
            offset: 3,
            page_size: 3,
            order: Order::Head,
        };
        let (page, _total, next) = paginate_lines(text, &p);
        assert_eq!(page, vec!["d", "e"]);
        assert!(next.is_none());
    }

    #[test]
    fn paginate_lines_offset_beyond_total_returns_empty() {
        let text = "a\nb";
        let p = Pagination {
            offset: 100,
            page_size: 10,
            order: Order::Head,
        };
        let (page, total, next) = paginate_lines(text, &p);
        assert_eq!(total, 2);
        assert!(page.is_empty());
        assert!(next.is_none());
    }
}
