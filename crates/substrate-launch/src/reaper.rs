//! Reaper-on-boot: the adopt-or-reap reconcile pass over the durable registry
//! (ADR-0068 §"Reaper on boot: adopt or reap").
//!
//! On every supervisor and MCP-server start, a reconcile pass walks
//! `${XDG_STATE_HOME:-~/.local/state}/substrate/stacks/<id>/supervisor.json`
//! (Stage 1's [`crate::supervisor_registry`]). For each recorded Stack:
//!
//! - If its supervisor is still live (its recorded pid is alive *and* its
//!   recorded start-time still matches, guarding against supervisor-pid reuse),
//!   the Stack is actively managed: every child is re-attached and the registry
//!   is left untouched. This also makes the running supervisor's own periodic
//!   sweep a no-op against its own Stack.
//! - Otherwise the supervisor is gone, so every recorded child is examined:
//!   1. Its live start-time is re-read and compared to the recorded value. A
//!      mismatch means the kernel recycled the pid onto a stranger: the entry is
//!      cleared with **no signal** and [`LaunchError::ChildPidRecycled`] recorded.
//!   2. On a start-time match, an orphan under a `detach` Stack is **adopted**
//!      (entry kept, [`LaunchError::OrphanAdopted`] recorded); an orphan under a
//!      `shutdown` Stack — or a stale entry whose process is already gone — is
//!      **reaped**: `killpg(pgid, SIGTERM)` then `SIGKILL` after the drain
//!      window, entry cleared, [`LaunchError::OrphanReaped`] recorded. Before any
//!      `killpg`, the pgid leader's start-time is re-verified, so a recycled
//!      process group is never signalled.
//!
//! After processing, a Stack with surviving children rewrites `supervisor.json`;
//! a Stack with none has its registry directory removed.
//!
//! This function is public and reusable: the detached supervisor's reactor calls
//! it on its periodic sweep, and a fresh MCP server calls it once at startup
//! (wired by a later Integration stage). It is idempotent and safe to call
//! repeatedly and concurrently across processes.
//!
//! References: ADR-0033, ADR-0053, ADR-0068.

use std::path::{Path, PathBuf};
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use uuid::Uuid;

use substrate_domain::launch::errors::LaunchError;
use substrate_domain::launch::stack::{StackChild, SupervisorRegistry};
use substrate_domain::launch::state::DisconnectPolicy;

use crate::pid_probe::{PidStat, read_pid_stat};
use crate::supervisor_registry::{read_supervisor_registry, run_blocking, write_supervisor_registry};

/// File name of the durable per-Stack registry document under each `stacks/<id>/`.
const SUPERVISOR_FILE: &str = "supervisor.json";

/// Drain window between `SIGTERM` and `SIGKILL` when reaping an orphan group.
///
/// Matches the in-process teardown grace window: the subprocess BC's
/// `shutdown_drain_secs` defaults to 5 seconds (the same value the in-process
/// `registry.rs::down` cascade relies on), so the reaper applies no new magic
/// number.
const REAP_DRAIN: Duration = Duration::from_secs(5);

/// The actions a single reconcile sweep took, returned for logging/observability.
///
/// Each entry is a `"<stack_id>/<service>"` label so a multi-Stack sweep stays
/// legible. The Integration stage that wires the MCP-server startup call site
/// consumes this to surface what the boot reaper did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Children left running under a live supervisor (registry untouched).
    pub reattached: Vec<String>,
    /// Orphans adopted under a `detach`-policy Stack.
    pub adopted: Vec<String>,
    /// Orphans (or stale entries) reaped and cleared.
    pub reaped: Vec<String>,
    /// Entries cleared because their pid was recycled (no signal sent).
    pub recycled: Vec<String>,
}

impl ReconcileReport {
    /// Returns `true` when the sweep took no action at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.reattached.is_empty()
            && self.adopted.is_empty()
            && self.reaped.is_empty()
            && self.recycled.is_empty()
    }
}

/// Walks every Stack registry under `stacks_root` and applies the adopt-or-reap
/// decision tree (ADR-0068 §"Reaper on boot").
///
/// `stacks_root` is the `${XDG_STATE_HOME:-~/.local/state}/substrate/stacks`
/// directory; see [`crate::supervisor_registry::launch_stacks_root`]. A registry
/// that cannot be read is skipped (logged), so one corrupt Stack never aborts the
/// whole sweep.
///
/// # Errors
///
/// Returns [`LaunchError::RegistryInsecure`] only when the blocking directory
/// listing itself fails to dispatch; per-Stack failures are logged and skipped.
pub async fn reconcile_sweep(stacks_root: &Path) -> Result<ReconcileReport, LaunchError> {
    let mut report = ReconcileReport::default();
    for stack_dir in list_stack_dirs(stacks_root).await? {
        if let Err(error) = reconcile_stack(&stack_dir, &mut report).await {
            tracing::warn!(
                %error,
                dir = %stack_dir.display(),
                "reaper: skipping unreadable stack registry"
            );
        }
    }
    Ok(report)
}

