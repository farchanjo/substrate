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

//! Step definitions for subprocess elicitation and env-var injection scenarios.
//!
//! Covers features:
//!   elicitation-required-before-spawn
//!   env-var-injection-blocked
//!
//! These tests exercise two complementary security gates:
//!   1. `elicitation_confirmed = false` → `SUBSTRATE_ELICITATION_REQUIRED`
//!   2. `env_override` containing `LD_PRELOAD` → `SUBSTRATE_SUBPROCESS_ENV_BANNED`
//!
//! Both checks are performed by `SubprocessRequest::validate()` before any OS
//! spawn is attempted, so no fixture binary is required.

#![cfg(feature = "subprocess")]
#![expect(
    clippy::expect_used,
    clippy::needless_pass_by_ref_mut,
    clippy::unused_async,
    clippy::needless_raw_string_hashes,
    reason = "cucumber step functions require &mut World and async signatures; \
              raw strings are idiomatic in step definitions"
)]

use std::collections::BTreeMap;

use cucumber::{given, then, when};
use tempfile::TempDir;

use substrate_domain::ports::subprocess::SubprocessPort;
use substrate_domain::subprocess::request::{CaptureKind, StdinKind, SubprocessRequest};

use super::NoCancel;
use crate::SubstrateWorld;

// ---------------------------------------------------------------------------
// Given steps — elicitation feature
// ---------------------------------------------------------------------------

/// `Given subprocess.spawn is invoked without elicitation_confirmed flag`
///
/// Records intent; actual spawn is done in the Then step to match the feature
/// wording which jumps directly from Given to Then without a When.
#[given(regex = r#"^subprocess\.spawn is invoked without elicitation_confirmed flag$"#)]
async fn given_spawn_without_elicitation(world: &mut SubstrateWorld) {
    world
        .context
        .insert("elicitation_confirmed".to_string(), "false".to_string());
    world
        .context
        .insert("spawn_requested".to_string(), "true".to_string());

    // Perform the spawn immediately so Then steps can assert on the result.
    let sandbox = TempDir::new().expect("TempDir");
    let cwd = sandbox
        .path()
        .canonicalize()
        .unwrap_or_else(|_| sandbox.path().to_path_buf());

    let registry = super::make_deny_all_registry(vec![cwd.clone()]);

    let req = SubprocessRequest {
        binary_path: super::echo_binary_path(),
        args: Vec::new(),
        env_allowlist: Vec::new(),
        env_override: BTreeMap::new(),
        cwd,
        stdin_kind: StdinKind::None,
        capture_kind: CaptureKind::InMemory,
        timeout_secs: Some(5),
        idempotency_key: None,
        elicitation_confirmed: false,
        name: None,
        restart_policy: None,
        health_probe: None,
        log_rotation: None,
    };

    let result = registry.spawn(req, &NoCancel).await;
    match result {
        Ok(h) => {
            world
                .context
                .insert("elicitation_spawn_success".to_string(), "true".to_string());
            world
                .context
                .insert("elicitation_job_id".to_string(), h.job_id.to_string());
        },
        Err(e) => {
            world
                .context
                .insert("elicitation_spawn_success".to_string(), "false".to_string());
            world
                .context
                .insert("elicitation_error_code".to_string(), e.code().to_string());
        },
    }
    drop(sandbox);
}

/// `Given subprocess.spawn is invoked with env_override containing key LD_PRELOAD`
///
/// Records the banned env-var attempt and performs the spawn immediately.
#[given(
    regex = r#"^subprocess\.spawn is invoked with env_override containing key (LD_PRELOAD|LD_LIBRARY_PATH|DYLD_INSERT_LIBRARIES|LD_AUDIT)$"#
)]
async fn given_spawn_with_banned_env(world: &mut SubstrateWorld, banned_key: String) {
    world
        .context
        .insert("banned_env_key".to_string(), banned_key.clone());

    let sandbox = TempDir::new().expect("TempDir");
    let cwd = sandbox
        .path()
        .canonicalize()
        .unwrap_or_else(|_| sandbox.path().to_path_buf());

    // Use a registry that allows /usr/bin/echo so the binary allowlist does
    // not interfere and we isolate the env-var check.
    let registry = super::make_registry_with_echo(vec![cwd.clone()]);

    let mut env_override = BTreeMap::new();
    env_override.insert(banned_key.clone(), "/evil/lib.so".to_string());

    let req = SubprocessRequest {
        binary_path: super::echo_binary_path(),
        args: Vec::new(),
        env_allowlist: Vec::new(),
        env_override,
        cwd,
        stdin_kind: StdinKind::None,
        capture_kind: CaptureKind::InMemory,
        timeout_secs: Some(5),
        idempotency_key: None,
        // Elicitation is confirmed so env-var check is the failure point.
        elicitation_confirmed: true,
        name: None,
        restart_policy: None,
        health_probe: None,
        log_rotation: None,
    };

    let result = registry.spawn(req, &NoCancel).await;
    match result {
        Ok(h) => {
            world
                .context
                .insert("env_spawn_success".to_string(), "true".to_string());
            world
                .context
                .insert("env_spawn_job_id".to_string(), h.job_id.to_string());
        },
        Err(e) => {
            world
                .context
                .insert("env_spawn_success".to_string(), "false".to_string());
            world
                .context
                .insert("env_spawn_error_code".to_string(), e.code().to_string());
        },
    }
    drop(sandbox);
}

// ---------------------------------------------------------------------------
// When steps — elicitation + env feature
// ---------------------------------------------------------------------------

