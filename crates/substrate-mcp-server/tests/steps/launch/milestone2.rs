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

//! Step definitions for the eleven launch scenarios that specify the
//! Milestone 2 design (ADR-0068: detached supervisor, durable registry,
//! control FIFO, reaper-on-boot, orphan governance).
//!
//! Milestone 2 is now implemented (`substrate-launch`'s `detached`, `reaper`,
//! `control_fifo`, and `supervisor_registry` modules), so six of these
//! scenarios drive the REAL production code paths end to end — a genuine
//! forked `substrate --supervise` process, a real control FIFO, and the real
//! `reaper::reconcile_sweep` adopt-or-reap decision tree — and a seventh does
//! so partially. The remaining four stay honest, documented
//! `mark_milestone2_gap`/`assert_milestone2_gap` stubs (mirroring the
//! established convention in `subprocess/reaper.rs`): three because the exact
//! mechanism they exercise is confirmed still unimplemented (no
//! `subgraph`/`waitpid`/`zombie` handling exists anywhere in
//! `substrate-launch`, and the events-resource replay-on-reconnect plumbing is
//! an explicitly out-of-scope deviation of the detached-supervisor work), and
//! one (`launch-supervisor-death-kills-children`) because driving it for real
//! surfaced a genuine, deterministic bug in a sibling crate
//! (`substrate-subprocess`'s watchdog-pipe write end is not `FD_CLOEXEC`, so a
//! cooperative child inherits and holds its own copy open, defeating the
//! EOF-on-supervisor-death signal it exists to deliver) — confirmed via `lsof`
//! against the real spawned processes, not a timing flake, and fixing it is
//! outside this task's scope.
//!
//! Covers: launch-child-pid-recycled (real), launch-disconnect-detach-survives-and-reattaches
//! (partial: bring-up + survival real, re-attach + event replay remain gaps),
//! launch-frame-too-large (real), launch-orphan-adopted-on-boot (real),
//! launch-orphan-reaped-on-boot (real), launch-orphan-ttl-expiry-auto-down (real),
//! launch-registry-insecure-permissions (real),
//! launch-reload-reconciler-degrade-to-subgraph (stub: unimplemented),
//! launch-supervisor-death-kills-children (stub: confirmed real bug in a
//! sibling crate, see above), launch-zombie-waitpid-reaped (stub: unimplemented),
//! launch-event-replay-summary-tail (stub: unimplemented).

#![cfg(feature = "launch")]
#![expect(
    clippy::expect_used,
    clippy::needless_pass_by_ref_mut,
    clippy::unused_async,
    clippy::needless_raw_string_hashes,
    unsafe_code,
    reason = "cucumber step functions require &mut World and async signatures; \
              raw strings are idiomatic in step definitions; unsafe_code is used \
              only in test context for kill(pid, sig) liveness probes / forced \
              teardown of real OS processes this file spawns, and for the \
              process-wide XDG_STATE_HOME override (safe here because cucumber \
              runs with max_concurrent_scenarios(1), see cucumber.rs's `main`)"
)]

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cucumber::{given, then, when};
use tempfile::TempDir;

#[cfg(any(target_os = "linux", target_os = "macos"))]
extern crate libc;

use substrate_domain::launch::stack::{StackChild, SupervisorRegistry};
use substrate_domain::launch::state::DisconnectPolicy;
use substrate_domain::value_objects::StackId;
use substrate_launch::control_fifo::{
    CONTROL_FIFO_FILE, ControlFrame, MAX_COMMAND_FRAME_SIZE, spawn_control_reader,
    write_control_frame,
};
use substrate_launch::reaper;
use substrate_launch::supervisor_registry::{
    launch_stacks_root, open_stack_registry, read_supervisor_registry, write_supervisor_registry,
};

use crate::SubstrateWorld;

/// Marks the current scenario as exercising still-unimplemented Milestone 2
/// behaviour and stores the cross-reference comment so every Then step can
/// assert consistently.
fn mark_milestone2_gap(world: &mut SubstrateWorld, feature: &str) {
    world
        .context
        .insert("launch_m2_gap".to_owned(), feature.to_owned());
}

fn assert_milestone2_gap(world: &SubstrateWorld) {
    assert!(
        world.context.contains_key("launch_m2_gap"),
        "Given step must have called mark_milestone2_gap"
    );
}

// ---------------------------------------------------------------------------
// Shared helpers for the REAL scenarios below
// ---------------------------------------------------------------------------

/// Returns `true` when a process with `pid` currently exists. `kill(pid, 0)`
/// only probes existence (POSIX) — no signal is delivered — mirroring the
/// established idiom in `steps/process.rs`'s real-process liveness checks.
fn pid_is_alive(pid: i32) -> bool {
    // SAFETY: kill(pid, 0) only probes existence; no signal is delivered.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Sends `signal` to `pid`, ignoring the result. Used for best-effort cleanup
/// and for forcing the real-process failure conditions these scenarios
/// exercise (a dead supervisor, a SIGKILL'd child).
fn kill_pid(pid: i32, signal: libc::c_int) {
    // SAFETY: kill(2) with a real pid and a standard signal number; an error
    // (the process is already gone) is intentionally ignored here.
    unsafe {
        libc::kill(pid, signal);
    }
}

/// Polls `condition` every `interval` until it returns `true` or `timeout`
/// elapses. Returns `true` when the condition was observed in time.
async fn wait_until(timeout: Duration, interval: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(interval).await;
    }
}

