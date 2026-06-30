//! In-process supervisor primitives for bring-up and teardown of a Stack (ADR-0063, ADR-0068).
//!
//! These free functions are the OS-facing seam of the launch BC, but they touch
//! no OS process API directly: every spawn and cancel routes through the injected
//! [`SubprocessPort`]. This crate therefore contains **zero**
//! `tokio::process::Command` calls (ADR-0063 §"in-process MVP") and needs no
//! `no_subprocess.rego` exception.
//!
//! The detached supervisor (`substrate --supervise` self-fork, control FIFO, mio
//! reactor, pidfd/kqueue child-exit, reaper-on-boot) is **Milestone 2**; the MVP
//! drives bring-up and teardown synchronously inside the live MCP server process.
//!
//! References: ADR-0063 §"in-process supervisor", ADR-0065 §"readiness gating",
//! ADR-0068 §"detached supervisor (Milestone 2)".

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use substrate_domain::launch::errors::LaunchError;
use substrate_domain::launch::profile::LaunchService;
use substrate_domain::ports::fs_index::CancelSignal;
use substrate_domain::ports::subprocess::SubprocessPort;
use substrate_domain::subprocess::handle::SubprocessHandle;
use substrate_domain::subprocess::request::{CaptureKind, StdinKind, SubprocessRequest};
use substrate_domain::subprocess::state::SubprocessState;
use substrate_domain::value_objects::pagination::PageSize;
use substrate_domain::value_objects::{ClientId, JobId};

/// Cadence at which [`wait_ready`] re-polls the subprocess port for a state change.
const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(5);
/// Upper bound on readiness polls before a Service is treated as failed (timeout).
///
/// `200 * 5ms = 1s`. A real health-probe budget (ADR-0056) supersedes this in
/// Milestone 2; the MVP uses a fixed ceiling so a hung Service cannot block
/// bring-up indefinitely.
const READINESS_MAX_POLLS: u32 = 200;

/// The terminal readiness verdict for one Service after its bring-up poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceOutcome {
    /// The Service reached `Ready`/`Running` (or exited zero) within budget.
    Ready,
    /// The Service failed readiness: it crashed, timed out, or never became ready.
    Failed,
}

/// Builds the [`SubprocessRequest`] that materializes `service` as a child process.
///
/// `argv[0]` becomes `binary_path`; the remaining argv plus `service.args` become
/// `args`. The request is marked `elicitation_confirmed = true` because an
/// orchestrated launch spawn carries the operator's `launch.up` confirmation, not
/// a per-child elicitation. `default_cwd` is used when the Service declares no
/// explicit working directory.
///
/// # Errors
///
/// Returns [`LaunchError::InvalidProfile`] when the Service command is not a
/// non-empty argv array (a bare-string command is rejected per ADR-0064).
pub(crate) fn build_request(
    name: &str,
    service: &LaunchService,
    default_cwd: &Path,
) -> Result<SubprocessRequest, LaunchError> {
    let command_argv = service.command.argv()?;
    let (binary, rest) = command_argv
        .split_first()
        .ok_or_else(|| LaunchError::InvalidProfile {
            msg: format!("service '{name}' has an empty command"),
        })?;
    let mut arg_list: Vec<String> = rest.to_vec();
    arg_list.extend(service.args.iter().cloned());
    let cwd: PathBuf = service
        .cwd
        .as_ref()
        .map_or_else(|| default_cwd.to_path_buf(), PathBuf::from);

    Ok(SubprocessRequest {
        binary_path: PathBuf::from(binary),
        args: arg_list,
        env_allowlist: Vec::new(),
        env_override: service.env.clone(),
        cwd,
        stdin_kind: StdinKind::None,
        capture_kind: CaptureKind::InMemory,
        timeout_secs: None,
        idempotency_key: None,
        // An orchestrated spawn is authorized by the launch.up confirmation; the
        // subprocess BC still re-checks every Layer 1-5 invariant on its side.
        elicitation_confirmed: true,
        name: Some(name.to_owned()),
        restart_policy: service.restart_policy.clone(),
        health_probe: service.health_probe.clone(),
        log_rotation: None,
    })
}

