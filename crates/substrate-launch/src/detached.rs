//! Detached supervisor: the `substrate --supervise <stack>` reactor (ADR-0068).
//!
//! This is the Milestone 2 component that lets a Stack survive the MCP server
//! that started it (`on_client_disconnect = detach`). The detached supervisor is
//! the **same binary** re-invoked as `substrate --supervise <stack_id> --profile
//! <path>` (ADR-0068 §"Same binary"); it is not a second artifact and opens no
//! socket. The live MCP server spawns it via `std::process::Command` (a later
//! stage wires that call site); this module is the new process's own behaviour
//! once `--supervise` is present.
//!
//! # Startup sequence
//!
//! 1. [`detach_session`] runs `setsid(2)` **before** the multi-threaded runtime
//!    exists (calling `setsid` once threads are running is unsafe to reason
//!    about); the binary's `main` does this synchronously.
//! 2. [`run_supervisor`] then: opens + security-checks the durable registry
//!    directory ([`crate::supervisor_registry::open_stack_registry`]); redirects
//!    `stdin` to `/dev/null` and `stdout`/`stderr` to a per-Stack `supervisor.log`
//!    (this process is not an MCP transport endpoint, so it must not retain the
//!    inherited STDIO channel — ADR-0005); loads + validates the Profile; spawns
//!    every Service in topological order, binding each child's parent-death signal
//!    to `SIGKILL` so a supervisor death kills the whole Stack (ADR-0068
//!    §"Cross-platform parent-death binding"); persists `supervisor.json`; and
//!    spawns the control-FIFO reader ([`crate::control_fifo::spawn_control_reader`]).
//! 3. The reactor loop ([`DetachedSupervisor::reactor_loop`]) multiplexes the
//!    control FIFO, operator signals, child-exit polling, the orphan-TTL timer,
//!    and the reconcile sweep over one `tokio::select! { biased; .. }`.
//!
//! # Child-exit detection: polling, by design
//!
//! `substrate-launch` only ever sees children through the injected
//! [`SubprocessPort`] trait (hexagonal layering, ADR-0022). That trait exposes no
//! way to *await* a live exit event — only [`SubprocessPort::list`] snapshots
//! current state. So child-exit detection is a periodic poll that diffs observed
//! states against the in-memory child map, exactly the periodic-timer
//! reconciliation ADR-0068 §"The monitor (reconcile sweep)" describes; this is
//! faithful to the spec, not a shortcut. The per-Service restart policy itself is
//! owned by the subprocess BC (ADR-0056): it is threaded into each
//! [`SubprocessRequest`] at spawn time, so the reactor records exits and never
//! re-spawns across the BC boundary.
//!
//! References: ADR-0033, ADR-0053, ADR-0056, ADR-0063, ADR-0065, ADR-0068.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::{Interval, MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use substrate_domain::launch::errors::LaunchError;
use substrate_domain::launch::profile::{LaunchProfile, ServiceName};
use substrate_domain::launch::stack::{StackChild, SupervisorRegistry};
use substrate_domain::launch::state::DisconnectPolicy;
use substrate_domain::ports::fs_index::CancelSignal;
use substrate_domain::ports::launch::ReloadReport;
use substrate_domain::ports::subprocess::SubprocessPort;
use substrate_domain::subprocess::handle::SubprocessHandle;
use substrate_domain::subprocess::state::SubprocessState;
use substrate_domain::value_objects::pagination::PageSize;
use substrate_domain::value_objects::{ClientId, StackId};

use crate::control_fifo::{ControlFrame, spawn_control_reader};
use crate::dag::reverse_topo;
use crate::pid_probe::read_pid_stat;
use crate::profile_loader::{LoadedProfile, load_untrusted};
use crate::reaper;
use crate::registry::compute_reload_report;
use crate::supervisor::{
    ServiceOutcome, build_request, launch_client_id, outcome_state, spawn_service, stop_service,
    wait_ready,
};
use crate::supervisor_registry::{insecure, open_stack_registry, run_blocking, write_supervisor_registry};

/// Raw POSIX signal bound via `PR_SET_PDEATHSIG` for every spawned child, so the
/// kernel kills the whole Stack if this supervisor dies (ADR-0068
/// §"Cross-platform parent-death binding"). `None` would preserve the ordinary
/// `subprocess.spawn` default of `SIGTERM`; the detached supervisor wants the
/// stronger, uncatchable `SIGKILL`.
const PARENT_DEATH_SIGKILL: i32 = libc::SIGKILL;

/// File name of the per-Stack supervisor log under its registry directory.
const SUPERVISOR_LOG_FILE: &str = "supervisor.log";

/// Cadence of the child-exit poll sweep (the only exit-detection mechanism
/// available through [`SubprocessPort`]; see the module docs).
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Cadence of the orphan-TTL timer tick (ADR-0068 §"Orphan TTL").
const TTL_TICK_INTERVAL: Duration = Duration::from_secs(1);

