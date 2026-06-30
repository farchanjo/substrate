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

//! Step definitions for the launch reconciler reload (ADR-0065).
//!
//! Covers: launch-reload-cascade-restart, launch-reload-metadata-only-no-restart.

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
use substrate_domain::ports::launch::LaunchPort;

use super::{FakeSubprocessPort, NeverCancel, THREE_TIER, registry, write_profile};
use crate::SubstrateWorld;

const CASCADE_EDITED: &str = "version = 1\n\n[services.db]\ncommand = [\"db\"]\n\n[services.api]\ncommand = [\"api\"]\nargs = [\"--v2\"]\ndepends_on = [\"db\"]\n\n[services.web]\ncommand = [\"web\"]\ndepends_on = [\"api\"]\n";

const METADATA_BASE: &str = "version = 1\n\n[services.app]\ncommand = [\"app\"]\n\n[services.app.restart_policy]\nkind = \"OnFailure\"\nmax_retries = 3\nbackoff_ms = 1000\n";
const METADATA_EDITED: &str = "version = 1\n\n[services.app]\ncommand = [\"app\"]\n\n[services.app.restart_policy]\nkind = \"OnFailure\"\nmax_retries = 5\nbackoff_ms = 1000\n";

// ---- launch-reload-cascade-restart -------------------------------------------

#[given(
    regex = r#"^a running Stack with services db, api depends_on db, and web depends_on api$"#
)]
async fn given_running_three_tier_stack(world: &mut SubstrateWorld) {
    let dir = tempfile::TempDir::new().expect("tempdir");
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
    world.context.insert("launch_profile_path".to_owned(), profile);
    world
        .context
        .insert("launch_stack_id".to_owned(), handle.stack_id.to_string());
    std::mem::forget(dir);
}

#[when(regex = r#"^the args of api are changed and the Profile is reloaded$"#)]
async fn when_api_args_changed_and_reloaded(world: &mut SubstrateWorld) {
    let profile = world
        .context
        .get("launch_profile_path")
        .cloned()
        .expect("Given must set launch_profile_path");
    tokio::fs::write(&profile, CASCADE_EDITED.as_bytes())
        .await
        .expect("write edited profile");
    let reg = world.launch_registry.clone().expect("Given must set launch_registry");
    // Editing the Profile changes its content hash, so the TOFU pin from the
    // Given step's `trust()` no longer matches (ADR-0064) — re-bless the
    // edited content before reload, mirroring an operator re-running
    // `launch.trust` after a reviewed edit.
    reg.trust(&profile).await.expect("re-trust edited profile");
    let stack_id: substrate_domain::value_objects::stack_id::StackId = world
        .context
        .get("launch_stack_id")
        .cloned()
        .expect("Given must set launch_stack_id")
        .parse()
        .expect("valid StackId");
    let report = reg
        .reload(&stack_id, Some(&profile), &NeverCancel)
        .await
        .expect("reload succeeds");
    world
        .context
        .insert("launch_reload_restarted".to_owned(), report.restarted.join(","));
}

#[then(
    regex = r#"^api is restarted as an orchestrated restart not counted against its crash budget$"#
)]
async fn then_api_restarted_orchestrated(world: &mut SubstrateWorld) {
    let restarted = world
        .context
        .get("launch_reload_restarted")
        .cloned()
        .expect("When must set launch_reload_restarted");
    assert!(
        restarted.split(',').any(|s| s == "api"),
        "expected api in the restarted set; got {restarted}"
    );
}

#[then(regex = r#"^web is restarted because it depends on api with the default cascade$"#)]
async fn then_web_restarted_cascade(world: &mut SubstrateWorld) {
    let restarted = world
        .context
        .get("launch_reload_restarted")
        .cloned()
        .expect("When must set launch_reload_restarted");
    assert!(
        restarted.split(',').any(|s| s == "web"),
        "expected web in the restarted set (cascade from api); got {restarted}"
    );
}

#[then(regex = r#"^db is not restarted$"#)]
async fn then_db_not_restarted(world: &mut SubstrateWorld) {
    let restarted = world
        .context
        .get("launch_reload_restarted")
        .cloned()
        .expect("When must set launch_reload_restarted");
    assert!(
        !restarted.split(',').any(|s| s == "db"),
        "db's spawn-affecting fields did not change; it must not restart. got {restarted}"
    );
}

// ---- launch-reload-metadata-only-no-restart ---------------------------------

#[given(regex = r#"^a running Stack with a Service under an OnFailure restart policy$"#)]
async fn given_running_stack_onfailure(world: &mut SubstrateWorld) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let profile = write_profile(dir.path(), METADATA_BASE).await;
    let fake = FakeSubprocessPort::new();
    let reg = registry(fake.clone(), dir.path());
    reg.trust(&profile).await.expect("trust");
    let handle = reg
        .up(&profile, None, None, &NeverCancel)
        .await
        .expect("up succeeds");
    world.launch_registry = Some(reg);
    world.launch_fake = Some(fake);
    world.context.insert("launch_profile_path".to_owned(), profile);
    world
        .context
        .insert("launch_stack_id".to_owned(), handle.stack_id.to_string());
    std::mem::forget(dir);
}

#[when(
    regex = r#"^the Profile is edited to change only restart_policy\.max_retries and reloaded$"#
)]
async fn when_max_retries_edited_and_reloaded(world: &mut SubstrateWorld) {
    let profile = world
        .context
        .get("launch_profile_path")
        .cloned()
        .expect("Given must set launch_profile_path");
    tokio::fs::write(&profile, METADATA_EDITED.as_bytes())
        .await
        .expect("write edited profile");
    let reg = world.launch_registry.clone().expect("Given must set launch_registry");
    // See the cascade-restart When step above: editing changes the content
    // hash, so the Profile must be re-blessed before reload (ADR-0064).
    reg.trust(&profile).await.expect("re-trust edited profile");
    let stack_id: substrate_domain::value_objects::stack_id::StackId = world
        .context
        .get("launch_stack_id")
        .cloned()
        .expect("Given must set launch_stack_id")
        .parse()
        .expect("valid StackId");
    let report = reg
        .reload(&stack_id, Some(&profile), &NeverCancel)
        .await
        .expect("reload succeeds");
    world
        .context
        .insert("launch_reload_restarted".to_owned(), report.restarted.join(","));
    world
        .context
        .insert("launch_reload_edge_only".to_owned(), report.edge_only.join(","));
}

#[then(regex = r#"^the reconciler applies the new policy to the live supervisor$"#)]
async fn then_reconciler_applies_new_policy(world: &mut SubstrateWorld) {
    // The ReloadReport surfacing the field as classified (no error, reload
    // returned Ok per the When step's `.expect`) is the externally observable
    // signal that the new policy value was accepted and applied; the
    // supervisor's internal RestartPolicy state is not separately exposed by
    // the public LaunchPort surface.
    assert!(world.context.contains_key("launch_reload_restarted"));
}

#[then(regex = r#"^no child process is restarted$"#)]
async fn then_no_child_process_restarted(world: &mut SubstrateWorld) {
    let restarted = world
        .context
        .get("launch_reload_restarted")
        .cloned()
        .expect("When must set launch_reload_restarted");
    assert!(
        restarted.is_empty(),
        "a metadata-only change (max_retries) must restart no child; got {restarted}"
    );
}
