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
use substrate_domain::subprocess::supervisor::HealthProbe;
use substrate_domain::value_objects::pagination::PageSize;
use substrate_domain::value_objects::{ClientId, JobId};

/// Cadence at which [`wait_ready`] re-polls the subprocess port for a state change.
///
/// 50ms balances bring-up responsiveness against the cost of polling `port.list()`
/// across a potentially multi-minute readiness window (a 5ms cadence would issue
/// tens of thousands of list calls while a Spring Boot service boots).
const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Readiness budget for a Service with no real probe (`HealthProbe::None` or absent).
///
/// Such a Service is born `Running` and is treated as ready almost immediately, so
/// this only bounds a pathological never-appears case.
const NONE_READINESS_BUDGET: Duration = Duration::from_secs(10);

/// Extra time granted, on top of `startup_grace_ms`, for a `PortOpen`/`HttpGet` probe
/// to first succeed before the Service is failed as "never became ready".
///
/// Neither probe carries an overall give-up timeout of its own (only `interval_ms`
/// cadence + `startup_grace_ms` delay), so the launch BC owns the ceiling here. It is
/// deliberately generous to accommodate slow cold starts (JVM/Spring Boot, bundlers);
/// this supersedes the fixed 1s MVP ceiling that made readiness gating a no-op.
const PROBE_READINESS_BUDGET: Duration = Duration::from_mins(3);

/// Computes the total readiness budget for one Service from its declared probe.
///
/// - `None`/absent: [`NONE_READINESS_BUDGET`] (ready is expected almost at once).
/// - `PortOpen`/`HttpGet`: `startup_grace_ms` + [`PROBE_READINESS_BUDGET`].
/// - `LogPattern`: its own `timeout_ms` plus a small poll margin.
fn readiness_budget(probe: Option<&HealthProbe>) -> Duration {
    match probe {
        None | Some(HealthProbe::None) => NONE_READINESS_BUDGET,
        Some(
            HealthProbe::PortOpen {
                startup_grace_ms, ..
            }
            | HealthProbe::HttpGet {
                startup_grace_ms, ..
            },
        ) => Duration::from_millis(*startup_grace_ms) + PROBE_READINESS_BUDGET,
        Some(HealthProbe::LogPattern { timeout_ms, .. }) => {
            Duration::from_millis(*timeout_ms) + READINESS_POLL_INTERVAL * 4
        },
    }
}

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
        parent_death_signal: None,
    })
}

/// Spawns one Service through the injected [`SubprocessPort`].
///
/// The launch BC never calls `tokio::process::Command`; the subprocess adapter
/// owns the OS `fork`/`exec` and all security layers.
///
/// Before spawning, the Service binary is resolved to an absolute path (see
/// [`resolve_binary`]) so a Profile may name a binary as a bare command
/// (`"node"`, `"java"`) resolved via `$PATH`, or as a `cwd`-relative path
/// (`"./gradlew"`), in addition to an absolute path. The subprocess adapter still
/// canonicalizes and allowlist-checks the resolved path — the binary allowlist
/// remains the security gate (ADR-0070).
///
/// # Errors
///
/// Returns [`LaunchError::SpawnFailed`] wrapping the subprocess adapter's failure.
pub(crate) async fn spawn_service(
    port: &dyn SubprocessPort,
    mut request: SubprocessRequest,
    cancel: &dyn CancelSignal,
    env_files: &[String],
    profile_dir: &Path,
) -> Result<SubprocessHandle, LaunchError> {
    // Merge any `.env` files under the inline env (inline wins) before spawning.
    if !env_files.is_empty() {
        request.env_override =
            crate::env_file::merge_env_files(&request.env_override, env_files, profile_dir).await?;
    }
    request.binary_path = resolve_binary(request.binary_path.clone(), request.cwd.clone()).await;
    port.spawn(request, cancel)
        .await
        .map_err(|e| LaunchError::SpawnFailed {
            source: io::Error::other(e.to_string()),
        })
}

/// Returns the directory containing the profile file — the base against which a
/// Service's `env_file` paths are resolved (ADR-0071).
pub(crate) fn profile_dir(profile_path: &str) -> PathBuf {
    Path::new(profile_path)
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Resolves a Service binary to an absolute path, shell-style (ADR-0070).
///
/// - An absolute path is returned unchanged.
/// - A path containing a separator (e.g. `./gradlew`, `bin/tool`) is resolved
///   against the Service's own working directory (`cwd`), matching how a shell
///   treats a relative command — NOT against `$PATH`.
/// - A bare command name (no separator, e.g. `node`) is searched on `$PATH`,
///   returning the first entry that is a regular, executable file.
///
/// On any miss the original value is returned unchanged so the subprocess adapter
/// surfaces the canonical `BinaryNotAllowed` / spawn error. Resolution is only a
/// convenience for producing a candidate path: the resolved path is still subject to
/// the subprocess binary-allowlist canonicalization gate, which is unchanged.
pub(crate) async fn resolve_binary(binary: PathBuf, cwd: PathBuf) -> PathBuf {
    if binary.is_absolute() {
        return binary;
    }
    // A relative path with any separator resolves against the Service cwd (shell
    // semantics: `./x` / `a/b` are never searched on PATH).
    if binary.components().count() > 1 {
        return cwd.join(binary);
    }
    // Bare name: search $PATH on the blocking pool (filesystem metadata calls,
    // async zone B per ADR-0003). Fall back to the bare name on any miss.
    let fallback = binary.clone();
    tokio::task::spawn_blocking(move || resolve_on_path(&binary).unwrap_or(binary))
        .await
        .unwrap_or(fallback)
}

/// Searches `$PATH` for `name`, returning the first executable regular-file match.
fn resolve_on_path(name: &Path) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

/// Returns `true` when `path` is a regular file with any execute bit set.
///
/// Uses `std::os::unix::fs::PermissionsExt::mode()` (u32 on both Linux and macOS),
/// avoiding the `nix` `Mode`/`mode_t` width divergence (u32 on Linux, u16 on macOS).
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && (meta.permissions().mode() & 0o111 != 0))
}