/// Cadence of the reconcile sweep timer tick (ADR-0068 §"The monitor").
const RECONCILE_SWEEP_INTERVAL: Duration = Duration::from_secs(5);

// ---- CLI parsing -------------------------------------------------------------

/// A parsed `--supervise <stack_id> --profile <path>` invocation.
#[derive(Debug, Clone)]
pub struct SuperviseArgs {
    /// The Stack the detached supervisor owns.
    pub stack_id: StackId,
    /// The `.substrate.toml` Profile pinned for this Stack.
    pub profile_path: PathBuf,
}

/// Detects and parses the `--supervise` invocation from a raw argv slice.
///
/// Returns `Ok(None)` when `--supervise` is absent (the process is an ordinary
/// MCP server), `Ok(Some(_))` when the flag and its operands are present and
/// valid, and `Err` when `--supervise` is present but malformed.
///
/// # Errors
///
/// Returns [`LaunchError::InvalidProfile`] when `--supervise` is present without
/// a valid `<stack_id>` operand or without a `--profile <path>` operand.
pub fn parse_supervise_args(args: &[String]) -> Result<Option<SuperviseArgs>, LaunchError> {
    let Some(idx) = args.iter().position(|a| a == "--supervise") else {
        return Ok(None);
    };
    let stack_id_raw = args.get(idx + 1).ok_or_else(|| LaunchError::InvalidProfile {
        msg: "--supervise requires a <stack_id> argument".to_owned(),
    })?;
    let stack_id = stack_id_raw
        .parse::<StackId>()
        .map_err(|e| LaunchError::InvalidProfile {
            msg: format!("invalid --supervise stack_id '{stack_id_raw}': {e}"),
        })?;
    let profile_path = flag_value(args, "--profile").ok_or_else(|| LaunchError::InvalidProfile {
        msg: "--supervise requires a --profile <path> argument".to_owned(),
    })?;
    Ok(Some(SuperviseArgs {
        stack_id,
        profile_path: PathBuf::from(profile_path),
    }))
}

/// Returns the operand immediately following `flag`, if both are present.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let idx = args.iter().position(|a| a == flag)?;
    args.get(idx + 1).map(String::as_str)
}

/// Detaches the current process into a new session via `setsid(2)`.
///
/// MUST be called synchronously from `main`, before any multi-threaded tokio
/// runtime is built: `setsid` is only well-defined while the process is
/// single-threaded (ADR-0068). A best-effort caller treats failure as
/// non-fatal — a supervisor that could not create a new session still functions,
/// it merely shares the parent's session.
///
/// # Errors
///
/// Returns the OS error from `setsid(2)` (commonly `EPERM` when the caller is
/// already a process-group leader).
pub fn detach_session() -> std::io::Result<()> {
    nix::unistd::setsid().map(|_| ()).map_err(std::io::Error::from)
}

// ---- Reactor entry point -----------------------------------------------------

/// Runs the detached supervisor reactor for `args` to completion.
///
/// The injected `subprocess` port is the exclusive process-management path
/// (hexagonal layering, ADR-0022); the composition root wires it. This function
/// returns `Ok(())` after a clean teardown (control-FIFO `down` or an operator
/// `SIGTERM`/`SIGINT`), or `Err` if startup fails before the reactor begins.
///
/// # Errors
///
/// - [`LaunchError::RegistryInsecure`] — the registry directory or its ancestry
///   is insecure (fatal startup failure per ADR-0068).
/// - [`LaunchError::ProfileNotTrusted`] / [`LaunchError::InvalidProfile`] —
///   the Profile cannot be loaded or is structurally invalid.
/// - [`LaunchError::SpawnFailed`] — a Service could not be spawned during
///   bring-up.
pub async fn run_supervisor(
    args: SuperviseArgs,
    subprocess: Arc<dyn SubprocessPort>,
) -> Result<(), LaunchError> {
    let (supervisor, control_rx) = DetachedSupervisor::bootstrap(args, subprocess).await?;
    supervisor.reactor_loop(control_rx).await
}

// ---- Cancellation adapter ----------------------------------------------------

/// A [`CancelSignal`] backed by a [`CancellationToken`].
///
/// Keeps `substrate-domain` free of `tokio-util` (ADR-0003): the domain trait is
/// implemented here in the adapter so [`spawn_service`] / [`wait_ready`] can be
/// driven with the supervisor's root token.
struct TokenCancel(CancellationToken);

#[async_trait]
impl CancelSignal for TokenCancel {
    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
    async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}

// ---- In-memory child record --------------------------------------------------

/// One supervised child's live bookkeeping, mirrored to `supervisor.json`.
struct ChildRecord {
    /// The subprocess job backing this Service.
    job_id: substrate_domain::value_objects::JobId,
    /// OS process id assigned at spawn.
    pid: i32,
    /// Process-group id (`== pid` after `setsid`), used for cascade reap.
    pgid: i32,
    /// Supervisor-observed spawn time, seconds since the Unix epoch.
    start_epoch: u64,
    /// Last observed lifecycle state, diffed by the child-exit poll.
    state: SubprocessState,
}

