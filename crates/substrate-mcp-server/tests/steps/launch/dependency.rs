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

//! Step definitions for the launch dependency graph and readiness gating
//! (ADR-0065).
//!
//! Covers: launch-depends-on-cycle-rejected,
//! launch-dependency-readiness-timeout-fails-dependents,
//! launch-optional-dependency-fails-without-blocking,
//! launch-up-readiness-gated-start.
//!
//! The shared `When launch.up is invoked for the Stack` / `Then the call
//! returns SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED` / `Then no child process is
//! spawned` steps used by several of these scenarios are registered once in
//! `trust.rs` and apply here too — cucumber matches steps by regex against
//! the Gherkin text, not per feature file.

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
use substrate_domain::launch::state::StackState;
use substrate_domain::ports::launch::LaunchPort;
use substrate_domain::subprocess::state::SubprocessState;
use tempfile::TempDir;

use super::{FakeSubprocessPort, NeverCancel, THREE_TIER, registry, write_profile};
use crate::SubstrateWorld;

// ---- launch-depends-on-cycle-rejected ---------------------------------------

#[given(regex = r#"^a Profile where service a depends_on b and service b depends_on a$"#)]
async fn given_cyclic_profile(world: &mut SubstrateWorld) {
    let dir = TempDir::new().expect("tempdir");
    let body = "version = 1\n\n[services.a]\ncommand = [\"a\"]\ndepends_on = [\"b\"]\n\n[services.b]\ncommand = [\"b\"]\ndepends_on = [\"a\"]\n";
    let profile = write_profile(dir.path(), body).await;
    let fake = FakeSubprocessPort::new();
    let reg = registry(fake.clone(), dir.path());
    reg.trust(&profile).await.expect("trust");
    world.launch_registry = Some(reg);
    world.launch_fake = Some(fake);
    world.context.insert("launch_profile_path".to_owned(), profile);
    std::mem::forget(dir);
}

#[when(regex = r#"^launch\.up validates the dependency graph$"#)]
async fn when_up_validates_dependency_graph(world: &mut SubstrateWorld) {
    let profile = world
        .context
        .get("launch_profile_path")
        .cloned()
        .expect("Given must set launch_profile_path");
    let reg = world.launch_registry.clone().expect("Given must set launch_registry");
    let result = reg.up(&profile, None, None, &NeverCancel).await;
    world
        .context
        .insert("launch_up_result".to_owned(), format!("{result:?}"));
}

#[then(regex = r#"^the call returns SUBSTRATE_LAUNCH_CYCLE_DETECTED$"#)]
async fn then_returns_cycle_detected(world: &mut SubstrateWorld) {
    let result = world
        .context
        .get("launch_up_result")
        .cloned()
        .expect("When must set launch_up_result");
    assert!(
        result.contains("CycleDetected"),
        "expected SUBSTRATE_LAUNCH_CYCLE_DETECTED; got {result}"
    );
}

#[then(regex = r#"^no Service is spawned$"#)]
async fn then_no_service_spawned(world: &mut SubstrateWorld) {
    let fake = world.launch_fake.clone().expect("Given must set launch_fake");
    assert!(fake.spawns().is_empty());
}

// ---- launch-dependency-readiness-timeout-fails-dependents -------------------

#[given(
    regex = r#"^service api depends_on db with required=true and db never reaches Ready within its probe budget$"#
)]
async fn given_required_dependency_fails(world: &mut SubstrateWorld) {
    let dir = TempDir::new().expect("tempdir");
    let body =
        "version = 1\n\n[services.db]\ncommand = [\"db\"]\n\n[services.api]\ncommand = [\"api\"]\ndepends_on = [\"db\"]\nrequired = true\n";
    let profile = write_profile(dir.path(), body).await;
    let fake = FakeSubprocessPort::new();
    fake.script("db", SubprocessState::Failed);
    let reg = registry(fake.clone(), dir.path());
    reg.trust(&profile).await.expect("trust");
    world.launch_registry = Some(reg);
    world.launch_fake = Some(fake);
    world.context.insert("launch_profile_path".to_owned(), profile);
    std::mem::forget(dir);
}

#[then(
    regex = r#"^api is not started and the call returns SUBSTRATE_LAUNCH_DEPENDENCY_FAILED$"#
)]
async fn then_api_not_started_dependency_failed(world: &mut SubstrateWorld) {
    let result = world
        .context
        .get("launch_up_result")
        .cloned()
        .expect("When must set launch_up_result");
    assert!(
        result.contains("DependencyFailed"),
        "expected SUBSTRATE_LAUNCH_DEPENDENCY_FAILED; got {result}"
    );
    let fake = world.launch_fake.clone().expect("Given must set launch_fake");
    assert!(
        !fake.spawns().contains(&"api".to_owned()),
        "api must not be started when its required dependency db failed readiness; spawns={:?}",
        fake.spawns()
    );
}