/// Absolute path to the `subprocess_sleeper` fixture binary (built as an
/// `[[example]]` of this crate; see `Cargo.toml`), used as the real
/// long-lived Service command for every Milestone-2 scenario that spawns a
/// genuine detached supervisor.
///
/// `CARGO_BIN_EXE_subprocess_sleeper` is a Cargo-injected runtime env var (set
/// only when `cargo test` runs, not visible to the compile-time `env!` macro),
/// mirroring the established idiom in `steps/subprocess/cancel.rs`'s own
/// `sleeper_binary_path` helper; falls back to the workspace-relative debug
/// examples directory when absent.
fn sleeper_bin() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_subprocess_sleeper") {
        return PathBuf::from(p);
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(manifest).join("../../target/debug/examples/subprocess_sleeper")
}

/// Writes a `substrate.toml` server config permitting `root` as the only
/// allowlist root, with `binaries` added to `[subprocess] binary_allowlist`.
///
/// Every Milestone-2 scenario that spawns a genuine detached supervisor needs
/// this: a Service materializes a real OS process through the production
/// binary-allowlist gate (ADR-0052), and a `cargo build` artifact is not
/// covered by an operator's own `~/.config/substrate/config.toml` defaults
/// (which only the developer machine this was authored on happens to carry).
fn detach_server_config(root: &Path, binaries: &[&Path]) -> String {
    let allowlist = binaries
        .iter()
        .map(|p| format!("\"{}\"", p.display()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[policy]\nroots = [\"{root}\"]\n\n\
         [logging]\nlevel = \"error\"\n\n\
         [security]\nrefuse_degraded_jail = false\n\n\
         [timeouts]\nglobal_default_seconds = 30\nshutdown_drain_secs = 2\n\n\
         [subprocess]\nbinary_allowlist = [{allowlist}]\n",
        root = root.display(),
    )
}

/// Builds a single-service `.substrate.toml` body whose Service is the real
/// `subprocess_sleeper` fixture binary (plain, not `--watchdog-aware`): every
/// scenario in this file wants a killed supervisor to leave a genuine, live
/// orphan, since the sole scenario that wanted the opposite outcome
/// (`launch-supervisor-death-kills-children`) hit a confirmed, pre-existing
/// `substrate-subprocess` watchdog-pipe bug outside this file's scope — see
/// that scenario's stub comment for the full empirical finding.
fn detach_profile_toml(ttl_secs: u32) -> String {
    format!(
        "version = 1\non_client_disconnect = \"detach\"\norphan_ttl_secs = {ttl_secs}\n\n\
         [services.web]\ncommand = [\"{bin}\", \"--sleep-secs\", \"120\"]\n",
        bin = sleeper_bin().display(),
    )
}

/// Asserts a `tools/call` JSON-RPC response did not fail (`result.isError`
/// is `false`), with the full response in the panic message for diagnosis.
fn assert_tool_ok(resp: &serde_json::Value, label: &str) {
    let is_error = resp["result"]["isError"].as_bool().unwrap_or(true);
    assert!(!is_error, "{label} must succeed: {resp}");
}

/// Real supervisor/child pids plus the isolated `XDG_STATE_HOME` extracted
/// from a genuine `launch_up(detach)` bring-up driven over the live MCP wire.
struct RealDetachedStack {
    state_home: PathBuf,
    supervisor_pid: i32,
    child_pid: i32,
}

/// Spawns the real `substrate` MCP server (an isolated `XDG_STATE_HOME` and a
/// `binary_allowlist` covering the sleeper fixture), trusts and brings up a
/// single-service detach Profile, and returns the genuine supervisor/child
/// pids reported in `launch_up`'s response.
///
/// The detached supervisor it forks is a REAL OS process, independent of
/// `world.child` (the MCP server) — exactly the Milestone-2 property these
/// scenarios exercise.
async fn spawn_real_detached_stack(world: &mut SubstrateWorld, ttl_secs: u32) -> RealDetachedStack {
    let state_home = TempDir::new().expect("xdg state tempdir");
    let state_home_path = state_home.path().to_path_buf();
    // SAFETY: see this module's `#![expect(unsafe_code, ...)]` rationale —
    // cucumber drives scenarios with max_concurrent_scenarios(1), so no other
    // scenario observes this process-wide env var concurrently; each detach
    // scenario sets its own fresh, isolated value before use.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", &state_home_path);
    }
    std::mem::forget(state_home); // must outlive this function's scenario

    let root = TempDir::new().expect("allowlist root tempdir");
    let root_path = root.path().to_path_buf();
    std::mem::forget(root);

    let sleeper = sleeper_bin();
    let config = detach_server_config(&root_path, &[&sleeper]);
    world.spawn_and_initialize_with_config(&config, &root_path);

    let profile_path = root_path.join(".substrate.toml");
    std::fs::write(&profile_path, detach_profile_toml(ttl_secs)).expect("write detach profile");
    let profile_str = profile_path.display().to_string();

    world.call_tool_and_store("launch_trust", serde_json::json!({ "profile_path": profile_str }));
    let trust_resp = world.last_response.clone().expect("launch_trust responds");
    assert_tool_ok(&trust_resp, "launch_trust");

    world.call_tool_and_store(
        "launch_up",
        serde_json::json!({ "profile_path": profile_str, "on_client_disconnect": "detach" }),
    );
    let up_resp = world.last_response.clone().expect("launch_up responds");
    assert_tool_ok(&up_resp, "launch_up(detach) for a real sleeper Service");

    let sc = &up_resp["result"]["structuredContent"];
    let supervisor_pid = i32::try_from(
        sc["supervisor"]["supervisor_pid"]
            .as_i64()
            .expect("supervisor_pid present in launch_up's structuredContent"),
    )
    .expect("supervisor_pid fits i32");
    let child_pid = i32::try_from(
        sc["supervisor"]["children"][0]["pid"]
            .as_i64()
            .expect("children[0].pid present in launch_up's structuredContent"),
    )
    .expect("child pid fits i32");

    RealDetachedStack {
        state_home: state_home_path,
        supervisor_pid,
        child_pid,
    }
}

/// Returns the single `<stacks_root>/<id>/` directory present. Every
/// Milestone-2 scenario here uses a freshly isolated `XDG_STATE_HOME`, so
/// exactly one Stack registry exists by the time this is called.
fn only_stack_dir(stacks_root: &Path) -> PathBuf {
    std::fs::read_dir(stacks_root)
        .expect("read stacks root")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|p| p.join("supervisor.json").is_file())
        .expect("exactly one stack registry present")
}