// ---- Supervisor actor --------------------------------------------------------

/// The detached supervisor's owned reactor state.
struct DetachedSupervisor {
    stack_id: StackId,
    profile_path: PathBuf,
    default_cwd: PathBuf,
    profile: LaunchProfile,
    config_hash: String,
    policy: DisconnectPolicy,
    stack_dir: PathBuf,
    subprocess: Arc<dyn SubprocessPort>,
    client_id: ClientId,
    children: BTreeMap<ServiceName, ChildRecord>,
    supervisor_pid: i32,
    start_epoch: u64,
    /// Epoch seconds of the last observed client activity (any inbound
    /// [`ControlFrame`]), driving the orphan-TTL timer (ADR-0068 §"Orphan TTL").
    last_activity_epoch: u64,
    cancel: CancellationToken,
}

impl DetachedSupervisor {
    /// Initializes the registry, redirects STDIO, brings up the Stack, and spawns
    /// the control-FIFO reader; returns the actor plus its control receiver.
    async fn bootstrap(
        args: SuperviseArgs,
        subprocess: Arc<dyn SubprocessPort>,
    ) -> Result<(Self, mpsc::Receiver<ControlFrame>), LaunchError> {
        let stack_dir = open_stack_registry(&args.stack_id).await?;
        if let Err(error) = redirect_stdio_to_log(&stack_dir).await {
            tracing::warn!(%error, "supervise: stdio redirect failed; using inherited fds");
        }
        let loaded = load_untrusted(&args.profile_path).await?;
        loaded.profile.validate()?;
        let mut supervisor = Self::assemble(args, subprocess, stack_dir.clone(), loaded)?;
        supervisor.start_epoch = probe_start_time(supervisor.supervisor_pid).await;
        supervisor.spawn_all().await?;
        supervisor.flush_registry().await;
        let control_rx = spawn_control_reader(stack_dir);
        Ok((supervisor, control_rx))
    }

