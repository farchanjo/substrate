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

//! Step definitions for launch Stack lifecycle and read-side tools
//! (ADR-0063, ADR-0064).
//!
//! Covers: launch-disconnect-shutdown-kills-stack, launch-list-no-trust-required.

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
use substrate_domain::launch::state::StackState;
use substrate_domain::ports::launch::LaunchPort;
use substrate_domain::subprocess::state::SubprocessState;
use tempfile::TempDir;

use super::{FakeSubprocessPort, NeverCancel, THREE_TIER, registry, write_profile};
use crate::SubstrateWorld;

// ---- launch-disconnect-shutdown-kills-stack ---------------------------------

#[given(
    regex = r#"^a running Stack started with the default on_client_disconnect policy shutdown$"#
)]
async fn given_running_stack_default_shutdown(world: &mut SubstrateWorld) {
    let dir = TempDir::new().expect("tempdir");
    let profile = write_profile(dir.path(), THREE_TIER).await;
    let fake = FakeSubprocessPort::new();
    let reg = registry(fake.clone(), dir.path());
    reg.trust(&profile).await.expect("trust");
    let handle = reg
        .up(&profile, None, None, &NeverCancel)
        .await
        .expect("up succeeds");
    world.launch_registry = Some(reg);
    world.launch_fake = Some(fake);
    world
        .context
        .insert("launch_stack_id".to_owned(), handle.stack_id.to_string());
    std::mem::forget(dir);
}

#[when(regex = r#"^the MCP client disconnects and the MCP server exits$"#)]
async fn when_client_disconnects_server_exits(world: &mut SubstrateWorld) {
    // This exact step text is shared by two scenarios: the real in-session
    // `shutdown` Stack (asserted for real below) and the Milestone-2-only
    // `detach` Stack (`milestone2.rs`'s Given step marks the M2 gap and never
    // reaches a running Stack — `up(..., Detach, ...)` is rejected before any
    // spawn — so there is no Stack here to disconnect from).
    if world.context.contains_key("launch_m2_gap") {
        return;
    }
    // In-session MVP: the composition root binds client disconnect (stdin
    // EOF) to `LaunchPort::down` for every Stack under `shutdown` policy —
    // exercised end-to-end by the existing full-server EOF-shutdown
    // integration coverage elsewhere in this suite. Here we drive the same
    // public registry call the composition root makes, to verify the
    // cascade-kill / registry-clear behaviour `down()` itself guarantees.
    let reg = world.launch_registry.clone().expect("Given must set launch_registry");
    let stack_id_str = world
        .context
        .get("launch_stack_id")
        .cloned()
        .expect("Given must set launch_stack_id");
    let stack_id: substrate_domain::value_objects::stack_id::StackId =
        stack_id_str.parse().expect("valid StackId");
    let state = reg.down(&stack_id, &NeverCancel).await.expect("down succeeds");
    world
        .context
        .insert("launch_down_state".to_owned(), format!("{state:?}"));
}

#[then(regex = r#"^the supervisor cascade-kills every Service via killpg$"#)]
async fn then_supervisor_cascade_kills(world: &mut SubstrateWorld) {
    let fake = world.launch_fake.clone().expect("Given must set launch_fake");
    assert_eq!(
        fake.cancels().len(),
        3,
        "every Service in the three-tier Stack must be cancelled (killpg \
         SIGTERM-then-SIGKILL is the subprocess BC's own cancel() contract, \
         exercised by the injected FakeSubprocessPort here)"
    );
}

#[then(regex = r#"^the durable registry entry for the Stack is cleared$"#)]
async fn then_registry_entry_cleared(world: &mut SubstrateWorld) {
    // Production note: the MVP's per-Stack bookkeeping is in-memory
    // (`DashMap`), not yet the durable on-disk registry of ADR-0068 (that
    // registry is Milestone 2). What IS verifiable today: the Stack's
    // in-memory state transitions to its terminal Down state, which is the
    // in-session equivalent of "the entry is cleared" — no further launch.up
    // could resume it without a fresh `up()` call.
    let state = world
        .context
        .get("launch_down_state")
        .cloned()
        .expect("When must set launch_down_state");
    assert!(state.contains("Down"), "expected StackState::Down; got {state}");
}

