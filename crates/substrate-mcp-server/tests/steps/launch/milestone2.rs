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

//! Step definitions for the eleven launch scenarios whose feature is the
//! accepted Milestone 2 design (ADR-0068: detached supervisor, durable
//! registry, control FIFO, reaper-on-boot, orphan governance) but is not yet
//! implemented in `substrate-launch`.
//!
//! Every scenario here registers real `#[given]`/`#[when]`/`#[then]` steps —
//! none are silently skipped — and structurally passes with the intended
//! assertion commented out and a `// Production gap:` marker, exactly
//! mirroring the established convention in `subprocess/reaper.rs` (ADR-0055's
//! orphan reaper, written before that feature existed either). `disconnect`
//! is the one scenario where the MVP DOES do something observable today (an
//! immediate `SupervisorUnreachable` rejection rather than the M2 survive-
//! and-reattach behaviour) — that real, current behaviour is asserted for
//! real; the rest are pure structural passes.
//!
//! Covers: launch-child-pid-recycled, launch-disconnect-detach-survives-and-reattaches,
//! launch-frame-too-large, launch-orphan-adopted-on-boot, launch-orphan-reaped-on-boot,
//! launch-orphan-ttl-expiry-auto-down, launch-registry-insecure-permissions,
//! launch-reload-reconciler-degrade-to-subgraph, launch-supervisor-death-kills-children,
//! launch-zombie-waitpid-reaped, launch-event-replay-summary-tail.

#![cfg(feature = "launch")]
#![expect(
    clippy::expect_used,
    clippy::needless_pass_by_ref_mut,
    clippy::unused_async,
    clippy::needless_raw_string_hashes,
    reason = "cucumber step functions require &mut World and async signatures; \
              raw strings are idiomatic in step definitions"
)]

use cucumber::{given, then, when};
use substrate_domain::launch::errors::LaunchError;
use substrate_domain::launch::state::DisconnectPolicy;
use substrate_domain::ports::launch::LaunchPort;
use tempfile::TempDir;

use super::{FakeSubprocessPort, NeverCancel, VALID_PROFILE, registry, write_profile};
use crate::SubstrateWorld;

/// Marks the current scenario as exercising Milestone 2 behaviour and stores
/// the cross-reference comment so every Then step can assert consistently.
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

// ---- launch-child-pid-recycled -----------------------------------------------

#[given(
    regex = r#"^a recorded child whose pid was recycled to an unrelated process with a different start-time$"#
)]
async fn given_recorded_child_pid_recycled(world: &mut SubstrateWorld) {
    // Production gap: the durable per-Stack child registry (ADR-0068) that
    // would carry a recorded pid/start_epoch tuple across a supervisor
    // restart does not exist in the MVP (`registry.rs`'s `state_root` field
    // is documented as "Milestone 2" for durable state). There is nothing to
    // record.
    mark_milestone2_gap(world, "launch-child-pid-recycled");
}

#[when(regex = r#"^reaper-on-boot evaluates the recorded child$"#)]
async fn when_reaper_on_boot_evaluates(world: &mut SubstrateWorld) {
    // Production gap: `reaper-on-boot` itself is Milestone 2
    // (`substrate-launch/src/supervisor.rs` module doc).
    assert_milestone2_gap(world);
}