/// Polls the subprocess port until `job_id` reaches a readiness verdict.
///
/// Returns [`ServiceOutcome::Ready`] on `Ready`/`Running` (or a zero-exit
/// `Succeeded`), and [`ServiceOutcome::Failed`] on any failed terminal state or when
/// the per-Service readiness budget is exhausted (the readiness timeout). A
/// probe-gated Service is born `Starting` and only reaches `Ready` once its health
/// probe passes, so this genuinely waits for the declared probe instead of treating
/// a freshly spawned `Running` child as ready. The budget is derived from `probe`
/// via [`readiness_budget`]. A fired `cancel` short-circuits to
/// [`ServiceOutcome::Failed`].
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
    probe: Option<&HealthProbe>,
) -> Result<ServiceOutcome, LaunchError> {
    let deadline = tokio::time::Instant::now() + readiness_budget(probe);
    loop {
        if cancel.is_cancelled() {
            return Ok(ServiceOutcome::Failed);
        }
        if let Some(outcome) = poll_once(port, client_id, job_id).await? {
            return Ok(outcome);
        }
        if tokio::time::Instant::now() >= deadline {
            // Budget exhausted: the Service never reached readiness (timeout).
            return Ok(ServiceOutcome::Failed);
        }
        tokio::time::sleep(READINESS_POLL_INTERVAL).await;
    }
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
            env_file: Vec::new(),
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
        // A probe-gated Service sits in Starting until its probe passes; the readiness
        // poll must keep waiting (None), not treat it as ready.
        assert_eq!(classify_state(SubprocessState::Starting), None);
    }

    #[tokio::test]
    async fn resolve_binary_absolute_is_unchanged() {
        // An absolute path bypasses PATH search and cwd-join entirely.
        let out = resolve_binary(PathBuf::from("/bin/echo"), PathBuf::from("/work")).await;
        assert_eq!(out, PathBuf::from("/bin/echo"));
    }

    #[tokio::test]
    async fn resolve_binary_relative_joins_cwd() {
        // A relative path with a separator resolves against the Service cwd, not PATH.
        let out = resolve_binary(PathBuf::from("./gradlew"), PathBuf::from("/work")).await;
        assert_eq!(out, PathBuf::from("/work").join("./gradlew"));

        let nested = resolve_binary(PathBuf::from("bin/tool"), PathBuf::from("/srv/app")).await;
        assert_eq!(nested, PathBuf::from("/srv/app").join("bin/tool"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn resolve_binary_bare_name_searches_path() {
        // `sh` is guaranteed on PATH on any POSIX host; a bare name resolves to an
        // absolute, executable path (never left as the bare "sh").
        let out = resolve_binary(PathBuf::from("sh"), PathBuf::from("/nonexistent-cwd")).await;
        assert!(out.is_absolute(), "bare name must resolve to an absolute path");
        assert!(out.ends_with("sh"));
        assert!(is_executable_file(&out));
    }

    #[tokio::test]
    async fn resolve_binary_bare_name_miss_falls_back_unchanged() {
        // A name not present on PATH falls back to the bare value so the subprocess
        // adapter surfaces the canonical BinaryNotAllowed/spawn error.
        let name = PathBuf::from("substrate-definitely-not-a-real-binary-xyz");
        let out = resolve_binary(name.clone(), PathBuf::from("/work")).await;
        assert_eq!(out, name);
    }

    #[test]
    #[cfg(unix)]
    fn is_executable_file_requires_regular_file_and_exec_bit() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();

        // A directory is never an executable file.
        assert!(!is_executable_file(dir.path()));

        // A regular file without an exec bit is rejected.
        let plain = dir.path().join("plain");
        std::fs::write(&plain, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable_file(&plain));

        // The same file with an exec bit set is accepted.
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable_file(&plain));

        // A missing path is not executable.
        assert!(!is_executable_file(&dir.path().join("missing")));
    }

    #[test]
    fn readiness_budget_scales_with_probe() {
        // No probe: the small fixed budget (child is born Running, ready at once).
        assert_eq!(readiness_budget(None), NONE_READINESS_BUDGET);
        assert_eq!(
            readiness_budget(Some(&HealthProbe::None)),
            NONE_READINESS_BUDGET
        );

        // PortOpen/HttpGet: startup_grace_ms + the generous probe ceiling, always far
        // larger than the None budget (this is what defeats the old 1s no-op ceiling).
        let port = HealthProbe::PortOpen {
            host: "127.0.0.1".to_owned(),
            port: 8080,
            interval_ms: 500,
            startup_grace_ms: 2_000,
        };
        assert_eq!(
            readiness_budget(Some(&port)),
            Duration::from_secs(2) + PROBE_READINESS_BUDGET
        );
        assert!(readiness_budget(Some(&port)) > NONE_READINESS_BUDGET);

        // LogPattern: its own timeout drives the budget.
        let log = HealthProbe::LogPattern {
            regex: "ready".to_owned(),
            timeout_ms: 5_000,
        };
        assert!(readiness_budget(Some(&log)) >= Duration::from_secs(5));
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