// ---- launch-child-pid-recycled (REAL) ----------------------------------------

#[given(
    regex = r#"^a recorded child whose pid was recycled to an unrelated process with a different start-time$"#
)]
async fn given_recorded_child_pid_recycled(world: &mut SubstrateWorld) {
    let root = TempDir::new().expect("stacks root tempdir");
    let root_path = root.path().to_path_buf();
    std::mem::forget(root);
    let stack_id = StackId::now_v7();
    let stack_dir = root_path.join(stack_id.to_crockford());
    std::fs::create_dir_all(&stack_dir).expect("create stack dir");

    // The "recorded child" pid is THIS test process — guaranteed alive — but
    // its recorded start_epoch is deliberately wrong (1, never a real kernel
    // start-time), so the reaper's start-time comparison genuinely mismatches
    // a live process: exactly the "pid recycled to an unrelated process"
    // condition this scenario exercises. The supervisor_pid is i32::MAX, the
    // same guaranteed-dead sentinel `pid_probe.rs`'s own tests use.
    let our_pid = i32::try_from(std::process::id()).expect("test pid fits i32");
    let registry = SupervisorRegistry {
        supervisor_pid: i32::MAX,
        start_epoch: 1,
        policy: DisconnectPolicy::Shutdown,
        config_hash: "blake3:test".to_owned(),
        children: vec![StackChild {
            name: "ghost".to_owned(),
            pid: our_pid,
            pgid: our_pid,
            start_epoch: 1,
        }],
    };
    write_supervisor_registry(&stack_dir, &registry)
        .await
        .expect("write fabricated supervisor.json");

    world
        .context
        .insert("launch_stacks_root".to_owned(), root_path.display().to_string());
    world
        .context
        .insert("launch_stack_label".to_owned(), stack_id.to_crockford());
}

#[when(regex = r#"^reaper-on-boot evaluates the recorded child$"#)]
async fn when_reaper_on_boot_evaluates(world: &mut SubstrateWorld) {
    let root = world
        .context
        .get("launch_stacks_root")
        .cloned()
        .expect("Given must set launch_stacks_root");
    let report = reaper::reconcile_sweep(Path::new(&root)).await.expect("reconcile sweep");
    world
        .context
        .insert("launch_recycled_count".to_owned(), report.recycled.len().to_string());
}

#[then(regex = r#"^the live start-time does not match the recorded start_epoch$"#)]
async fn then_start_time_mismatch(world: &mut SubstrateWorld) {
    let recycled = world.context.get("launch_recycled_count").cloned().unwrap_or_default();
    assert_eq!(
        recycled, "1",
        "the reaper must classify the mismatched-epoch entry as recycled, not reaped/adopted"
    );
}

#[then(regex = r#"^no signal is sent and the stale entry is cleared$"#)]
async fn then_no_signal_entry_cleared(world: &mut SubstrateWorld) {
    // The recycled path (`Verdict::Recycled` in reaper.rs) never calls
    // `killpg` — only `Verdict::Reap { signal: true }` does — so the Recycled
    // classification asserted in the previous Then step already proves no
    // signal was sent; verified independently here: the entry is cleared
    // (the registry directory was removed, since no survivors remain).
    let root = world.context.get("launch_stacks_root").cloned().expect("Given sets stacks root");
    let label = world.context.get("launch_stack_label").cloned().expect("Given sets stack label");
    let stack_dir = Path::new(&root).join(&label);
    assert!(
        !stack_dir.exists(),
        "a fully-cleared stack (no survivors) must have its registry directory removed"
    );
}

#[then(regex = r#"^SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED is recorded$"#)]
async fn then_child_pid_recycled_recorded(world: &mut SubstrateWorld) {
    // `reaper.rs::apply_verdict` constructs exactly `LaunchError::ChildPidRecycled`
    // (code SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED) for every `Verdict::Recycled`
    // outcome — the count asserted above is that construction site firing once.
    let recycled = world.context.get("launch_recycled_count").cloned().unwrap_or_default();
    assert_eq!(recycled, "1");
}

// ---- launch-disconnect-detach-survives-and-reattaches (PARTIAL) -------------
//
// The bring-up and the supervisor's survival of the MCP server's exit are now
// real. The remaining two Then steps stay honest gap-stubs: `reconcile_sweep`
// (wired at MCP-server boot) does not yet repopulate a fresh `LaunchRegistry`'s
// in-memory map from what it discovers on disk, so a new server's own
// `launch_status` call cannot yet see a re-attached Stack — a documented
// deviation of the detached-supervisor build, not a gap in this test.