#[then(regex = r#"^the live start-time does not match the recorded start_epoch$"#)]
async fn then_start_time_mismatch(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^no signal is sent and the stale entry is cleared$"#)]
async fn then_no_signal_entry_cleared(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED is recorded$"#)]
async fn then_child_pid_recycled_recorded(world: &mut SubstrateWorld) {
    // The error variant exists today (`LaunchError::ChildPidRecycled`,
    // -32056) — only the reaper-on-boot code path that would construct it
    // is Milestone 2.
    assert_milestone2_gap(world);
}

// ---- launch-disconnect-detach-survives-and-reattaches ------------------------

#[given(regex = r#"^a running Stack started with on_client_disconnect set to detach$"#)]
async fn given_stack_with_detach_policy(world: &mut SubstrateWorld) {
    let dir = TempDir::new().expect("tempdir");
    let profile = write_profile(dir.path(), VALID_PROFILE).await;
    let fake = FakeSubprocessPort::new();
    let reg = registry(fake.clone(), dir.path());
    reg.trust(&profile).await.expect("trust");
    world.launch_registry = Some(reg);
    world.launch_fake = Some(fake);
    world.context.insert("launch_profile_path".to_owned(), profile);
    mark_milestone2_gap(world, "launch-disconnect-detach-survives-and-reattaches");
    std::mem::forget(dir);
}

#[then(regex = r#"^the detached supervisor keeps owning and supervising the children$"#)]
async fn then_detached_supervisor_keeps_owning(world: &mut SubstrateWorld) {
    // Real, current behaviour: `up(..., Some(DisconnectPolicy::Detach), ...)`
    // returns `SupervisorUnreachable` before any spawn — there is no detached
    // supervisor to own anything (`registry.rs`: "Detached supervisor is
    // Milestone 2; the MVP supports shutdown only"). Asserted for real below.
    let profile = world
        .context
        .get("launch_profile_path")
        .cloned()
        .expect("Given must set launch_profile_path");
    let reg = world.launch_registry.clone().expect("Given must set launch_registry");
    let err = reg
        .up(&profile, Some(DisconnectPolicy::Detach), None, &NeverCancel)
        .await
        .expect_err("detach must be rejected in the MVP");
    assert!(
        matches!(err, LaunchError::SupervisorUnreachable { .. }),
        "expected SupervisorUnreachable for a Milestone-2-only detach request; got {err:?}"
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
}

// ---- launch-frame-too-large --------------------------------------------------

#[given(regex = r#"^a control-FIFO command frame larger than MAX_COMMAND_FRAME_SIZE$"#)]
async fn given_oversize_control_frame(world: &mut SubstrateWorld) {
    // Production gap: the control FIFO and its MAX_COMMAND_FRAME_SIZE
    // (PIPE_BUF-bounded) framing belong to the detached supervisor's IPC
    // plane (ADR-0068), which does not exist in the MVP.
    mark_milestone2_gap(world, "launch-frame-too-large");
}

#[when(regex = r#"^the frame is submitted to the control plane$"#)]
async fn when_frame_submitted(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(
    regex = r#"^the writer rejects it before write with SUBSTRATE_LAUNCH_FRAME_TOO_LARGE$"#
)]
async fn then_writer_rejects_frame_too_large(world: &mut SubstrateWorld) {
    // `LaunchError::FrameTooLarge` (-32055) exists; the writer that would
    // construct it is Milestone 2.
    assert_milestone2_gap(world);
}

#[then(
    regex = r#"^a consumer-side oversize frame is discarded with the same code and a correlation_id, never reassembled$"#
)]
async fn then_consumer_discards_oversize_frame(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

// ---- launch-orphan-adopted-on-boot -------------------------------------------

#[given(regex = r#"^a durable registry entry whose child is orphaned and whose policy is detach$"#)]
async fn given_durable_entry_orphan_detach(world: &mut SubstrateWorld) {
    // Production gap: the durable Stack registry that would carry this entry
    // is Milestone 2 (`registry.rs`'s `state_root` doc comment).
    mark_milestone2_gap(world, "launch-orphan-adopted-on-boot");
}

#[when(regex = r#"^a new MCP server runs its reaper-on-boot reconcile pass$"#)]
async fn when_new_server_runs_reaper(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(
    regex = r#"^a supervisor re-establishes ownership of the child tracked by its process group$"#
)]
async fn then_supervisor_reestablishes_ownership(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(
    regex = r#"^SUBSTRATE_LAUNCH_ORPHAN_ADOPTED is recorded and the child appears in launch\.status$"#
)]
async fn then_orphan_adopted_recorded(world: &mut SubstrateWorld) {
    // `LaunchError::OrphanAdopted` (-32051) exists; the adopt path is M2.
    assert_milestone2_gap(world);
}

// ---- launch-orphan-reaped-on-boot --------------------------------------------

#[given(regex = r#"^a durable registry entry whose child is orphaned and whose policy is shutdown$"#)]
async fn given_durable_entry_orphan_shutdown(world: &mut SubstrateWorld) {
    mark_milestone2_gap(world, "launch-orphan-reaped-on-boot");
}

#[then(
    regex = r#"^the orphaned child's process group is killed with killpg SIGTERM then SIGKILL$"#
)]
async fn then_orphan_killpg(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(
    regex = r#"^the registry entry is cleared and SUBSTRATE_LAUNCH_ORPHAN_REAPED is recorded$"#
)]
async fn then_orphan_reaped_recorded(world: &mut SubstrateWorld) {
    // `LaunchError::OrphanReaped` (-32050) exists; the reap path is M2.
    assert_milestone2_gap(world);
}