    /// Builds the actor struct from the parsed args and the loaded Profile.
    fn assemble(
        args: SuperviseArgs,
        subprocess: Arc<dyn SubprocessPort>,
        stack_dir: PathBuf,
        loaded: LoadedProfile,
    ) -> Result<Self, LaunchError> {
        let default_cwd = args
            .profile_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| stack_dir.clone(), Path::to_path_buf);
        Ok(Self {
            stack_id: args.stack_id,
            profile_path: args.profile_path,
            default_cwd,
            profile: loaded.profile,
            config_hash: loaded.config_hash,
            policy: DisconnectPolicy::Detach,
            stack_dir,
            subprocess,
            client_id: launch_client_id()?,
            children: BTreeMap::new(),
            supervisor_pid: i32::try_from(std::process::id()).unwrap_or(i32::MAX),
            start_epoch: now_epoch_secs(),
            last_activity_epoch: now_epoch_secs(),
            cancel: CancellationToken::new(),
        })
    }

    /// Spawns every Service in topological order, binding `SIGKILL` parent-death.
    async fn spawn_all(&mut self) -> Result<(), LaunchError> {
        let cancel = TokenCancel(self.cancel.clone());
        for name in self.profile.topological_order()? {
            let Some(service) = self.profile.services.get(&name).cloned() else {
                continue;
            };
            let mut request = build_request(&name, &service, &self.default_cwd)?;
            request.parent_death_signal = Some(PARENT_DEATH_SIGKILL);
            let handle = spawn_service(self.subprocess.as_ref(), request, &cancel).await?;
            let outcome =
                wait_ready(self.subprocess.as_ref(), &self.client_id, &handle.job_id, &cancel)
                    .await?;
            self.record_child(name, &handle, outcome).await;
        }
        Ok(())
    }

    /// Records a freshly spawned child in the in-memory map.
    ///
    /// `start_epoch` is the child's kernel start-time (the PID-recycle guard the
    /// reaper compares against, ADR-0068), read on the blocking pool right after
    /// spawn; it falls back to the wall clock only when the platform read fails.
    async fn record_child(&mut self, name: ServiceName, handle: &SubprocessHandle, outcome: ServiceOutcome) {
        let pid = handle.process_group.pid();
        let start_epoch = probe_start_time(pid).await;
        self.children.insert(
            name,
            ChildRecord {
                job_id: handle.job_id.clone(),
                pid,
                pgid: handle.process_group.pgid(),
                start_epoch,
                state: outcome_state(outcome),
            },
        );
    }

    /// The single `tokio::select! { biased; .. }` reactor (ADR-0068, ADR-0003).
    ///
    /// `biased` puts the real-work arms (control FIFO, operator signals,
    /// child-exit poll) ahead of the periodic TTL and reconcile timers. The
    /// control receiver, signal streams, and timers are loop-local — never actor
    /// fields — so no two `select!` arm futures borrow `self` simultaneously.
    async fn reactor_loop(mut self, mut control_rx: mpsc::Receiver<ControlFrame>) -> Result<(), LaunchError> {
        use tokio::signal::unix::{SignalKind, signal};

        let mut control_open = true;
        let mut sigterm = signal(SignalKind::terminate()).ok();
        let mut sigint = signal(SignalKind::interrupt()).ok();
        let mut child_poll = make_interval(CHILD_POLL_INTERVAL);
        let mut ttl_tick = make_interval(TTL_TICK_INTERVAL);
        let mut sweep_tick = make_interval(RECONCILE_SWEEP_INTERVAL);

        loop {
            tokio::select! {
                biased;
                maybe = control_rx.recv(), if control_open => {
                    if self.handle_control(maybe, &mut control_open).await {
                        return Ok(());
                    }
                }
                () = recv_signal(sigterm.as_mut()) => {
                    self.teardown().await;
                    return Ok(());
                }
                () = recv_signal(sigint.as_mut()) => {
                    self.teardown().await;
                    return Ok(());
                }
                _ = child_poll.tick() => self.poll_children().await,
                _ = ttl_tick.tick() => {
                    if check_orphan_ttl(&self).await {
                        return Ok(());
                    }
                }
                _ = sweep_tick.tick() => self.run_reconcile_sweep().await,
            }
        }
    }

    /// Applies one inbound control frame; returns `true` when the supervisor must
    /// exit (a `down` command). A closed channel disables the control arm.
    async fn handle_control(&mut self, frame: Option<ControlFrame>, control_open: &mut bool) -> bool {
        match frame {
            Some(ControlFrame::Down { .. }) => {
                self.last_activity_epoch = now_epoch_secs();
                self.teardown().await;
                true
            },
            Some(ControlFrame::Reload { profile_path, .. }) => {
                self.last_activity_epoch = now_epoch_secs();
                self.handle_reload(profile_path).await;
                false
            },
            None => {
                *control_open = false;
                false
            },
        }
    }

    /// Drains and cascade-kills every child, then clears the registry entry.
    ///
    /// Reverse-topological stop through [`stop_service`] — the same `cancel(force
    /// = false)` SIGTERM-then-drain-then-SIGKILL cascade the in-process teardown
    /// (`registry.rs::down`) uses, so the grace window is the subprocess BC's
    /// configured `shutdown_drain_secs`, not a new magic number.
    async fn teardown(&self) {
        self.cancel.cancel();
        for name in reverse_topo(&self.profile).unwrap_or_default() {
            if let Some(child) = self.children.get(&name) {
                stop_service(self.subprocess.as_ref(), &child.job_id).await;
            }
        }
        self.clear_registry().await;
    }

    /// Best-effort removal of the durable registry directory on teardown.
    async fn clear_registry(&self) {
        if let Err(error) = tokio::fs::remove_dir_all(&self.stack_dir).await {
            tracing::warn!(%error, "supervise: failed to clear registry directory");
        }
    }

    /// Polls [`SubprocessPort::list`] once and reconciles observed child states.
    async fn poll_children(&mut self) {
        let snapshot = match self
            .subprocess
            .list(&self.client_id, None, None, PageSize::DEFAULT)
            .await
        {
            Ok((handles, _)) => handles,
            Err(error) => {
                tracing::warn!(%error, "supervise: child poll list failed");
                return;
            },
        };
        let mut changed = false;
        for (name, record) in &mut self.children {
            if diff_child(name, record, &snapshot) {
                changed = true;
            }
        }
        if changed {
            self.flush_registry().await;
        }
    }

    /// Reloads the Stack from `profile_path` (or the pinned path) and reconciles.
    async fn handle_reload(&mut self, profile_path: Option<String>) {
        if let Err(error) = self.reload_inner(profile_path).await {
            tracing::warn!(%error, "supervise: reload failed");
        }
    }

    /// Computes the reload diff and applies the minimal stop/start closure.
    async fn reload_inner(&mut self, profile_path: Option<String>) -> Result<(), LaunchError> {
        let path = profile_path.map_or_else(|| self.profile_path.clone(), PathBuf::from);
        let loaded = load_untrusted(&path).await?;
        loaded.profile.validate()?;
        let new_profile = loaded.profile;
        new_profile.topological_order()?;
        let report = compute_reload_report(&self.profile, &new_profile);
        self.apply_reload_stop(&report).await;
        self.apply_reload_start(&new_profile, &report).await?;
        self.profile = new_profile;
        self.profile_path = path;
        self.config_hash = loaded.config_hash;
        self.flush_registry().await;
        Ok(())
    }

    /// Stops removed and restarted Services, dependents-first (reverse topo).
    async fn apply_reload_stop(&mut self, report: &ReloadReport) {
        let stop: BTreeSet<ServiceName> = report
            .removed
            .iter()
            .chain(&report.restarted)
            .cloned()
            .collect();
        for name in reverse_topo(&self.profile).unwrap_or_default() {
            if !stop.contains(&name) {
                continue;
            }
            if let Some(child) = self.children.get(&name) {
                stop_service(self.subprocess.as_ref(), &child.job_id).await;
            }
            self.children.remove(&name);
        }
    }

    /// Spawns added and restarted Services, dependency-first (forward topo).
    async fn apply_reload_start(
        &mut self,
        new_profile: &LaunchProfile,
        report: &ReloadReport,
    ) -> Result<(), LaunchError> {
        let start: BTreeSet<ServiceName> = report
            .added
            .iter()
            .chain(&report.restarted)
            .cloned()
            .collect();
        let cancel = TokenCancel(self.cancel.clone());
        for name in new_profile.topological_order()? {
            if !start.contains(&name) {
                continue;
            }
            let Some(service) = new_profile.services.get(&name) else {
                continue;
            };
            let mut request = build_request(&name, service, &self.default_cwd)?;
            request.parent_death_signal = Some(PARENT_DEATH_SIGKILL);
            let handle = spawn_service(self.subprocess.as_ref(), request, &cancel).await?;
            let outcome =
                wait_ready(self.subprocess.as_ref(), &self.client_id, &handle.job_id, &cancel)
                    .await?;
            self.record_child(name, &handle, outcome).await;
        }
        Ok(())
    }

    /// Atomically rewrites `supervisor.json` from the current in-memory state.
    async fn flush_registry(&self) {
        let registry = self.registry_snapshot();
        if let Err(error) = write_supervisor_registry(&self.stack_dir, &registry).await {
            tracing::warn!(%error, "supervise: failed to flush supervisor.json");
        }
    }

    /// Runs the reaper's adopt-or-reap reconcile pass over the stacks root
    /// (ADR-0068 §"The monitor (reconcile sweep)").
    ///
    /// Delegates to the public [`reaper::reconcile_sweep`], which short-circuits
    /// this Stack as a re-attach (this supervisor is live and owns it) and only
    /// adopts/reaps orphans left by *other* supervisors that have since died.
    async fn run_reconcile_sweep(&self) {
        let Some(stacks_root) = self.stack_dir.parent() else {
            return;
        };
        match reaper::reconcile_sweep(stacks_root).await {
            Ok(report) if !report.is_empty() => tracing::info!(
                reattached = report.reattached.len(),
                adopted = report.adopted.len(),
                reaped = report.reaped.len(),
                recycled = report.recycled.len(),
                "supervise: reconcile sweep applied"
            ),
            Ok(_) => {},
            Err(error) => tracing::warn!(%error, "supervise: reconcile sweep failed"),
        }
    }

    /// Builds the durable [`SupervisorRegistry`] document from live state.
    fn registry_snapshot(&self) -> SupervisorRegistry {
        SupervisorRegistry {
            supervisor_pid: self.supervisor_pid,
            start_epoch: self.start_epoch,
            policy: self.policy,
            config_hash: self.config_hash.clone(),
            children: self
                .children
                .iter()
                .map(|(name, record)| StackChild {
                    name: name.clone(),
                    pid: record.pid,
                    pgid: record.pgid,
                    start_epoch: record.start_epoch,
                })
                .collect(),
        }
    }
}