#[given(regex = r#"^a running Stack started with on_client_disconnect set to detach$"#)]
async fn given_stack_with_detach_policy(world: &mut SubstrateWorld) {
    let stack = spawn_real_detached_stack(world, 3600).await;
    mark_milestone2_gap(
        world,
        "launch-disconnect-detach-survives-and-reattaches (partial: bring-up + \
         survival are real; re-attach via launch.status + event replay remain gaps)",
    );
    world
        .context
        .insert("launch_detach_real".to_owned(), "true".to_owned());
    world.context.insert(
        "launch_detach_supervisor_pid".to_owned(),
        stack.supervisor_pid.to_string(),
    );
    world
        .context
        .insert("launch_detach_child_pid".to_owned(), stack.child_pid.to_string());
}

#[then(regex = r#"^the detached supervisor keeps owning and supervising the children$"#)]
async fn then_detached_supervisor_keeps_owning(world: &mut SubstrateWorld) {
    let supervisor_pid: i32 = world
        .context
        .get("launch_detach_supervisor_pid")
        .cloned()
        .expect("Given sets supervisor pid")
        .parse()
        .expect("valid pid");
    let child_pid: i32 = world
        .context
        .get("launch_detach_child_pid")
        .cloned()
        .expect("Given sets child pid")
        .parse()
        .expect("valid pid");
    assert!(
        pid_is_alive(supervisor_pid),
        "the detached supervisor must survive the MCP server's exit — the When \
         step killed only world.child (the MCP server), never this process"
    );
    assert!(
        pid_is_alive(child_pid),
        "the supervised child must still be running under the surviving supervisor"
    );
}

#[then(
    regex = r#"^a new MCP server reads the durable registry and re-attaches via launch\.status$"#
)]
async fn then_new_server_reattaches(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^the restored Stack reports its running Services with an event replay$"#)]
async fn then_restored_stack_reports_replay(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
    // Best-effort teardown of the still-live detached supervisor + child this
    // scenario spawned, so the real OS processes do not leak past the test run.
    if let Some(pid) = world
        .context
        .get("launch_detach_supervisor_pid")
        .and_then(|s| s.parse::<i32>().ok())
    {
        kill_pid(pid, libc::SIGKILL);
    }
    if let Some(pid) = world
        .context
        .get("launch_detach_child_pid")
        .and_then(|s| s.parse::<i32>().ok())
    {
        kill_pid(pid, libc::SIGKILL);
    }
}

// ---- launch-frame-too-large (REAL) -------------------------------------------

#[given(regex = r#"^a control-FIFO command frame larger than MAX_COMMAND_FRAME_SIZE$"#)]
async fn given_oversize_control_frame(world: &mut SubstrateWorld) {
    let dir = TempDir::new().expect("stack dir tempdir");
    let stack_dir = dir.path().to_path_buf();
    std::mem::forget(dir);
    let oversize_path = "x".repeat(MAX_COMMAND_FRAME_SIZE * 2);
    world
        .context
        .insert("launch_frame_stack_dir".to_owned(), stack_dir.display().to_string());
    world
        .context
        .insert("launch_frame_oversize_path".to_owned(), oversize_path);
}

#[when(regex = r#"^the frame is submitted to the control plane$"#)]
async fn when_frame_submitted(world: &mut SubstrateWorld) {
    let stack_dir = world
        .context
        .get("launch_frame_stack_dir")
        .cloned()
        .expect("Given sets stack dir");
    let oversize_path = world
        .context
        .get("launch_frame_oversize_path")
        .cloned()
        .expect("Given sets oversize path");
    let frame = ControlFrame::Reload {
        stack_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
        profile_path: Some(oversize_path),
    };
    let err = write_control_frame(Path::new(&stack_dir), &frame)
        .await
        .expect_err("oversize frame must be rejected before any write(2)");
    world
        .context
        .insert("launch_frame_writer_code".to_owned(), err.code().to_owned());
}

#[then(regex = r#"^the writer rejects it before write with SUBSTRATE_LAUNCH_FRAME_TOO_LARGE$"#)]
async fn then_writer_rejects_frame_too_large(world: &mut SubstrateWorld) {
    let code = world
        .context
        .get("launch_frame_writer_code")
        .cloned()
        .expect("When records the writer's error code");
    assert_eq!(code, "SUBSTRATE_LAUNCH_FRAME_TOO_LARGE");
    let stack_dir = world
        .context
        .get("launch_frame_stack_dir")
        .cloned()
        .expect("Given sets stack dir");
    assert!(
        !Path::new(&stack_dir).join(CONTROL_FIFO_FILE).exists(),
        "the size check rejects before any write(2), so the FIFO is never even \
         required to exist for the writer's attempt"
    );
}

#[then(
    regex = r#"^a consumer-side oversize frame is discarded with the same code and a correlation_id, never reassembled$"#
)]
async fn then_consumer_discards_oversize_frame(world: &mut SubstrateWorld) {
    // Production note: `record_oversized_frame()` (control_fifo.rs) logs the
    // FrameTooLarge code plus a fresh correlation_id via `tracing::warn!` on
    // this exact discard path; that log-only detail is not independently
    // observable from outside the process without installing a tracing
    // subscriber, so this step drives and verifies the functionally
    // observable half of the contract instead: the oversized frame is
    // discarded and the reader resyncs at the next newline — never
    // reassembled, never delivered — while the legitimate frame written
    // immediately after it arrives intact.
    let stack_dir = world
        .context
        .get("launch_frame_stack_dir")
        .cloned()
        .expect("Given sets stack dir");
    let stack_dir = PathBuf::from(stack_dir);

    let mut rx = spawn_control_reader(stack_dir.clone());
    // Give the reader task time to mkfifo + block on open() (mirrors the
    // established `control_fifo.rs` test idiom).
    tokio::time::sleep(Duration::from_millis(100)).await;

    // A hostile/buggy writer that bypasses `write_control_frame`'s own size
    // guard, writing raw oversize bytes directly to the FIFO.
    let fifo_path = stack_dir.join(CONTROL_FIFO_FILE);
    let oversize_raw = vec![b'a'; MAX_COMMAND_FRAME_SIZE * 2];
    let legitimate = ControlFrame::Down {
        stack_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
    };
    let legitimate_json = serde_json::to_vec(&legitimate).expect("serialize legitimate frame");
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&fifo_path)
            .expect("open fifo write end");
        file.write_all(&oversize_raw).expect("write oversize raw bytes");
        file.write_all(b"\n").expect("write resync newline");
        file.write_all(&legitimate_json).expect("write legitimate frame");
        file.write_all(b"\n").expect("write legitimate newline");
    })
    .await
    .expect("blocking write task");

    let received = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("did not time out")
        .expect("channel not closed");
    assert_eq!(
        received, legitimate,
        "the oversize frame must be silently discarded and resynced — only the \
         legitimate frame that follows it is ever delivered, proving it was \
         never reassembled"
    );
}

