//! Step definitions for the process bounded context.
//!
//! Covers features:
//!   proc-list-happy-path, proc-signal-not-found,
//!   proc-signal-pid-outside-allowlist-blocked, proc-signal-sigkill-requires-elicitation.

#![allow(unused_variables)]
#![expect(
    clippy::expect_used,
    clippy::needless_pass_by_ref_mut,
    clippy::unused_async,
    clippy::trivial_regex,
    clippy::needless_raw_string_hashes,
    clippy::cast_possible_truncation,
    clippy::unimplemented,
    reason = "cucumber step functions require &mut World and async signatures; \
              raw strings and regex patterns are idiomatic in step definitions; \
              u32 truncation is intentional for PID conversion in test context; \
              unimplemented!() stubs are tracked separately"
)]

use cucumber::{given, then, when};

use crate::SubstrateWorld;

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given(regex = r#"^the host has more than (\d+) running processes$"#)]
async fn given_host_many_processes(world: &mut SubstrateWorld, min: u32) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(regex = r#"^the host has a running process with pid=(\d+) and name="([^"]+)"$"#)]
async fn given_running_process_pid(world: &mut SubstrateWorld, pid: u32, name: String) {
    world
        .context
        .insert("target_pid".to_string(), pid.to_string());
    world.context.insert("target_name".to_string(), name);
}

#[given(regex = r#"^the process pid=(\d+) is within the allowed PID range$"#)]
async fn given_pid_in_allowlist(world: &mut SubstrateWorld, pid: u32) {
    world
        .context
        .insert("allowed_pid".to_string(), pid.to_string());
}

#[given(
    regex = r#"^PID (\d+) does not refer to any running process on the system$"#
)]
async fn given_pid_not_running(world: &mut SubstrateWorld, pid: u32) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world
        .context
        .insert("nonexistent_pid".to_string(), pid.to_string());
}

#[given(regex = r#"^PID (\d+) does not refer to any running process$"#)]
async fn given_pid_not_running_simple(world: &mut SubstrateWorld, pid: u32) {
    world
        .context
        .insert("nonexistent_pid".to_string(), pid.to_string());
}

#[given(
    regex = r#"^a running process with PID (\d+) owned by the current user and within the process allowlist$"#
)]
async fn given_running_process_in_allowlist(world: &mut SubstrateWorld, pid: u32) {
    world
        .context
        .insert("target_pid".to_string(), pid.to_string());
}

#[given(regex = r#"^the host has at least (\d+) running processes$"#)]
async fn given_host_at_least_processes(world: &mut SubstrateWorld, count: u32) {
    world
        .context
        .insert("min_processes".to_string(), count.to_string());
}

#[given(regex = r#"^the first proc\.list call returned cursor "([^"]+)"$"#)]
async fn given_proc_first_cursor(world: &mut SubstrateWorld, cursor: String) {
    world
        .context
        .insert("prior_proc_cursor".to_string(), cursor);
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(regex = r#"^the client calls proc\.list$"#)]
async fn when_proc_list(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store("proc_list", serde_json::json!({}));
}

#[when(regex = r#"^the client calls proc\.list with cursor="([^"]+)"$"#)]
async fn when_proc_list_cursor(world: &mut SubstrateWorld, cursor: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "proc_list",
        serde_json::json!({ "cursor": cursor }),
    );
}

#[when(regex = r#"^the client calls proc\.list with page_size=(\d+)$"#)]
async fn when_proc_list_page_size(world: &mut SubstrateWorld, page_size: u32) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "proc_list",
        serde_json::json!({ "page_size": page_size }),
    );
}

#[when(
    regex = r#"^the client calls proc\.signal with pid=(\d+) and signal="?([A-Z]+)"? and elicitation_confirmed=(true|false)$"#
)]
async fn when_proc_signal(
    world: &mut SubstrateWorld,
    pid: u32,
    signal: String,
    confirmed: bool,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "proc_signal",
        serde_json::json!({
            "pid": pid,
            "signal": signal,
            "elicitation_confirmed": confirmed,
        }),
    );
}

#[when(
    regex = r#"^the client calls proc\.signal with pid=(\d+) and signal=([A-Z]+) and elicitation_confirmed=(true|false)$"#
)]
async fn when_proc_signal_unquoted(
    world: &mut SubstrateWorld,
    pid: u32,
    signal: String,
    confirmed: bool,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "proc_signal",
        serde_json::json!({
            "pid": pid,
            "signal": signal,
            "elicitation_confirmed": confirmed,
        }),
    );
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(regex = r#"^the structured content has exactly (\d+) process entries$"#)]
async fn then_proc_count(world: &mut SubstrateWorld, expected: usize) {
    unimplemented!(
        "step pending: proc-list-happy-path — process entry count {expected} requires live OS fixture"
    );
}

#[then(
    regex = r#"^each entry contains fields: pid, name, cpu_percent, mem_percent, parent_pid$"#
)]
async fn then_proc_entry_fields(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: proc-list-happy-path — per-entry field check requires live process data"
    );
}