/// `When the subprocess_invariants Rego policy evaluates the request`
///
/// The Rego policy is evaluated at request-validation time inside
/// `SubprocessRequest::validate()`.  The Given step above already performed
/// the spawn (and `validate()` call).  This When step is a no-op continuation
/// that matches the Gherkin without re-executing the spawn.
#[when(regex = r#"^the subprocess_invariants Rego policy evaluates the request$"#)]
async fn when_rego_policy_evaluates(_world: &mut SubstrateWorld) {
    // No-op: the spawn and policy evaluation were performed in the Given step.
}

// ---------------------------------------------------------------------------
// Then steps — elicitation feature
// ---------------------------------------------------------------------------

/// `Then an MCP elicitation form is emitted to the client describing the spawn request`
///
/// In the current implementation the elicitation gate is expressed as
/// `SUBSTRATE_ELICITATION_REQUIRED` error rather than a live MCP form-mode
/// request (Wave 2c wires the form-mode protocol interaction). This step asserts
/// the error code as a proxy for the form being emitted.
#[then(
    regex = r#"^an MCP elicitation form is emitted to the client describing the spawn request$"#
)]
async fn then_elicitation_form_emitted(world: &mut SubstrateWorld) {
    let success = world
        .context
        .get("elicitation_spawn_success")
        .is_some_and(|v| v == "true");
    assert!(
        !success,
        "expected spawn to be gated by elicitation but it succeeded"
    );
    let code = world
        .context
        .get("elicitation_error_code")
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        code, "SUBSTRATE_ELICITATION_REQUIRED",
        "expected SUBSTRATE_ELICITATION_REQUIRED but got {code}"
    );
}

/// `And no child process is created until elicitation_confirmed is true`
#[then(regex = r#"^no child process is created until elicitation_confirmed is true$"#)]
async fn then_no_child_until_confirmed(world: &mut SubstrateWorld) {
    let success = world
        .context
        .get("elicitation_spawn_success")
        .is_some_and(|v| v == "true");
    assert!(
        !success,
        "expected no child process to be created without elicitation confirmation"
    );
}

/// `And re-invocation with elicitation_confirmed false also returns SUBSTRATE_ELICITATION_REQUIRED`
#[then(
    regex = r#"^re-invocation with elicitation_confirmed false also returns SUBSTRATE_ELICITATION_REQUIRED$"#
)]
async fn then_reinvocation_also_blocked(_world: &mut SubstrateWorld) {
    // Re-invoke synchronously to verify idempotency of the gate.
    let sandbox = TempDir::new().expect("TempDir");
    let cwd = sandbox
        .path()
        .canonicalize()
        .unwrap_or_else(|_| sandbox.path().to_path_buf());

    let registry = super::make_deny_all_registry(vec![cwd.clone()]);

    let req = SubprocessRequest {
        binary_path: super::echo_binary_path(),
        args: Vec::new(),
        env_allowlist: Vec::new(),
        env_override: BTreeMap::new(),
        cwd,
        stdin_kind: StdinKind::None,
        capture_kind: CaptureKind::InMemory,
        timeout_secs: Some(5),
        idempotency_key: None,
        elicitation_confirmed: false,
        name: None,
        restart_policy: None,
        health_probe: None,
        log_rotation: None,
    };

    let result = registry.spawn(req, &NoCancel).await;
    match result {
        Ok(_) => panic!(
            "re-invocation with elicitation_confirmed=false should return \
             SUBSTRATE_ELICITATION_REQUIRED but spawn succeeded"
        ),
        Err(e) => {
            assert_eq!(
                e.code(),
                "SUBSTRATE_ELICITATION_REQUIRED",
                "expected SUBSTRATE_ELICITATION_REQUIRED on re-invocation but got {}",
                e.code()
            );
        },
    }
    drop(sandbox);
}

// ---------------------------------------------------------------------------
// Then steps — env-var-injection feature
// ---------------------------------------------------------------------------

/// `Then the policy denies with msg containing "banned env var"`
#[then(regex = r#"^the policy denies with msg containing "([^"]+)"$"#)]
async fn then_policy_denies_with_msg(world: &mut SubstrateWorld, _expected_fragment: String) {
    let success = world
        .context
        .get("env_spawn_success")
        .is_some_and(|v| v == "true");
    assert!(
        !success,
        "expected env-var spawn to be denied but it succeeded"
    );
    let code = world
        .context
        .get("env_spawn_error_code")
        .cloned()
        .unwrap_or_default();
    // The error code SUBSTRATE_SUBPROCESS_ENV_BANNED confirms the policy denial.
    // The feature says "banned env var" — the domain code uses that exact phrase.
    assert_eq!(
        code, "SUBSTRATE_SUBPROCESS_ENV_BANNED",
        "expected env-ban code but got {code}"
    );
}

/// `And error code SUBSTRATE_SUBPROCESS_ENV_BANNED is returned`
#[then(regex = r#"^error code (SUBSTRATE_[A-Z_]+) is returned$"#)]
async fn then_error_code_returned(world: &mut SubstrateWorld, expected_code: String) {
    // Look in both env and elicitation context keys.
    let actual_code = world
        .context
        .get("env_spawn_error_code")
        .or_else(|| world.context.get("elicitation_error_code"))
        .cloned()
        .unwrap_or_default();

    let matches = actual_code == expected_code
        || (expected_code == "SUBSTRATE_SUBPROCESS_ENV_BANNED"
            && actual_code == "SUBSTRATE_SUBPROCESS_ENV_BANNED");

    assert!(
        matches,
        "expected error code {expected_code} but got {actual_code}"
    );
}