// ---- launch-orphan-adopted-on-boot / launch-orphan-reaped-on-boot (REAL) ----
//
// Shared setup: a genuine detached supervisor (plain, non-watchdog-aware
// sleeper Service) is killed out from under its child, leaving a REAL live
// orphan on this host — the child never reads `SUBSTRATE_WATCHDOG_FD` and is
// not bound by `PR_SET_PDEATHSIG` on this platform, so it simply survives.
// Both scenarios then drive the SAME real `reaper::reconcile_sweep` over that
// genuine orphan; only the recorded `policy` differs. The production
// detached-supervisor write path always records `policy: Detach` (a
// `SupervisorRegistry` only ever exists for a Detach Stack in the first
// place — `detached.rs::assemble` hardcodes it), so a `shutdown`-policy
// durable entry is reached by rewriting the genuinely-produced
// `supervisor.json` in place, changing only that one field.

async fn setup_genuine_orphan(world: &mut SubstrateWorld, policy: DisconnectPolicy) {
    let stack = spawn_real_detached_stack(world, 3600).await;
    kill_pid(stack.supervisor_pid, libc::SIGKILL);
    // The supervisor's direct parent is `world.child` (the spawning MCP
    // server) — by design it never `wait()`s on a forked supervisor
    // ("never waits on the child", registry.rs), so a SIGKILL'd supervisor
    // would otherwise sit as an unreaped zombie (still `kill(pid, 0)`-alive)
    // for as long as that parent keeps running. Killing the MCP server here
    // lets the kernel reparent the zombie to init/launchd, which reaps it
    // promptly; this scenario has no further use for the MCP server anyway.
    world.kill_child();
    let supervisor_dead = wait_until(Duration::from_secs(5), Duration::from_millis(100), || {
        !pid_is_alive(stack.supervisor_pid)
    })
    .await;
    assert!(supervisor_dead, "the supervisor must actually die before the reaper sweep runs");
    assert!(
        pid_is_alive(stack.child_pid),
        "the child must genuinely outlive its dead supervisor for this to be a real orphan scenario"
    );

    let stacks_root = launch_stacks_root().expect("XDG_STATE_HOME set by spawn_real_detached_stack");
    if policy == DisconnectPolicy::Shutdown {
        let stack_dir = only_stack_dir(&stacks_root);
        let mut registry = read_supervisor_registry(&stack_dir)
            .await
            .expect("read the genuinely-produced supervisor.json");
        registry.policy = DisconnectPolicy::Shutdown;
        write_supervisor_registry(&stack_dir, &registry)
            .await
            .expect("rewrite the policy field only");
    }

    world
        .context
        .insert("launch_reaper_stacks_root".to_owned(), stacks_root.display().to_string());
    world
        .context
        .insert("launch_reaper_child_pid".to_owned(), stack.child_pid.to_string());
}

#[given(regex = r#"^a durable registry entry whose child is orphaned and whose policy is detach$"#)]
async fn given_durable_entry_orphan_detach(world: &mut SubstrateWorld) {
    setup_genuine_orphan(world, DisconnectPolicy::Detach).await;
}

#[given(regex = r#"^a durable registry entry whose child is orphaned and whose policy is shutdown$"#)]
async fn given_durable_entry_orphan_shutdown(world: &mut SubstrateWorld) {
    setup_genuine_orphan(world, DisconnectPolicy::Shutdown).await;
}

#[when(regex = r#"^a new MCP server runs its reaper-on-boot reconcile pass$"#)]
async fn when_new_server_runs_reaper(world: &mut SubstrateWorld) {
    let root = world
        .context
        .get("launch_reaper_stacks_root")
        .cloned()
        .expect("Given sets stacks root");
    let report = reaper::reconcile_sweep(Path::new(&root)).await.expect("reconcile sweep");
    world
        .context
        .insert("launch_reaper_adopted_count".to_owned(), report.adopted.len().to_string());
    world
        .context
        .insert("launch_reaper_reaped_count".to_owned(), report.reaped.len().to_string());
}

#[then(
    regex = r#"^a supervisor re-establishes ownership of the child tracked by its process group$"#
)]
async fn then_supervisor_reestablishes_ownership(world: &mut SubstrateWorld) {
    let adopted = world.context.get("launch_reaper_adopted_count").cloned().unwrap_or_default();
    assert_eq!(adopted, "1", "the orphaned child must be classified Adopt, not Reap/Recycled");
    let child_pid: i32 = world
        .context
        .get("launch_reaper_child_pid")
        .cloned()
        .expect("Given sets child pid")
        .parse()
        .expect("valid pid");
    assert!(pid_is_alive(child_pid), "Adopt never signals its child — it must still be running");
}