// ---- Orphan-TTL hook ---------------------------------------------------------

/// Brings the detached Stack down once it has had no client activity for
/// `orphan_ttl_secs`, recording `SUBSTRATE_LAUNCH_STACK_TTL_EXPIRED` (ADR-0068
/// §"Orphan TTL").
///
/// Returns `true` when the TTL fired and the supervisor must exit — teardown
/// (the same drain + cascade-kill + registry-clear path SIGTERM uses) has already
/// run, so the caller returns `Ok(())` from the reactor. Returns `false` while
/// the Stack is still within its TTL, when the policy is not `Detach`, or when
/// the TTL is disabled (`orphan_ttl_secs == 0`).
///
/// "Client activity" is any inbound [`ControlFrame`]: the MVP reactor has no
/// explicit client-attach event, so a received control command stands in as
/// evidence of a live client (documented deviation, ADR-0068).
async fn check_orphan_ttl(supervisor: &DetachedSupervisor) -> bool {
    let ttl = supervisor.profile.orphan_ttl_secs;
    if ttl == 0 || supervisor.policy != DisconnectPolicy::Detach {
        return false;
    }
    let idle = now_epoch_secs().saturating_sub(supervisor.last_activity_epoch);
    if idle < u64::from(ttl) {
        return false;
    }
    let error = LaunchError::StackTtlExpired {
        stack_id: supervisor.stack_id.to_crockford(),
    };
    let correlation_id = Uuid::now_v7();
    tracing::info!(
        code = error.code(),
        %correlation_id,
        idle_secs = idle,
        ttl_secs = ttl,
        "supervise: orphan TTL expired; bringing detached stack down"
    );
    supervisor.teardown().await;
    true
}

// ---- Free helpers ------------------------------------------------------------

/// Reads `pid`'s kernel start-time on the blocking pool (zone B per ADR-0003) for
/// the PID-recycle guard, falling back to the wall clock when the read fails.
async fn probe_start_time(pid: i32) -> u64 {
    run_blocking(move || Ok(read_pid_stat(pid)))
        .await
        .ok()
        .flatten()
        .map_or_else(now_epoch_secs, |stat| stat.start_time)
}