#[then(regex = r#"^the error payload names db as the failed dependency$"#)]
async fn then_error_names_db(world: &mut SubstrateWorld) {
    let result = world
        .context
        .get("launch_up_result")
        .cloned()
        .expect("When must set launch_up_result");
    assert!(
        result.contains("dependency: \"db\"") || result.contains("\"db\""),
        "expected the DependencyFailed payload to name db; got {result}"
    );
}

// ---- launch-optional-dependency-fails-without-blocking ----------------------

#[given(
    regex = r#"^service web depends_on cache with required=false and cache fails readiness$"#
)]
async fn given_optional_dependency_fails(world: &mut SubstrateWorld) {
    let dir = TempDir::new().expect("tempdir");
    let body = "version = 1\n\n[services.cache]\ncommand = [\"cache\"]\nrequired = false\n\n[services.web]\ncommand = [\"web\"]\ndepends_on = [\"cache\"]\n";
    let profile = write_profile(dir.path(), body).await;
    let fake = FakeSubprocessPort::new();
    fake.script("cache", SubprocessState::Failed);
    let reg = registry(fake.clone(), dir.path());
    reg.trust(&profile).await.expect("trust");
    world.launch_registry = Some(reg);
    world.launch_fake = Some(fake);
    world.context.insert("launch_profile_path".to_owned(), profile);
    std::mem::forget(dir);
}

#[then(regex = r#"^web still starts and reaches readiness$"#)]
async fn then_web_still_starts(world: &mut SubstrateWorld) {
    let result = world
        .context
        .get("launch_up_result")
        .cloned()
        .expect("When must set launch_up_result");
    assert!(
        result.starts_with("Ok"),
        "expected launch.up to succeed (degraded, not failed); got {result}"
    );
    let fake = world.launch_fake.clone().expect("Given must set launch_fake");
    assert!(
        fake.spawns().contains(&"web".to_owned()),
        "web must still be started; spawns={:?}",
        fake.spawns()
    );
}

#[then(
    regex = r#"^the failed cache is reported as a warning, not SUBSTRATE_LAUNCH_DEPENDENCY_FAILED$"#
)]
async fn then_cache_reported_as_warning(world: &mut SubstrateWorld) {
    let result = world
        .context
        .get("launch_up_result")
        .cloned()
        .expect("When must set launch_up_result");
    assert!(
        !result.contains("DependencyFailed"),
        "an optional (required=false) dependency failure must not surface as \
         DependencyFailed; got {result}"
    );
}

// ---- launch-up-readiness-gated-start ----------------------------------------

#[given(
    regex = r#"^a trusted Profile with services db, api depends_on db, and web depends_on api$"#
)]
async fn given_trusted_three_tier_profile(world: &mut SubstrateWorld) {
    let dir = TempDir::new().expect("tempdir");
    let profile = write_profile(dir.path(), THREE_TIER).await;
    let fake = FakeSubprocessPort::new();
    let reg = registry(fake.clone(), dir.path());
    reg.trust(&profile).await.expect("trust");
    world.launch_registry = Some(reg);
    world.launch_fake = Some(fake);
    world.context.insert("launch_profile_path".to_owned(), profile);
    std::mem::forget(dir);
}

#[then(
    regex = r#"^db is started first and api waits until db reaches the Ready state$"#
)]
async fn then_db_started_first(world: &mut SubstrateWorld) {
    let fake = world.launch_fake.clone().expect("Given must set launch_fake");
    let spawns = fake.spawns();
    assert_eq!(
        spawns.first().map(String::as_str),
        Some("db"),
        "db must be the first Service started; spawns={spawns:?}"
    );
}

#[then(
    regex = r#"^api is started next and web waits until api reaches the Ready state$"#
)]
async fn then_api_started_next(world: &mut SubstrateWorld) {
    let fake = world.launch_fake.clone().expect("Given must set launch_fake");
    assert_eq!(
        fake.spawns(),
        vec!["db".to_owned(), "api".to_owned(), "web".to_owned()],
        "Services must start in strict topological order"
    );
}

#[then(
    regex = r#"^the launch\.up Task reports the Stack Running once every Service is Ready$"#
)]
async fn then_stack_reports_running(world: &mut SubstrateWorld) {
    let result = world
        .context
        .get("launch_up_result")
        .cloned()
        .expect("When must set launch_up_result");
    assert!(
        result.contains("Running"),
        "expected the Stack to report state Running; got {result}"
    );
}