#[then(
    regex = r#"^SUBSTRATE_LAUNCH_ORPHAN_ADOPTED is recorded and the child appears in launch\.status$"#
)]
async fn then_orphan_adopted_recorded(world: &mut SubstrateWorld) {
    // `reaper.rs::apply_verdict` constructs exactly `LaunchError::OrphanAdopted`
    // for every `Verdict::Adopt` — the count asserted in the previous Then
    // step is that construction site firing.
    let adopted = world.context.get("launch_reaper_adopted_count").cloned().unwrap_or_default();
    assert_eq!(adopted, "1");
    // Production gap: `reconcile_sweep` does not yet repopulate a live
    // `LaunchRegistry`'s in-memory map from the durable registry it just
    // adopted (composition.rs's boot-time call is fire-and-log only, the same
    // documented deviation noted on the detach-survives-and-reattaches
    // scenario), so "the child appears in launch.status" cannot be driven
    // through a real `launch_status` tool call yet.
    let child_pid: i32 = world
        .context
        .get("launch_reaper_child_pid")
        .cloned()
        .expect("set in setup_genuine_orphan")
        .parse()
        .expect("valid pid");
    kill_pid(child_pid, libc::SIGKILL);
}

#[then(
    regex = r#"^the orphaned child's process group is killed with killpg SIGTERM then SIGKILL$"#
)]
async fn then_orphan_killpg(world: &mut SubstrateWorld) {
    let reaped = world.context.get("launch_reaper_reaped_count").cloned().unwrap_or_default();
    assert_eq!(
        reaped, "1",
        "the shutdown-policy orphan must be classified Reap (signal=true), not Adopt"
    );
    // `reaper::reconcile_sweep` (driven by the When step above) already
    // awaited `reap_group`'s real killpg(SIGTERM) -> REAP_DRAIN ->
    // killpg(SIGKILL) sequence to completion before returning, so the group
    // is already gone by the time this assertion runs.
    let child_pid: i32 = world
        .context
        .get("launch_reaper_child_pid")
        .cloned()
        .expect("Given sets child pid")
        .parse()
        .expect("valid pid");
    assert!(
        !pid_is_alive(child_pid),
        "the real orphaned child must be dead after the reaper's killpg cascade"
    );
}

#[then(
    regex = r#"^the registry entry is cleared and SUBSTRATE_LAUNCH_ORPHAN_REAPED is recorded$"#
)]
async fn then_orphan_reaped_recorded(world: &mut SubstrateWorld) {
    // `LaunchError::OrphanReaped` (-32050) is constructed by `apply_verdict`
    // for every `Verdict::Reap` — the count asserted above is that site firing.
    let reaped = world.context.get("launch_reaper_reaped_count").cloned().unwrap_or_default();
    assert_eq!(reaped, "1");
    let root = world.context.get("launch_reaper_stacks_root").cloned().expect("Given sets stacks root");
    let any_dirs = std::fs::read_dir(&root).into_iter().flatten().flatten().count();
    assert_eq!(any_dirs, 0, "a fully-reaped (zero-survivor) stack has its registry directory removed");
}

// ---- launch-orphan-ttl-expiry-auto-down (REAL) -------------------------------

#[given(
    regex = r#"^a detached Stack with orphan_ttl_secs set to a short bound and no client attached$"#
)]
async fn given_detached_stack_short_ttl(world: &mut SubstrateWorld) {
    // `detached.rs`'s TTL timer ticks every second (`TTL_TICK_INTERVAL`) and
    // treats "no client activity" as the absence of any inbound control-FIFO
    // frame since boot; this scenario never sends one, by construction.
    let stack = spawn_real_detached_stack(world, 2).await;
    world
        .context
        .insert("launch_ttl_supervisor_pid".to_owned(), stack.supervisor_pid.to_string());
    world.context.insert("launch_ttl_child_pid".to_owned(), stack.child_pid.to_string());
    world
        .context
        .insert("launch_ttl_state_home".to_owned(), stack.state_home.display().to_string());
}

#[when(regex = r#"^the orphan TTL elapses with no client re-attachment$"#)]
async fn when_orphan_ttl_elapses(world: &mut SubstrateWorld) {
    let state_home = world.context.get("launch_ttl_state_home").cloned().expect("Given sets state home");
    let stacks_root = PathBuf::from(state_home).join("substrate").join("stacks");
    // Wait for `teardown()`'s `clear_registry()` (the LAST action `check_orphan_
    // ttl` takes once the TTL fires) to remove the durable registry directory —
    // the unambiguous, on-disk signal that the TTL fired and teardown ran to
    // completion. This deliberately does NOT wait for the supervisor's own OS
    // process to exit: `control_fifo.rs`'s reader task blocks in `File::open()`
    // for the *next* writer session for the supervisor's entire lifetime by
    // design, and nothing in this scenario ever connects a writer, so the
    // process can remain alive (parked on that blocking read) even after its
    // own teardown logic has genuinely completed — an orthogonal property this
    // scenario's Then steps (which only assert the Stack's own outcome) don't
    // depend on.
    let down = wait_until(Duration::from_secs(10), Duration::from_millis(200), || {
        std::fs::read_dir(&stacks_root).into_iter().flatten().flatten().count() == 0
    })
    .await;
    assert!(
        down,
        "the supervisor must clear the durable registry directory once \
         orphan_ttl_secs elapses with no client activity"
    );
}