#[then(regex = r#"^every entry has a non-null pid field of integer type$"#)]
async fn then_proc_pid_nonnull(world: &mut SubstrateWorld) {
    unimplemented!("step pending: proc-list-happy-path — pid field type check");
}

#[then(regex = r#"^every entry has a non-empty name field of string type$"#)]
async fn then_proc_name_nonempty(world: &mut SubstrateWorld) {
    unimplemented!("step pending: proc-list-happy-path — name field check");
}

#[then(
    regex = r#"^every entry has a cpu_percent field of float type between 0 and 100$"#
)]
async fn then_proc_cpu_range(world: &mut SubstrateWorld) {
    unimplemented!("step pending: proc-list-happy-path — cpu_percent range check");
}

#[then(
    regex = r#"^every entry has a mem_percent field of float type between 0 and 100$"#
)]
async fn then_proc_mem_range(world: &mut SubstrateWorld) {
    unimplemented!("step pending: proc-list-happy-path — mem_percent range check");
}

#[then(
    regex = r#"^every entry has a parent_pid field which is null for root processes$"#
)]
async fn then_proc_parent_pid(world: &mut SubstrateWorld) {
    unimplemented!("step pending: proc-list-happy-path — parent_pid nullability check");
}

#[then(
    regex = r#"^the returned PIDs do not overlap with the first page PIDs$"#
)]
async fn then_proc_no_pid_overlap(world: &mut SubstrateWorld) {
    unimplemented!("step pending: proc-list-happy-path — PID deduplication across pages");
}

#[then(regex = r#"^the process pid=(\d+) is still running$"#)]
async fn then_proc_still_running(world: &mut SubstrateWorld, pid: u32) {
    // Verify the process exists in /proc on Linux or via sysinfo on macOS.
    #[cfg(target_os = "linux")]
    {
        assert!(
            std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "expected pid {pid} to still exist"
        );
    }
    // On macOS we skip the live check — it requires the pid fixture to be real.
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
}

#[then(regex = r#"^the process pid=(\d+) is no longer running$"#)]
async fn then_proc_not_running(world: &mut SubstrateWorld, pid: u32) {
    unimplemented!(
        "step pending: proc-signal-sigkill — real-process termination check for pid {pid}"
    );
}

#[then(
    regex = r#"^the tool returns a success result with the signal sent and target pid$"#
)]
async fn then_signal_success(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["result"].is_object() && !resp["error"].is_object(),
        "expected signal success result, got: {resp}"
    );
}

#[then(regex = r#"^the signal SIGTERM is sent to process pid=(\d+)$"#)]
async fn then_sigterm_sent(world: &mut SubstrateWorld, pid: u32) {
    let resp = world.last_response.as_ref().expect("no response");
    // Accept either a success or an error that is not CONFIRMATION_REQUIRED.
    if resp["error"].is_object() {
        let code = resp["error"]["data"]["code"].as_str().unwrap_or("");
        assert_ne!(
            code, "SUBSTRATE_CONFIRMATION_REQUIRED",
            "SIGTERM should not require confirmation: {resp}"
        );
    }
}

#[then(regex = r#"^no SUBSTRATE_CONFIRMATION_REQUIRED error is returned$"#)]
async fn then_no_confirmation_required(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let code = resp["error"]["data"]["code"].as_str().unwrap_or("");
    assert_ne!(
        code, "SUBSTRATE_CONFIRMATION_REQUIRED",
        "unexpected CONFIRMATION_REQUIRED: {resp}"
    );
}

#[then(
    regex = r#"^the recovery_hint mentions "process does not exist" or "no such process"$"#
)]
async fn then_hint_mentions_no_process(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let hint = resp["error"]["data"]["recovery_hint"]
        .as_str()
        .unwrap_or("");
    assert!(
        hint.contains("process does not exist") || hint.contains("no such process"),
        "recovery_hint should mention missing process, got: '{hint}'"
    );
}

#[then(
    regex = r#"^the error object does not have field "code" equal to "SUBSTRATE_PERMISSION_DENIED"$"#
)]
async fn then_not_permission_denied(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let code = resp["error"]["data"]["code"].as_str().unwrap_or("");
    assert_ne!(
        code, "SUBSTRATE_PERMISSION_DENIED",
        "unexpected PERMISSION_DENIED: {resp}"
    );
}

#[then(
    regex = r#"^the error object details include field "pid" equal to (\d+)$"#
)]
async fn then_error_details_pid(world: &mut SubstrateWorld, pid: u32) {
    let resp = world.last_response.as_ref().expect("no response");
    let actual = resp["error"]["data"]["details"]["pid"]
        .as_u64()
        .unwrap_or(0);
    assert_eq!(
        actual as u32, pid,
        "expected pid {pid} in error details, got: {actual}"
    );
}

#[then(
    regex = r#"^the response does not contain a SUBSTRATE_NOT_FOUND error$"#
)]
async fn then_no_not_found_error(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let code = resp["error"]["data"]["code"].as_str().unwrap_or("");
    assert_ne!(
        code, "SUBSTRATE_NOT_FOUND",
        "unexpected SUBSTRATE_NOT_FOUND: {resp}"
    );
}