/// Lists the `<stacks_root>/<id>/` directories that hold a `supervisor.json`.
async fn list_stack_dirs(stacks_root: &Path) -> Result<Vec<PathBuf>, LaunchError> {
    let root = stacks_root.to_path_buf();
    run_blocking(move || Ok(list_stack_dirs_blocking(&root))).await
}

/// Synchronous body of [`list_stack_dirs`] (blocking `read_dir`, zone B).
fn list_stack_dirs_blocking(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join(SUPERVISOR_FILE).is_file())
        .collect()
}

/// Reconciles one Stack registry, appending its actions to `report`.
async fn reconcile_stack(stack_dir: &Path, report: &mut ReconcileReport) -> Result<(), LaunchError> {
    let mut registry = read_supervisor_registry(stack_dir).await?;
    let label = stack_label(stack_dir);

    if supervisor_alive(&registry).await {
        for child in &registry.children {
            report.reattached.push(format!("{label}/{}", child.name));
        }
        return Ok(());
    }

    let supervisor_pid = registry.supervisor_pid;
    let policy = registry.policy;
    let children = std::mem::take(&mut registry.children);
    let mut survivors: Vec<StackChild> = Vec::new();
    for child in children {
        apply_verdict(&child, policy, supervisor_pid, &label, report, &mut survivors).await;
    }
    registry.children = survivors;
    finalize(stack_dir, &registry).await
}

/// Adjudicates one child and records the chosen action into `report`/`survivors`.
async fn apply_verdict(
    child: &StackChild,
    policy: DisconnectPolicy,
    supervisor_pid: i32,
    label: &str,
    report: &mut ReconcileReport,
    survivors: &mut Vec<StackChild>,
) {
    let entry = format!("{label}/{}", child.name);
    match decide(probe(child.pid).await, child.start_epoch, policy, supervisor_pid) {
        Verdict::Reattach => {
            report.reattached.push(entry);
            survivors.push(child.clone());
        },
        Verdict::Adopt => {
            record(&LaunchError::OrphanAdopted { name: child.name.clone() });
            report.adopted.push(entry);
            survivors.push(child.clone());
        },
        Verdict::Reap { signal } => {
            if signal {
                reap_group(child).await;
            }
            record(&LaunchError::OrphanReaped { name: child.name.clone() });
            report.reaped.push(entry);
        },
        Verdict::Recycled => {
            record(&LaunchError::ChildPidRecycled {
                name: child.name.clone(),
                pid: child.pid,
            });
            report.recycled.push(entry);
        },
    }
}

/// The reconcile outcome for one recorded child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Keep the entry: the child is still parented to its (named) supervisor.
    Reattach,
    /// Keep the entry under a `detach` Stack and record the adoption.
    Adopt,
    /// Clear the entry, optionally signalling the still-live process group.
    Reap {
        /// `true` when a live group must be `killpg`'d; `false` for a stale entry.
        signal: bool,
    },
    /// Clear the entry without signalling: the pid was recycled to a stranger.
    Recycled,
}

/// Pure adopt-or-reap decision from a child's probe result (no I/O), so the full
/// tree is unit-testable without live processes or signals.
const fn decide(
    probe: Option<PidStat>,
    recorded_start: u64,
    policy: DisconnectPolicy,
    supervisor_pid: i32,
) -> Verdict {
    let Some(stat) = probe else {
        // No live process holds this pid: a stale entry. Nothing to signal.
        return Verdict::Reap { signal: false };
    };
    if stat.start_time != recorded_start {
        return Verdict::Recycled;
    }
    if stat.ppid == supervisor_pid {
        // The supervisor was judged gone at the Stack level, yet this child still
        // names it as parent: a transient race during supervisor exit. Leave the
        // child for the next sweep rather than reaping a possibly-attached child.
        return Verdict::Reattach;
    }
    match policy {
        DisconnectPolicy::Detach => Verdict::Adopt,
        DisconnectPolicy::Shutdown => Verdict::Reap { signal: true },
    }
}