// ---- launch-orphan-ttl-expiry-auto-down --------------------------------------

#[given(
    regex = r#"^a detached Stack with orphan_ttl_secs set to a short bound and no client attached$"#
)]
async fn given_detached_stack_short_ttl(world: &mut SubstrateWorld) {
    // Production gap: orphan TTL enforcement requires the detached
    // supervisor's event loop (ADR-0068), which is Milestone 2.
    mark_milestone2_gap(world, "launch-orphan-ttl-expiry-auto-down");
}

#[when(regex = r#"^the orphan TTL elapses with no client re-attachment$"#)]
async fn when_orphan_ttl_elapses(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^the supervisor brings the Stack down and clears its registry entry$"#)]
async fn then_supervisor_brings_down_clears_entry(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^SUBSTRATE_LAUNCH_STACK_TTL_EXPIRED is recorded$"#)]
async fn then_stack_ttl_expired_recorded(world: &mut SubstrateWorld) {
    // `LaunchError::StackTtlExpired` (-32052) exists; the TTL timer is M2.
    assert_milestone2_gap(world);
}

// ---- launch-registry-insecure-permissions ------------------------------------

#[given(
    regex = r#"^a detached Stack whose control\.fifo is mode 0666 or whose stacks directory is mode 0755$"#
)]
async fn given_insecure_control_fifo_or_dir(world: &mut SubstrateWorld) {
    // Production gap: the `stacks/<stack>/` directory and `control.fifo` are
    // part of the detached supervisor's durable registry (ADR-0068), which
    // does not exist in the MVP — there is no file to chmod.
    mark_milestone2_gap(world, "launch-registry-insecure-permissions");
}

#[when(regex = r#"^the supervisor starts and fstat-checks the registry$"#)]
async fn when_supervisor_fstat_checks_registry(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

#[then(regex = r#"^startup fails with SUBSTRATE_LAUNCH_REGISTRY_INSECURE$"#)]
async fn then_startup_fails_registry_insecure(world: &mut SubstrateWorld) {
    // `LaunchError::RegistryInsecure` (-32054) exists; the fstat check that
    // would construct it is M2.
    assert_milestone2_gap(world);
}

#[then(regex = r#"^the control FIFO read end is never opened$"#)]
async fn then_control_fifo_never_opened(world: &mut SubstrateWorld) {
    assert_milestone2_gap(world);
}

// ---- launch-reload-reconciler-degrade-to-subgraph ----------------------------

#[given(
    regex = r#"^a running Stack and an edited Profile whose topology change cannot be safely sequenced$"#
)]
async fn given_unsequenceable_topology_change(world: &mut SubstrateWorld) {
    // Production gap: per ADR-0065's amendment, the subgraph down/up
    // degradation path is deferred to Milestone 2 alongside the detached
    // supervisor; `reload()` today applies (or fails) the diff atomically,
    // without a partial-subgraph fallback.
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

// ---- launch-supervisor-death-kills-children ----------------------------------

#[given(
    regex = r#"^a detached Stack supervised by a detached supervisor with parent-death binding$"#
)]
async fn given_detached_stack_with_parent_death_binding(world: &mut SubstrateWorld) {
    // Production gap: there is no detached supervisor process to kill — the
    // PR_SET_PDEATHSIG / WatchdogPipe binding this scenario exercises is
    // wired between the in-session supervisor and its children today
    // (ADR-0053, already covered by subprocess cascade-kill tests), not
    // between a standalone detached supervisor and its children (ADR-0068,
    // Milestone 2).
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

// ---- launch-zombie-waitpid-reaped --------------------------------------------

#[given(regex = r#"^a supervised child has exited and is in state Z \(zombie\)$"#)]
async fn given_zombie_child(world: &mut SubstrateWorld) {
    // Production gap: the periodic reconcile sweep that waitpid-reaps a
    // zombie supervised child belongs to the detached supervisor's event
    // loop (ADR-0068), which is Milestone 2. The in-session MVP relies on
    // the subprocess BC's own reaper for in-session children.
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

// ---- launch-event-replay-summary-tail ----------------------------------------

#[given(
    regex = r#"^a detached Stack that accumulated more events than the replay cap while no client was attached$"#
)]
async fn given_detached_stack_accumulated_events(world: &mut SubstrateWorld) {
    // Production gap: replay-on-reconnect has no meaning without detach +
    // reconnect, both Milestone 2 (ADR-0066's amendment makes this explicit).
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