/// Builds a periodic [`Interval`] that coalesces missed ticks instead of bursting.
fn make_interval(period: Duration) -> Interval {
    let mut it = interval(period);
    it.set_missed_tick_behavior(MissedTickBehavior::Delay);
    it
}

/// Resolves when `sig` fires, or pends forever when the signal is unregistered.
///
/// A `None` signal (registration failed) degrades that reactor arm to
/// never-ready rather than aborting the supervisor.
async fn recv_signal(sig: Option<&mut tokio::signal::unix::Signal>) {
    match sig {
        Some(s) => {
            s.recv().await;
        },
        None => std::future::pending::<()>().await,
    }
}

/// Diffs one child against the latest [`SubprocessPort::list`] snapshot.
///
/// Returns `true` when the recorded state changed. A transition into a terminal
/// (exit) state is logged; the actual re-spawn for a Service that carries a
/// restart policy is owned by the subprocess BC (ADR-0056), which received that
/// policy in the [`SubprocessRequest`]. A child no longer present in the snapshot
/// is left for the reconcile sweep / reaper-on-boot stage rather than fabricating
/// a terminal state here.
fn diff_child(name: &str, record: &mut ChildRecord, snapshot: &[SubprocessHandle]) -> bool {
    let Some(handle) = snapshot.iter().find(|h| h.job_id == record.job_id) else {
        return false;
    };
    if handle.state == record.state {
        return false;
    }
    if is_exit_state(handle.state) && !is_exit_state(record.state) {
        tracing::info!(
            service = %name,
            state = ?handle.state,
            "supervise: child exited; per-Service restart policy is owned by the subprocess BC (ADR-0056)"
        );
    }
    record.state = handle.state;
    true
}

/// Returns `true` for the terminal states that count as a child exit.
const fn is_exit_state(state: SubprocessState) -> bool {
    matches!(
        state,
        SubprocessState::Failed
            | SubprocessState::Cancelled
            | SubprocessState::Killed
            | SubprocessState::TimedOut
            | SubprocessState::Succeeded
    )
}

/// Redirects `stdin` to `/dev/null` and `stdout`/`stderr` to `supervisor.log`.
///
/// The detached supervisor is not an MCP transport endpoint, so it must not
/// retain the inherited STDIO channel (ADR-0005 stdout sanctity applies to the
/// MCP server, not here). The blocking `open`/`dup2` syscalls run on the blocking
/// pool (async zone B per ADR-0003).
///
/// # Errors
///
/// Returns [`LaunchError::RegistryInsecure`] (the log lives under the registry
/// directory) when `/dev/null` or the log file cannot be opened, or when any
/// `dup2(2)` fails.
async fn redirect_stdio_to_log(stack_dir: &Path) -> Result<(), LaunchError> {
    let log_path = stack_dir.join(SUPERVISOR_LOG_FILE);
    run_blocking(move || redirect_stdio_blocking(&log_path)).await
}

/// Synchronous body of [`redirect_stdio_to_log`] (blocking syscalls).
fn redirect_stdio_blocking(log_path: &Path) -> Result<(), LaunchError> {
    let devnull = std::fs::File::open("/dev/null").map_err(|_| insecure(log_path))?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_err(|_| insecure(log_path))?;
    nix::unistd::dup2_stdin(&devnull).map_err(|_| insecure(log_path))?;
    nix::unistd::dup2_stdout(&log).map_err(|_| insecure(log_path))?;
    nix::unistd::dup2_stderr(&log).map_err(|_| insecure(log_path))?;
    Ok(())
}