/// Returns `true` when the Stack's recorded supervisor is still the live process
/// it claims to be (pid alive **and** start-time matches, guarding supervisor-pid
/// reuse).
async fn supervisor_alive(registry: &SupervisorRegistry) -> bool {
    probe(registry.supervisor_pid)
        .await
        .is_some_and(|stat| stat.start_time == registry.start_epoch)
}

/// `killpg(SIGTERM)`, drain, then `killpg(SIGKILL)` for a still-live orphan group.
///
/// The pgid leader's start-time is re-verified before each signal, so a process
/// group whose leader exited (and whose pgid may have been recycled) is never
/// signalled. For supervised children the leader is the child itself
/// (`pgid == pid` after `setsid`), so its recorded start-time is
/// `child.start_epoch`.
async fn reap_group(child: &StackChild) {
    if !leader_matches(child.pgid, child.start_epoch).await {
        return;
    }
    send_group_signal(child.pgid, Signal::SIGTERM);
    tokio::time::sleep(REAP_DRAIN).await;
    if leader_matches(child.pgid, child.start_epoch).await {
        send_group_signal(child.pgid, Signal::SIGKILL);
    }
}

/// Returns `true` when `pgid`'s leader is live with the recorded start-time.
async fn leader_matches(pgid: i32, recorded_start: u64) -> bool {
    probe(pgid).await.is_some_and(|stat| stat.start_time == recorded_start)
}

/// Sends `signal` to process group `pgid`, treating `ESRCH` (group already gone)
/// as a benign no-op.
fn send_group_signal(pgid: i32, signal: Signal) {
    match killpg(Pid::from_raw(pgid), Some(signal)) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {},
        Err(error) => tracing::warn!(
            %error,
            pgid,
            %signal,
            "reaper: killpg failed (non-fatal; sweep continues)"
        ),
    }
}

/// Reads `pid`'s start-time + ppid on the blocking pool (zone B per ADR-0003).
async fn probe(pid: i32) -> Option<PidStat> {
    run_blocking(move || Ok(read_pid_stat(pid))).await.ok().flatten()
}

/// Rewrites the surviving registry, or removes the directory when none survive.
async fn finalize(stack_dir: &Path, registry: &SupervisorRegistry) -> Result<(), LaunchError> {
    if registry.children.is_empty() {
        remove_stack_dir(stack_dir).await;
        return Ok(());
    }
    write_supervisor_registry(stack_dir, registry).await
}

/// Best-effort removal of a fully-drained Stack's registry directory.
async fn remove_stack_dir(stack_dir: &Path) {
    if let Err(error) = tokio::fs::remove_dir_all(stack_dir).await {
        tracing::warn!(
            %error,
            dir = %stack_dir.display(),
            "reaper: failed to remove drained stack registry dir"
        );
    }
}

/// Derives a human-readable Stack label from its registry directory name.
fn stack_label(stack_dir: &Path) -> String {
    stack_dir
        .file_name()
        .map_or_else(|| "<unknown-stack>".to_owned(), |name| name.to_string_lossy().into_owned())
}