/// Spawns one Service through the injected [`SubprocessPort`].
///
/// The launch BC never calls `tokio::process::Command`; the subprocess adapter
/// owns the OS `fork`/`exec` and all security layers.
///
/// # Errors
///
/// Returns [`LaunchError::SpawnFailed`] wrapping the subprocess adapter's failure.
pub(crate) async fn spawn_service(
    port: &dyn SubprocessPort,
    request: SubprocessRequest,
    cancel: &dyn CancelSignal,
) -> Result<SubprocessHandle, LaunchError> {
    port.spawn(request, cancel)
        .await
        .map_err(|e| LaunchError::SpawnFailed {
            source: io::Error::other(e.to_string()),
        })
}

/// Polls the subprocess port until `job_id` reaches a readiness verdict.
///
/// Returns [`ServiceOutcome::Ready`] on `Ready`/`Running` (or a zero-exit
/// `Succeeded`), and [`ServiceOutcome::Failed`] on any failed terminal state or
/// when the fixed poll budget is exhausted (the MVP readiness timeout). A fired
/// `cancel` short-circuits to [`ServiceOutcome::Failed`].
///
/// # Errors
///
/// Returns [`LaunchError::SpawnFailed`] when the subprocess port itself errors
/// while listing handles.
pub(crate) async fn wait_ready(
    port: &dyn SubprocessPort,
    client_id: &ClientId,
    job_id: &JobId,
    cancel: &dyn CancelSignal,
) -> Result<ServiceOutcome, LaunchError> {
    for _ in 0..READINESS_MAX_POLLS {
        if cancel.is_cancelled() {
            return Ok(ServiceOutcome::Failed);
        }
        if let Some(outcome) = poll_once(port, client_id, job_id).await? {
            return Ok(outcome);
        }
        tokio::time::sleep(READINESS_POLL_INTERVAL).await;
    }
    // Budget exhausted: the Service never reached readiness (timeout).
    Ok(ServiceOutcome::Failed)
}

/// Performs a single readiness poll, returning `None` while the Service is still
/// transitioning (`Pending`/`Starting`/`Restarting` or not yet visible).
async fn poll_once(
    port: &dyn SubprocessPort,
    client_id: &ClientId,
    job_id: &JobId,
) -> Result<Option<ServiceOutcome>, LaunchError> {
    let (handles, _) = port
        .list(client_id, None, None, PageSize::DEFAULT)
        .await
        .map_err(|e| LaunchError::SpawnFailed {
            source: io::Error::other(e.to_string()),
        })?;
    let Some(handle) = handles.iter().find(|h| &h.job_id == job_id) else {
        return Ok(None);
    };
    Ok(classify_state(handle.state))
}

/// Maps a [`SubprocessState`] to a readiness verdict, or `None` while in flight.
const fn classify_state(state: SubprocessState) -> Option<ServiceOutcome> {
    match state {
        SubprocessState::Ready | SubprocessState::Running | SubprocessState::Succeeded => {
            Some(ServiceOutcome::Ready)
        },
        SubprocessState::Failed
        | SubprocessState::Cancelled
        | SubprocessState::Killed
        | SubprocessState::TimedOut => Some(ServiceOutcome::Failed),
        SubprocessState::Pending | SubprocessState::Starting | SubprocessState::Restarting => None,
    }
}

/// Cascade-stops one Service through the subprocess port (SIGTERM-then-drain).
///
/// Errors are swallowed: a Service already terminal (race with natural exit) or a
/// missing job is a no-op for teardown idempotency. `force = false` lets the
/// subprocess adapter run its SIGTERM -> drain -> SIGKILL cascade per ADR-0053.
pub(crate) async fn stop_service(port: &dyn SubprocessPort, job_id: &JobId) {
    let _ = port.cancel(job_id, false).await;
}

/// Builds the orchestrator's [`ClientId`] used for every subprocess operation.
///
/// The launch BC owns its Services under one stable client identity so the
/// subprocess BC's per-client isolation groups them together.
///
/// # Errors
///
/// Returns [`LaunchError::InvalidProfile`] only if the compiled-in identity ever
/// violates the `ClientId` pattern (unreachable with the constant below).
pub(crate) fn launch_client_id() -> Result<ClientId, LaunchError> {
    ClientId::parse("launch-orchestrator").map_err(|e| LaunchError::InvalidProfile {
        msg: format!("internal: invalid launch client id: {e}"),
    })
}

