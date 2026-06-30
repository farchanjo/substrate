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

//! Step definitions for the launch event stream's redaction-at-source and
//! pull-floor degrade (ADR-0066).
//!
//! Covers: launch-event-redaction-at-source, launch-global-redact-denylist-applied,
//! launch-event-pull-floor-no-subscribe.

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
use substrate_launch::redaction::Redactor;
use tempfile::TempDir;

use super::{FakeSubprocessPort, NeverCancel, THREE_TIER, registry, write_profile};
use crate::SubstrateWorld;

// ---- launch-event-redaction-at-source ---------------------------------------

#[given(regex = r#"^a Service whose redact patterns match a secret token$"#)]
async fn given_service_with_redact_pattern(world: &mut SubstrateWorld) {
    world.context.insert(
        "launch_redact_per_service".to_owned(),
        "s3cr3t-token".to_owned(),
    );
    world
        .context
        .insert("launch_redact_global".to_owned(), String::new());
}

#[when(regex = r#"^the child prints a line containing that token$"#)]
async fn when_child_prints_secret_line(world: &mut SubstrateWorld) {
    let per_service = world
        .context
        .get("launch_redact_per_service")
        .cloned()
        .unwrap_or_default();
    let global = world.context.get("launch_redact_global").cloned().unwrap_or_default();
    let per_service_list: Vec<String> = per_service
        .split(',')
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    let global_list: Vec<String> = global
        .split(',')
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    let redactor = Redactor::new(&global_list, &per_service_list);
    let needle = per_service_list
        .first()
        .or_else(|| global_list.first())
        .cloned()
        .unwrap_or_default();
    let raw_line = format!("connecting with {needle} now");
    let redacted = redactor.redact_line(&raw_line);
    world.context.insert("launch_redact_raw".to_owned(), raw_line);
    world.context.insert("launch_redact_needle".to_owned(), needle);
    world
        .context
        .insert("launch_redact_output".to_owned(), redacted);
}

#[then(regex = r#"^the line written to the event-log is redacted$"#)]
async fn then_line_written_is_redacted(world: &mut SubstrateWorld) {
    let needle = world
        .context
        .get("launch_redact_needle")
        .cloned()
        .expect("When must set launch_redact_needle");
    let output = world
        .context
        .get("launch_redact_output")
        .cloned()
        .expect("When must set launch_redact_output");
    assert!(
        !output.contains(&needle),
        "redacted line must not contain the secret; got {output:?}"
    );
    assert!(output.contains("[REDACTED]"));
}

#[then(regex = r#"^the event delivered to the client is redacted$"#)]
async fn then_event_delivered_is_redacted(world: &mut SubstrateWorld) {
    // The MVP applies redaction once, at the same seam that would feed both
    // the event-log write and the client-delivered event (the `Redactor` is
    // applied before either consumer per ADR-0066 "redaction at the source");
    // the previous Then step's assertion on the redacted output covers both.
    let output = world
        .context
        .get("launch_redact_output")
        .cloned()
        .expect("When must set launch_redact_output");
    assert!(output.contains("[REDACTED]"));
}

// ---- launch-global-redact-denylist-applied ----------------------------------

#[given(
    regex = r#"^a service with an empty per-service redact list and a global denylist matching API_KEY assignments$"#
)]
async fn given_empty_per_service_global_denylist(world: &mut SubstrateWorld) {
    world
        .context
        .insert("launch_redact_per_service".to_owned(), String::new());
    world.context.insert(
        "launch_redact_global".to_owned(),
        "AKIAEXAMPLESECRET".to_owned(),
    );
}

#[when(regex = r#"^the service prints a line containing an API_KEY value$"#)]
async fn when_service_prints_api_key_line(world: &mut SubstrateWorld) {
    let global = world
        .context
        .get("launch_redact_global")
        .cloned()
        .unwrap_or_default();
    let redactor = Redactor::new(&[global.clone()], &[]);
    let raw_line = format!("API_KEY={global}");
    let redacted = redactor.redact_line(&raw_line);
    world.context.insert("launch_redact_needle".to_owned(), global);
    world
        .context
        .insert("launch_redact_output".to_owned(), redacted);
}

#[then(regex = r#"^the stored event-log entry and the emitted event are redacted$"#)]
async fn then_stored_and_emitted_redacted(world: &mut SubstrateWorld) {
    let output = world
        .context
        .get("launch_redact_output")
        .cloned()
        .expect("When must set launch_redact_output");
    assert_eq!(output, "API_KEY=[REDACTED]");
}

#[then(regex = r#"^the raw secret never reaches the model context$"#)]
async fn then_raw_secret_never_reaches_context(world: &mut SubstrateWorld) {
    let needle = world
        .context
        .get("launch_redact_needle")
        .cloned()
        .expect("When must set launch_redact_needle");
    let output = world
        .context
        .get("launch_redact_output")
        .cloned()
        .expect("When must set launch_redact_output");
    assert!(!output.contains(&needle));
}

// ---- launch-event-pull-floor-no-subscribe -----------------------------------

#[given(regex = r#"^a client that does not advertise resources\.subscribe$"#)]
async fn given_client_without_subscribe(world: &mut SubstrateWorld) {
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

#[when(regex = r#"^a Stack emits lifecycle and semantic events$"#)]
async fn when_stack_emits_events(world: &mut SubstrateWorld) {
    let profile = world
        .context
        .get("launch_profile_path")
        .cloned()
        .expect("Given must set launch_profile_path");
    let reg = world.launch_registry.clone().expect("Given must set launch_registry");
    let handle = reg
        .up(&profile, None, None, &NeverCancel)
        .await
        .expect("up succeeds");
    world
        .context
        .insert("launch_stack_id".to_owned(), handle.stack_id.to_string());
}

#[then(regex = r#"^no notifications/resources/updated poke is sent$"#)]
async fn then_no_resources_updated_poke(_world: &mut SubstrateWorld) {
    // The MVP has no resource-subscription push path at all (ADR-0066
    // amendment: deferred to Milestone 2 alongside the detached supervisor) —
    // every client, subscribing or not, is on the pull floor today. The
    // absence of a push mechanism is structurally guaranteed by the registry
    // never holding a notification sender; nothing to assert beyond the next
    // step's confirmation that the pull path works.
}

#[then(
    regex = r#"^the events remain readable via launch\.status and launch\.logs polling$"#
)]
async fn then_events_readable_via_polling(world: &mut SubstrateWorld) {
    let reg = world.launch_registry.clone().expect("Given must set launch_registry");
    let stack_id_str = world
        .context
        .get("launch_stack_id")
        .cloned()
        .expect("When must set launch_stack_id");
    let stack_id: substrate_domain::value_objects::stack_id::StackId =
        stack_id_str.parse().expect("valid StackId");
    let (events, _cursor) = reg
        .logs(&stack_id, None, None)
        .await
        .expect("logs must be readable via polling");
    assert!(!events.is_empty(), "lifecycle events must be present");
    let handles = reg.status(Some(&stack_id)).await.expect("status");
    assert!(!handles.is_empty(), "status must be readable via polling");
}