/// Records one reconcile action with its stable [`LaunchError`] code and a fresh
/// correlation id, mirroring the audit-logging idiom in [`crate::control_fifo`].
fn record(error: &LaunchError) {
    let correlation_id = Uuid::now_v7();
    tracing::info!(
        code = error.code(),
        %correlation_id,
        message = %error,
        "reaper: recorded reconcile action"
    );
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use tempfile::TempDir;

    use substrate_domain::value_objects::StackId;

    use super::*;
    use crate::pid_probe::read_pid_stat;

    fn own_pid() -> i32 {
        i32::try_from(std::process::id()).expect("test pid fits in i32")
    }

    fn own_start_time() -> u64 {
        read_pid_stat(own_pid()).expect("own process readable").start_time
    }

    fn child(name: &str, pid: i32, start_epoch: u64) -> StackChild {
        StackChild { name: name.to_owned(), pid, pgid: pid, start_epoch }
    }

    fn registry(supervisor_pid: i32, start_epoch: u64, policy: DisconnectPolicy, children: Vec<StackChild>) -> SupervisorRegistry {
        SupervisorRegistry {
            supervisor_pid,
            start_epoch,
            policy,
            config_hash: "blake3:test".to_owned(),
            children,
        }
    }

    async fn write_stack(stacks_root: &Path, reg: &SupervisorRegistry) -> PathBuf {
        let dir = stacks_root.join(StackId::now_v7().to_crockford());
        std::fs::create_dir_all(&dir).expect("create stack dir");
        write_supervisor_registry(&dir, reg).await.expect("write supervisor.json");
        dir
    }

    // ---- Pure decision-tree tests (no live processes, no signals) ----

    #[test]
    fn decide_reaps_stale_entry_without_signal() {
        let v = decide(None, 123, DisconnectPolicy::Shutdown, 999);
        assert_eq!(v, Verdict::Reap { signal: false });
    }

    #[test]
    fn decide_clears_recycled_pid() {
        let probe = Some(PidStat { start_time: 222, ppid: 1 });
        assert_eq!(decide(probe, 111, DisconnectPolicy::Detach, 999), Verdict::Recycled);
    }

    #[test]
    fn decide_reattaches_child_still_parented_to_supervisor() {
        let probe = Some(PidStat { start_time: 111, ppid: 4242 });
        assert_eq!(decide(probe, 111, DisconnectPolicy::Shutdown, 4242), Verdict::Reattach);
    }

    #[test]
    fn decide_adopts_orphan_under_detach() {
        let probe = Some(PidStat { start_time: 111, ppid: 1 });
        assert_eq!(decide(probe, 111, DisconnectPolicy::Detach, 4242), Verdict::Adopt);
    }

    #[test]
    fn decide_reaps_orphan_under_shutdown() {
        let probe = Some(PidStat { start_time: 111, ppid: 1 });
        assert_eq!(decide(probe, 111, DisconnectPolicy::Shutdown, 4242), Verdict::Reap { signal: true });
    }

    // ---- End-to-end registry tests ----

    #[tokio::test]
    async fn live_supervisor_stack_is_reattached_and_kept() {
        let root = TempDir::new().expect("tempdir");
        let reg = registry(
            own_pid(),
            own_start_time(),
            DisconnectPolicy::Detach,
            vec![child("web", own_pid(), own_start_time())],
        );
        let dir = write_stack(root.path(), &reg).await;

        let report = reconcile_sweep(root.path()).await.expect("sweep");

        assert_eq!(report.reattached.len(), 1, "live-supervisor child re-attached");
        assert!(report.adopted.is_empty() && report.reaped.is_empty() && report.recycled.is_empty());
        assert!(dir.join(SUPERVISOR_FILE).is_file(), "a live supervisor's registry is left intact");
    }

    #[tokio::test]
    async fn stale_dead_child_is_reaped_and_dir_removed() {
        let root = TempDir::new().expect("tempdir");
        // Dead supervisor (i32::MAX never live) and a dead child pid.
        let reg = registry(
            i32::MAX,
            1,
            DisconnectPolicy::Shutdown,
            vec![child("worker", i32::MAX, 12345)],
        );
        let dir = write_stack(root.path(), &reg).await;

        let report = reconcile_sweep(root.path()).await.expect("sweep");

        assert_eq!(report.reaped.len(), 1, "a stale entry is reaped");
        assert!(!dir.exists(), "a fully-drained stack registry dir is removed");
    }

    #[tokio::test]
    async fn recycled_child_is_cleared_without_signal() {
        let root = TempDir::new().expect("tempdir");
        // Dead supervisor; child pid is our own LIVE pid but with a deliberately
        // wrong recorded start-time, so the recycle guard fires (and crucially no
        // signal is ever sent to our own process group).
        let reg = registry(
            i32::MAX,
            1,
            DisconnectPolicy::Shutdown,
            vec![child("ghost", own_pid(), own_start_time().wrapping_add(1))],
        );
        let dir = write_stack(root.path(), &reg).await;

        let report = reconcile_sweep(root.path()).await.expect("sweep");

        assert_eq!(report.recycled.len(), 1, "a pid-recycled entry is cleared");
        assert!(report.reaped.is_empty(), "recycle path must not enter the reap/signal path");
        assert!(!dir.exists(), "no survivors -> registry dir removed");
    }

    #[tokio::test]
    async fn reap_group_does_not_signal_a_dead_leader() {
        // The leader (i32::MAX) is gone, so leader_matches is false and no killpg
        // is attempted. This must complete promptly without the drain sleep.
        reap_group(&child("gone", i32::MAX, 999)).await;
    }

    #[tokio::test]
    async fn empty_root_is_a_no_op() {
        let root = TempDir::new().expect("tempdir");
        let report = reconcile_sweep(root.path()).await.expect("sweep");
        assert!(report.is_empty(), "an empty stacks root yields an empty report");
    }
}