/// Returns `true` when any spawn-affecting field differs between two revisions of
/// the same Service.
///
/// Spawn-affecting fields force a child re-spawn on reload: `command`, `args`,
/// `env`, and `cwd`. Supervisor-live fields (`restart_policy`, `health_probe`,
/// `error_patterns`, `redact`, `streams`, `on_dependency_restart`) and dependency
/// edges (`depends_on`) are handled without a re-spawn (ADR-0065 reconciler).
#[must_use]
pub(crate) fn spawn_fields_differ(old: &LaunchService, new: &LaunchService) -> bool {
    old.command != new.command
        || old.args != new.args
        || old.env != new.env
        || old.cwd != new.cwd
}

/// Returns `true` when only the dependency edges differ (no spawn-affecting change).
#[must_use]
pub(crate) fn edges_only_differ(old: &LaunchService, new: &LaunchService) -> bool {
    !spawn_fields_differ(old, new) && old.depends_on != new.depends_on
}

/// Maps a readiness verdict to the per-Service [`SubprocessState`] recorded in the
/// Stack handle.
#[must_use]
pub(crate) const fn outcome_state(outcome: ServiceOutcome) -> SubprocessState {
    match outcome {
        ServiceOutcome::Ready => SubprocessState::Ready,
        ServiceOutcome::Failed => SubprocessState::Failed,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use std::collections::BTreeMap;

    use substrate_domain::launch::profile::{CommandSpec, DependencyRestartMode, StreamMux};
    use substrate_domain::subprocess::supervisor::RestartPolicy;

    use super::*;

    fn service(cmd: &[&str]) -> LaunchService {
        LaunchService {
            command: CommandSpec::Argv(cmd.iter().map(|s| (*s).to_owned()).collect()),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            depends_on: Vec::new(),
            required: true,
            restart_policy: None,
            health_probe: None,
            on_dependency_restart: DependencyRestartMode::Restart,
            error_patterns: Vec::new(),
            redact: Vec::new(),
            streams: StreamMux::Multiplexed,
        }
    }

    #[test]
    fn build_request_splits_argv_and_appends_args() {
        let mut svc = service(&["cargo", "run"]);
        svc.args = vec!["--release".to_owned()];
        let req = build_request("api", &svc, Path::new("/work")).unwrap();
        assert_eq!(req.binary_path, PathBuf::from("cargo"));
        assert_eq!(req.args, vec!["run".to_owned(), "--release".to_owned()]);
        assert_eq!(req.cwd, PathBuf::from("/work"));
        assert_eq!(req.name.as_deref(), Some("api"));
        assert!(req.elicitation_confirmed);
    }

    #[test]
    fn build_request_rejects_string_command() {
        let mut svc = service(&["x"]);
        svc.command = CommandSpec::Shell("echo hi".to_owned());
        assert!(matches!(
            build_request("web", &svc, Path::new("/work")),
            Err(LaunchError::InvalidProfile { .. })
        ));
    }

    #[test]
    fn classify_state_maps_lifecycle_correctly() {
        assert_eq!(
            classify_state(SubprocessState::Ready),
            Some(ServiceOutcome::Ready)
        );
        assert_eq!(
            classify_state(SubprocessState::Failed),
            Some(ServiceOutcome::Failed)
        );
        assert_eq!(classify_state(SubprocessState::Starting), None);
    }

    #[test]
    fn spawn_fields_differ_on_args_change() {
        let a = service(&["bin"]);
        let mut b = service(&["bin"]);
        b.args = vec!["--flag".to_owned()];
        assert!(spawn_fields_differ(&a, &b));
        assert!(!edges_only_differ(&a, &b));
    }

    #[test]
    fn edges_only_differ_on_depends_on_change() {
        let a = service(&["bin"]);
        let mut b = service(&["bin"]);
        b.depends_on = vec!["db".to_owned()];
        assert!(!spawn_fields_differ(&a, &b));
        assert!(edges_only_differ(&a, &b));
    }

    #[test]
    fn metadata_only_change_is_neither_spawn_nor_edge() {
        let a = service(&["bin"]);
        let mut b = service(&["bin"]);
        b.restart_policy = Some(RestartPolicy::Always { backoff_ms: 1000 });
        assert!(!spawn_fields_differ(&a, &b));
        assert!(!edges_only_differ(&a, &b));
    }

    #[test]
    fn launch_client_id_is_valid() {
        assert_eq!(launch_client_id().unwrap().as_str(), "launch-orchestrator");
    }
}