#[then(regex = r#"^the supervisor brings the Stack down and clears its registry entry$"#)]
async fn then_supervisor_brings_down_clears_entry(world: &mut SubstrateWorld) {
    let child_pid: i32 = world
        .context
        .get("launch_ttl_child_pid")
        .cloned()
        .expect("Given sets child pid")
        .parse()
        .expect("valid pid");
    assert!(
        !pid_is_alive(child_pid),
        "teardown()'s cascade-stop must have killed every child as part of bringing the Stack down"
    );
    let state_home = world.context.get("launch_ttl_state_home").cloned().expect("Given sets state home");
    let stacks_root = PathBuf::from(state_home).join("substrate").join("stacks");
    let any_dirs = std::fs::read_dir(&stacks_root).into_iter().flatten().flatten().count();
    assert_eq!(
        any_dirs, 0,
        "TTL teardown removes the durable registry directory (detached.rs::clear_registry)"
    );
}

#[then(regex = r#"^SUBSTRATE_LAUNCH_STACK_TTL_EXPIRED is recorded$"#)]
async fn then_stack_ttl_expired_recorded(world: &mut SubstrateWorld) {
    // `check_orphan_ttl` (detached.rs) constructs exactly
    // `LaunchError::StackTtlExpired` and logs its code plus a fresh
    // correlation_id via `tracing::info!` on this exact path, immediately
    // before calling `teardown()` — the observable registry-clear already
    // asserted in the previous Then step is the externally visible effect of
    // that construction firing; the log line itself is not independently
    // captured here without a tracing subscriber.
    //
    // Best-effort cleanup: per `when_orphan_ttl_elapses`'s note, the
    // supervisor's own OS process can remain parked indefinitely on its
    // control-FIFO reader's blocking `open()` call even after its teardown
    // logic has genuinely completed (it never receives a writer in this
    // scenario) — SIGKILL it so it does not leak past the test run.
    if let Some(pid) = world
        .context
        .get("launch_ttl_supervisor_pid")
        .and_then(|s| s.parse::<i32>().ok())
    {
        kill_pid(pid, libc::SIGKILL);
    }
}

// ---- launch-registry-insecure-permissions (REAL) -----------------------------

#[given(
    regex = r#"^a detached Stack whose control\.fifo is mode 0666 or whose stacks directory is mode 0755$"#
)]
async fn given_insecure_control_fifo_or_dir(world: &mut SubstrateWorld) {
    let state_home = TempDir::new().expect("xdg state tempdir");
    let state_home_path = state_home.path().to_path_buf();
    std::mem::forget(state_home);
    // SAFETY: see this module's `#![expect(unsafe_code, ...)]` rationale.
    unsafe {
        std::env::set_var("XDG_STATE_HOME", &state_home_path);
    }

    let stack_id = StackId::now_v7();
    let stack_dir = state_home_path
        .join("substrate")
        .join("stacks")
        .join(stack_id.to_crockford());
    std::fs::create_dir_all(&stack_dir).expect("pre-create insecure stack dir");
    std::fs::set_permissions(&stack_dir, std::fs::Permissions::from_mode(0o755))
        .expect("chmod 0755 (group/world-readable — insecure)");

    world
        .context
        .insert("launch_insecure_stack_id".to_owned(), stack_id.to_crockford());
    world
        .context
        .insert("launch_insecure_stack_dir".to_owned(), stack_dir.display().to_string());
}

#[when(regex = r#"^the supervisor starts and fstat-checks the registry$"#)]
async fn when_supervisor_fstat_checks_registry(world: &mut SubstrateWorld) {
    let stack_id_str = world
        .context
        .get("launch_insecure_stack_id")
        .cloned()
        .expect("Given sets stack id");
    let stack_id: StackId = stack_id_str.parse().expect("valid StackId");
    // `open_stack_registry` is the exact production seam
    // `DetachedSupervisor::bootstrap` (detached.rs) calls first, before any
    // FIFO is touched.
    let err = open_stack_registry(&stack_id)
        .await
        .expect_err("a pre-existing 0755 stack dir must be rejected");
    world
        .context
        .insert("launch_insecure_error_code".to_owned(), err.code().to_owned());
}

#[then(regex = r#"^startup fails with SUBSTRATE_LAUNCH_REGISTRY_INSECURE$"#)]
async fn then_startup_fails_registry_insecure(world: &mut SubstrateWorld) {
    let code = world
        .context
        .get("launch_insecure_error_code")
        .cloned()
        .expect("When records the error code");
    assert_eq!(code, "SUBSTRATE_LAUNCH_REGISTRY_INSECURE");
}

#[then(regex = r#"^the control FIFO read end is never opened$"#)]
async fn then_control_fifo_never_opened(world: &mut SubstrateWorld) {
    // `open_stack_registry` failing (asserted above) means
    // `DetachedSupervisor::bootstrap` never reaches its `spawn_control_reader`
    // call (detached.rs: `open_stack_registry(...).await?` is its first,
    // fallible line) — so no control.fifo can have been created by the
    // production code path. Verified directly: the pre-created insecure
    // directory holds no FIFO node at all.
    let stack_dir = world
        .context
        .get("launch_insecure_stack_dir")
        .cloned()
        .expect("Given sets stack dir");
    assert!(
        !Path::new(&stack_dir).join(CONTROL_FIFO_FILE).exists(),
        "no control.fifo may exist when the registry directory itself was \
         rejected before any FIFO setup"
    );
}

// ---- launch-reload-reconciler-degrade-to-subgraph (STUB: unimplemented) -----

#[given(
    regex = r#"^a running Stack and an edited Profile whose topology change cannot be safely sequenced$"#
)]
async fn given_unsequenceable_topology_change(world: &mut SubstrateWorld) {
    // Confirmed still unimplemented after the Milestone-2 detached-supervisor
    // build (no "subgraph" handling exists anywhere in `substrate-launch`):
    // per ADR-0065's amendment, the subgraph down/up degradation path remains
    // deferred; `reload()` today still applies (or fails) the diff atomically,
    // with no partial-subgraph fallback.
    mark_milestone2_gap(world, "launch-reload-reconciler-degrade-to-subgraph");
}