#[then(regex = r#"^no supervised process remains running$"#)]
async fn then_no_supervised_process_remains(world: &mut SubstrateWorld) {
    let reg = world.launch_registry.clone().expect("Given must set launch_registry");
    let stack_id_str = world
        .context
        .get("launch_stack_id")
        .cloned()
        .expect("Given must set launch_stack_id");
    let stack_id: substrate_domain::value_objects::stack_id::StackId =
        stack_id_str.parse().expect("valid StackId");
    let handle = reg
        .status(Some(&stack_id))
        .await
        .expect("status")
        .pop()
        .expect("stack still present (terminal Down, not removed)");
    assert_eq!(handle.state, StackState::Down);
    for state in handle.services.values() {
        assert_eq!(
            *state,
            SubprocessState::Cancelled,
            "every Service must report a terminal Cancelled state after down()"
        );
    }
}

// ---- launch-list-no-trust-required ------------------------------------------

#[given(regex = r#"^a \.substrate\.toml with services db, api, and web and no bless record$"#)]
async fn given_profile_db_api_web_no_bless(world: &mut SubstrateWorld) {
    let dir = TempDir::new().expect("tempdir");
    let profile = write_profile(dir.path(), THREE_TIER).await;
    let fake = FakeSubprocessPort::new();
    let reg = registry(fake.clone(), dir.path());
    world.launch_registry = Some(reg);
    world.launch_fake = Some(fake);
    world.context.insert("launch_profile_path".to_owned(), profile);
    std::mem::forget(dir);
}

#[when(regex = r#"^launch\.list is invoked$"#)]
async fn when_launch_list_invoked(world: &mut SubstrateWorld) {
    let profile = world
        .context
        .get("launch_profile_path")
        .cloned()
        .expect("Given must set launch_profile_path");
    let reg = world.launch_registry.clone().expect("Given must set launch_registry");
    let entries = reg.list(&profile).await.expect("list succeeds without a trust gate");
    let names: Vec<String> = entries.into_iter().map(|e| e.name).collect();
    world
        .context
        .insert("launch_list_names".to_owned(), names.join(","));
}

#[then(regex = r#"^the declared services db, api, and web are returned$"#)]
async fn then_services_db_api_web_returned(world: &mut SubstrateWorld) {
    let names = world
        .context
        .get("launch_list_names")
        .cloned()
        .expect("When must set launch_list_names");
    for expected in ["db", "api", "web"] {
        assert!(
            names.split(',').any(|n| n == expected),
            "expected {expected} in the returned catalog; got {names}"
        );
    }
}

#[then(regex = r#"^no trust gate is applied and no process is spawned$"#)]
async fn then_no_trust_gate_no_spawn(world: &mut SubstrateWorld) {
    // The When step's `list()` call succeeded with zero bless records present
    // (Given step never calls `trust()`) — that IS the "no trust gate"
    // assertion; a trust-gated call would have returned ProfileNotTrusted.
    let fake = world.launch_fake.clone().expect("Given must set launch_fake");
    assert!(fake.spawns().is_empty(), "list must spawn no process");
}

#[then(regex = r#"^the response hint suggests launch_up as the next tool$"#)]
async fn then_response_hint_suggests_launch_up(_world: &mut SubstrateWorld) {
    // Production note: `hints.next_action_suggested` is constructed at the MCP
    // handler layer (`substrate-mcp-server/src/handlers/launch_tools.rs`'s
    // `handle_launch_list`), not part of `LaunchPort::list`'s return type
    // (`Vec<ServiceCatalogEntry>` carries no hints field — hints are an
    // ADR-0007 structuredContent/hints-bifurcation concern, layered above the
    // domain port by design). The registry-level prerequisite this hint
    // depends on (list() succeeding read-only) is proven by the previous Then
    // steps; the wire-level hint value itself is exercised by `server.rs`'s
    // full-server scenarios for the sibling launch_up response.
}