/// Returns the current wall-clock time in seconds since the Unix epoch.
fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

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

    use substrate_domain::errors::{SubstrateError, SubstrateResult};
    use substrate_domain::launch::profile::{CommandSpec, DependencyRestartMode, LaunchService, StreamMux};
    use substrate_domain::ports::subprocess::{
        SignalTarget, SubprocessResult, SubprocessSignalName,
    };
    use substrate_domain::subprocess::errors::SubprocessError;
    use substrate_domain::subprocess::pagination::{SubprocessSearchRequest, SubprocessSearchResult};
    use substrate_domain::subprocess::request::SubprocessRequest;
    use substrate_domain::value_objects::{JobId, ProcessGroup};

    use super::*;

    /// A scripted [`SubprocessPort`] double that records spawns + cancels and lets
    /// tests mutate a job's observed state.
    struct FakePort {
        handles: Mutex<HashMap<String, SubprocessHandle>>,
        states: Mutex<HashMap<String, SubprocessState>>,
        spawn_pdeath: Mutex<Vec<Option<i32>>>,
        cancels: Mutex<Vec<String>>,
        counter: Mutex<i32>,
    }

    impl FakePort {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                handles: Mutex::new(HashMap::new()),
                states: Mutex::new(HashMap::new()),
                spawn_pdeath: Mutex::new(Vec::new()),
                cancels: Mutex::new(Vec::new()),
                counter: Mutex::new(1000),
            })
        }

        fn set_state(&self, job_id: &JobId, state: SubprocessState) {
            let key = job_id.to_crockford();
            self.states.lock().unwrap().insert(key.clone(), state);
            if let Some(h) = self.handles.lock().unwrap().get_mut(&key) {
                h.state = state;
            }
        }

        fn pdeath_signals(&self) -> Vec<Option<i32>> {
            self.spawn_pdeath.lock().unwrap().clone()
        }

        fn cancels(&self) -> Vec<String> {
            self.cancels.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SubprocessPort for FakePort {
        async fn spawn(
            &self,
            req: SubprocessRequest,
            _cancel: &dyn CancelSignal,
        ) -> Result<SubprocessHandle, SubprocessError> {
            self.spawn_pdeath.lock().unwrap().push(req.parent_death_signal);
            let pid = {
                let mut c = self.counter.lock().unwrap();
                *c += 1;
                *c
            };
            let handle = SubprocessHandle {
                job_id: JobId::now_v7(),
                process_group: ProcessGroup::new(pid, pid).expect("valid pid"),
                state: SubprocessState::Ready,
                started_at: OffsetDateTime::now_utc(),
                exit_code: None,
                stream_chunks_dropped: 0,
                tmp_files: Vec::new(),
            };
            self.handles
                .lock()
                .unwrap()
                .insert(handle.job_id.to_crockford(), handle.clone());
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
            self.cancels.lock().unwrap().push(job_id.to_crockford());
            if let Some(h) = self.handles.lock().unwrap().get_mut(&job_id.to_crockford()) {
                h.state = SubprocessState::Cancelled;
            }
            Ok(SubprocessState::Cancelled)
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

    fn service(cmd: &[&str], deps: &[&str]) -> LaunchService {
        LaunchService {
            command: CommandSpec::Argv(cmd.iter().map(|s| (*s).to_owned()).collect()),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            depends_on: deps.iter().map(|s| (*s).to_owned()).collect(),
            required: true,
            restart_policy: None,
            health_probe: None,
            on_dependency_restart: DependencyRestartMode::Restart,
            error_patterns: Vec::new(),
            redact: Vec::new(),
            streams: StreamMux::Multiplexed,
        }
    }

    fn two_tier_profile() -> LaunchProfile {
        let mut services = BTreeMap::new();
        services.insert("db".to_owned(), service(&["db"], &[]));
        services.insert("api".to_owned(), service(&["api"], &["db"]));
        LaunchProfile {
            version: 1,
            on_client_disconnect: DisconnectPolicy::Detach,
            orphan_ttl_secs: 3600,
            services,
        }
    }

    fn supervisor(port: Arc<FakePort>, stack_dir: &Path) -> DetachedSupervisor {
        DetachedSupervisor {
            stack_id: StackId::now_v7(),
            profile_path: stack_dir.join(".substrate.toml"),
            default_cwd: stack_dir.to_path_buf(),
            profile: two_tier_profile(),
            config_hash: "blake3:test".to_owned(),
            policy: DisconnectPolicy::Detach,
            stack_dir: stack_dir.to_path_buf(),
            subprocess: port as Arc<dyn SubprocessPort>,
            client_id: launch_client_id().expect("client id"),
            children: BTreeMap::new(),
            supervisor_pid: 4242,
            start_epoch: 1_770_000_000,
            last_activity_epoch: now_epoch_secs(),
            cancel: CancellationToken::new(),
        }
    }

    #[test]
    fn parse_returns_none_without_flag() {
        let args = vec!["substrate".to_owned(), "--config".to_owned(), "/x".to_owned()];
        assert!(parse_supervise_args(&args).expect("parse").is_none());
    }

    #[test]
    fn parse_extracts_stack_id_and_profile() {
        let id = StackId::now_v7().to_crockford();
        let args = vec![
            "substrate".to_owned(),
            "--supervise".to_owned(),
            id.clone(),
            "--profile".to_owned(),
            "/proj/.substrate.toml".to_owned(),
        ];
        let parsed = parse_supervise_args(&args).expect("parse").expect("present");
        assert_eq!(parsed.stack_id.to_crockford(), id);
        assert_eq!(parsed.profile_path, PathBuf::from("/proj/.substrate.toml"));
    }

    #[test]
    fn parse_rejects_missing_profile() {
        let id = StackId::now_v7().to_crockford();
        let args = vec!["substrate".to_owned(), "--supervise".to_owned(), id];
        assert!(matches!(
            parse_supervise_args(&args),
            Err(LaunchError::InvalidProfile { .. })
        ));
    }

    #[test]
    fn parse_rejects_bad_stack_id() {
        let args = vec![
            "substrate".to_owned(),
            "--supervise".to_owned(),
            "not-a-stack-id".to_owned(),
            "--profile".to_owned(),
            "/p".to_owned(),
        ];
        assert!(matches!(
            parse_supervise_args(&args),
            Err(LaunchError::InvalidProfile { .. })
        ));
    }

    #[test]
    fn exit_states_are_terminal() {
        assert!(is_exit_state(SubprocessState::Failed));
        assert!(is_exit_state(SubprocessState::Succeeded));
        assert!(is_exit_state(SubprocessState::Killed));
        assert!(!is_exit_state(SubprocessState::Running));
        assert!(!is_exit_state(SubprocessState::Ready));
    }

    #[tokio::test]
    async fn spawn_all_binds_sigkill_and_records_children() {
        let dir = TempDir::new().expect("tempdir");
        let port = FakePort::new();
        let mut sup = supervisor(port.clone(), dir.path());

        sup.spawn_all().await.expect("spawn_all");

        assert_eq!(sup.children.len(), 2, "both services recorded");
        let pdeaths = port.pdeath_signals();
        assert_eq!(pdeaths.len(), 2);
        assert!(
            pdeaths.iter().all(|p| *p == Some(libc::SIGKILL)),
            "every spawned child binds SIGKILL parent-death; got {pdeaths:?}"
        );
        assert!(sup.children.get("db").is_some_and(|c| c.pid >= 2));
    }

    #[tokio::test]
    async fn teardown_cascade_stops_and_clears_registry() {
        let dir = TempDir::new().expect("tempdir");
        let port = FakePort::new();
        let mut sup = supervisor(port.clone(), dir.path());
        sup.spawn_all().await.expect("spawn_all");
        sup.flush_registry().await;
        assert!(dir.path().join("supervisor.json").exists(), "registry written");

        sup.teardown().await;

        assert_eq!(port.cancels().len(), 2, "every child cancelled on teardown");
        assert!(!dir.path().exists(), "registry directory cleared on teardown");
    }

    #[tokio::test]
    async fn poll_children_records_terminal_exit() {
        let dir = TempDir::new().expect("tempdir");
        let port = FakePort::new();
        let mut sup = supervisor(port.clone(), dir.path());
        sup.spawn_all().await.expect("spawn_all");

        let api_job = sup.children.get("api").expect("api child").job_id.clone();
        port.set_state(&api_job, SubprocessState::Failed);

        sup.poll_children().await;

        assert_eq!(
            sup.children.get("api").map(|c| c.state),
            Some(SubprocessState::Failed),
            "child-exit poll records the terminal transition"
        );
    }

    #[tokio::test]
    async fn orphan_ttl_does_not_fire_within_window() {
        let dir = TempDir::new().expect("tempdir");
        let port = FakePort::new();
        let mut sup = supervisor(port, dir.path());
        sup.profile.orphan_ttl_secs = 3600;
        sup.last_activity_epoch = now_epoch_secs();

        assert!(!check_orphan_ttl(&sup).await, "fresh activity keeps the stack up");
    }

    #[tokio::test]
    async fn orphan_ttl_ignores_non_detach_policy() {
        let dir = TempDir::new().expect("tempdir");
        let port = FakePort::new();
        let mut sup = supervisor(port, dir.path());
        sup.policy = DisconnectPolicy::Shutdown;
        sup.profile.orphan_ttl_secs = 1;
        sup.last_activity_epoch = 0;

        assert!(
            !check_orphan_ttl(&sup).await,
            "TTL only governs detached stacks; shutdown-policy stacks are never auto-downed here"
        );
    }

    #[tokio::test]
    async fn orphan_ttl_fires_and_tears_down_after_window() {
        let dir = TempDir::new().expect("tempdir");
        let port = FakePort::new();
        let mut sup = supervisor(port, dir.path());
        sup.spawn_all().await.expect("spawn_all");
        sup.flush_registry().await;
        sup.profile.orphan_ttl_secs = 1;
        sup.last_activity_epoch = 0;

        assert!(check_orphan_ttl(&sup).await, "an idle detached stack past its TTL is brought down");
        assert!(!dir.path().exists(), "TTL teardown clears the registry directory");
    }

    #[tokio::test]
    async fn orphan_ttl_disabled_when_zero() {
        let dir = TempDir::new().expect("tempdir");
        let port = FakePort::new();
        let mut sup = supervisor(port, dir.path());
        sup.profile.orphan_ttl_secs = 0;
        sup.last_activity_epoch = 0;

        assert!(!check_orphan_ttl(&sup).await, "orphan_ttl_secs == 0 disables detached survival timeout");
    }

    #[tokio::test]
    async fn registry_snapshot_reflects_live_children() {
        let dir = TempDir::new().expect("tempdir");
        let port = FakePort::new();
        let mut sup = supervisor(port.clone(), dir.path());
        sup.spawn_all().await.expect("spawn_all");

        let snapshot = sup.registry_snapshot();
        assert_eq!(snapshot.supervisor_pid, 4242);
        assert_eq!(snapshot.policy, DisconnectPolicy::Detach);
        assert_eq!(snapshot.config_hash, "blake3:test");
        assert_eq!(snapshot.children.len(), 2);
        assert!(snapshot.children.iter().all(|c| c.pid >= 2 && c.pgid >= 2));
    }
}