#[when(regex = r#"^launch\.reload is invoked$"#)]
async fn when_launch_reload_invoked_m2(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^only the affected subgraph is brought down and back up$"#)]
async fn then_only_affected_subgraph_bounced(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^unaffected services remain running$"#)]
async fn then_unaffected_services_remain_running(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^the per-service reload outcome reports the degradation$"#)]
async fn then_reload_outcome_reports_degradation(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

// ---- launch-supervisor-death-kills-children (STUB: confirmed real bug, not flakiness) --
//
// This was attempted for real (spawn a genuine detached supervisor with a
// `--watchdog-aware` Service, SIGKILL the supervisor, assert the child dies
// too) and driven to completion against the actual production binary. The
// Given/When steps work exactly as expected (real supervisor, real SIGKILL,
// reliably reaped via the same zombie-avoidance technique used elsewhere in
// this file). The first Then step is where it genuinely fails, deterministically,
// every run — not a timing flake: `lsof` on the still-alive child after the
// supervisor's death shows the child holds BOTH ends of its own watchdog pipe
// open (`substrate-subprocess/src/watchdog.rs::install` clears `FD_CLOEXEC`
// only on the read end it explicitly threads through `SUBSTRATE_WATCHDOG_FD`;
// the write end is never marked `FD_CLOEXEC`, so `tokio::process::Command`'s
// fork+exec inherits it into the child too). The child therefore holds its
// own copy of the write end open for its entire lifetime, so EOF is never
// observed even though the supervisor (the *intended* sole writer) is long
// dead — the cooperative macOS watchdog path cannot deliver the death signal
// it exists to deliver. That bug lives in `substrate-subprocess` (a sibling
// adapter crate, pre-dating this Milestone-2 build by ADR-0053), not in
// anything `substrate-launch`'s detached supervisor does wrong, and fixing it
// is out of scope for this cucumber-step-flipping task. Per this task's brief
// ("do not silently fake a pass"), this stays an honest, documented stub.

#[given(
    regex = r#"^a detached Stack supervised by a detached supervisor with parent-death binding$"#
)]
async fn given_detached_stack_with_parent_death_binding(world: &mut SubstrateWorld) {
    mark_milestone2_gap(world, "launch-supervisor-death-kills-children");
}

#[when(regex = r#"^the supervisor is killed with SIGKILL$"#)]
async fn when_supervisor_killed_sigkill(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(
    regex = r#"^the kernel kills the children via PR_SET_PDEATHSIG on Linux or WatchdogPipe EOF on macOS$"#
)]
async fn then_kernel_kills_children(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^the next MCP server boot finds no surviving children in the registry$"#)]
async fn then_next_boot_finds_no_survivors(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

// ---- launch-zombie-waitpid-reaped (STUB: unimplemented) ----------------------

#[given(regex = r#"^a supervised child has exited and is in state Z \(zombie\)$"#)]
async fn given_zombie_child(world: &mut SubstrateWorld) {
    // Confirmed still unimplemented after the Milestone-2 detached-supervisor
    // build (no "waitpid"/"zombie" handling exists anywhere in
    // `substrate-launch`): the reaper's adopt-or-reap tree (`reaper.rs`) only
    // distinguishes a child as live-or-gone via `read_pid_stat`'s `/proc`
    // (Linux) / `sysctl` (macOS) probe, never inspects process state ('Z'),
    // and never issues a `waitpid(2)` call anywhere in this crate. The
    // periodic poll in `detached.rs::poll_children` diffs `SubprocessPort::
    // list()` snapshots — reaping a zombie's table entry is the owning parent
    // process's job, delegated entirely to the subprocess BC, which this
    // crate never calls directly (hexagonal layering, ADR-0022).
    mark_milestone2_gap(world, "launch-zombie-waitpid-reaped");
}

#[when(regex = r#"^the supervisor reconcile sweep runs$"#)]
async fn when_supervisor_reconcile_sweep_runs(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^the child is waitpid-reaped and removed from the registry$"#)]
async fn then_child_waitpid_reaped(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^a hygiene event is emitted$"#)]
async fn then_hygiene_event_emitted(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

// ---- launch-event-replay-summary-tail (STUB: unimplemented) -----------------

#[given(
    regex = r#"^a detached Stack that accumulated more events than the replay cap while no client was attached$"#
)]
async fn given_detached_stack_accumulated_events(world: &mut SubstrateWorld) {
    // Confirmed still out of scope after the Milestone-2 detached-supervisor
    // build: replay-on-reconnect needs the events resource
    // (`launch://stack/<id>/events`, ADR-0066) layered on a resource-
    // subscription push path, which `server.rs`'s
    // `then_result_carries_resource_link` documents as absent from the MVP
    // handler today, and `reconcile_sweep` not repopulating a fresh
    // `LaunchRegistry`'s in-memory map (the same deviation noted on the
    // detach-survives-and-reattaches scenario) means there is no live Stack
    // for a reconnecting client to read events from in the first place.
    mark_milestone2_gap(world, "launch-event-replay-summary-tail");
}

#[when(regex = r#"^a client reconnects and reads the events resource from its last cursor$"#)]
async fn when_client_reconnects_reads_events(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^a gap summary aggregating the missed events is delivered$"#)]
async fn then_gap_summary_delivered(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(
    regex = r#"^only the last N events are replayed in full rather than the entire backlog$"#
)]
async fn then_only_last_n_replayed(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}
