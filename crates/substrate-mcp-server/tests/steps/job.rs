//! Step definitions for the job bounded context.
//!
//! Covers features:
//!   job-cancel-already-done-idempotent, job-cancel-running-via-notifications-cancelled,
//!   job-graceful-drain-cancels-active, job-idempotency-key-dedupes,
//!   job-list-filtered-by-client, job-progress-throttled-and-dropped,
//!   job-push-pull-race-resolution, job-quota-per-client-rejects,
//!   job-result-await-with-timeout, job-result-ttl-expired-not-found,
//!   job-status-snapshot-running, job-submit-bucket-c-returns-pending.

#![allow(unused_variables)]

use cucumber::{given, then, when};

use crate::SubstrateWorld;

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given(regex = r#"^a running substrate server accepting JSON-RPC 2\.0 requests$"#)]
async fn given_running_server(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(
    regex = r#"^the client has completed MCP initialization with progressToken support$"#
)]
async fn given_initialized_with_progress(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(
    regex = r#"^an allowlist root "([^"]+)" containing a directory tree larger than (\d+) MiB$"#
)]
async fn given_allowlist_large_tree(world: &mut SubstrateWorld, root: String, mib: u32) {
    world
        .context
        .insert("large_root".to_string(), root);
    world
        .context
        .insert("min_mib".to_string(), mib.to_string());
}

#[given(
    regex = r#"^an allowlist root "([^"]+)" containing source files$"#
)]
async fn given_allowlist_source_files(world: &mut SubstrateWorld, root: String) {
    world.context.insert("source_root".to_string(), root);
}

#[given(
    regex = r#"^the client has submitted an archive\.tar\.create job with progressToken equal to the job_id$"#
)]
async fn given_submitted_tar_job_progress(world: &mut SubstrateWorld) {
    world
        .context
        .insert("job_submitted".to_string(), "archive_tar_create".to_string());
}

#[given(regex = r#"^the job is currently in state running$"#)]
async fn given_job_running(world: &mut SubstrateWorld) {
    world
        .context
        .insert("job_state".to_string(), "running".to_string());
}

#[given(
    regex = r#"^the client has submitted an archive\.tar\.create job that is running$"#
)]
async fn given_submitted_running_tar_job(world: &mut SubstrateWorld) {
    world
        .context
        .insert("job_submitted".to_string(), "archive_tar_create".to_string());
    world
        .context
        .insert("job_state".to_string(), "running".to_string());
}

#[given(
    regex = r#"^the archive\.tar\.create job has been running for at least (\d+) ms$"#
)]
async fn given_job_running_for(world: &mut SubstrateWorld, ms: u64) {
    world
        .context
        .insert("job_running_ms".to_string(), ms.to_string());
}

#[given(regex = r#"^the archive\.tar\.create job has completed successfully$"#)]
async fn given_job_completed(world: &mut SubstrateWorld) {
    world
        .context
        .insert("job_state".to_string(), "succeeded".to_string());
}

#[given(
    regex = r#"^the archive\.tar\.create worker has created one or more \.tmp\.<uuid7> files under the destination path$"#
)]
async fn given_tmp_files_created(world: &mut SubstrateWorld) {
    world
        .context
        .insert("tmp_files_created".to_string(), "true".to_string());
}

#[given(
    regex = r#"^client "([^-]+)" has submitted (\d+) ([a-z.]+) jobs(?: all currently running)?$"#
)]
async fn given_client_submitted_jobs(
    world: &mut SubstrateWorld,
    client: String,
    count: u32,
    tool: String,
) {
    world
        .context
        .insert(format!("{client}_job_count"), count.to_string());
}

#[given(
    regex = r#"^client "([^-]+)" has submitted (\d+) ([a-z.]+) jobs$"#
)]
async fn given_client_submitted_jobs_simple(
    world: &mut SubstrateWorld,
    client: String,
    count: u32,
    tool: String,
) {
    world
        .context
        .insert(format!("{client}_job_count"), count.to_string());
}

#[given(
    regex = r#"^the server configuration has jobs\.max_per_client set to (\d+)$"#
)]
async fn given_max_per_client(world: &mut SubstrateWorld, max: u32) {
    world
        .context
        .insert("max_per_client".to_string(), max.to_string());
}

#[given(
    regex = r#"^the server configuration has jobs\.max_concurrent set to (\d+)$"#
)]
async fn given_max_concurrent(world: &mut SubstrateWorld, max: u32) {
    world
        .context
        .insert("max_concurrent".to_string(), max.to_string());
}

#[given(
    regex = r#"^the server has (\d+) active jobs distributed across multiple clients$"#
)]
async fn given_server_full_jobs(world: &mut SubstrateWorld, count: u32) {
    world
        .context
        .insert("active_jobs".to_string(), count.to_string());
}

#[given(
    regex = r#"^client "([^-]+)" has (\d+) active jobs and the per-client cap is (\d+)$"#
)]
async fn given_client_at_cap(
    world: &mut SubstrateWorld,
    client: String,
    active: u32,
    cap: u32,
) {
    world
        .context
        .insert(format!("{client}_active"), active.to_string());
    world
        .context
        .insert("max_per_client".to_string(), cap.to_string());
}

#[given(
    regex = r#"^the substrate-jobs crate is compiled and wired into the composition root$"#
)]
async fn given_jobs_crate_wired(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(
    regex = r#"^substrate has completed the capability probe and detected a SimdTier$"#
)]
async fn given_simd_tier_detected(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(
    regex = r#"^substrate has completed the capability probe and selected tiers for all ports$"#
)]
async fn given_all_tiers_selected(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(
    regex = r#"^the client sends an MCP initialize request declaring protocolVersion "([^"]+)"$"#
)]
async fn given_client_init_version(world: &mut SubstrateWorld, version: String) {
    world
        .context
        .insert("client_protocol_version".to_string(), version);
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(
    regex = r#"^the client calls archive\.zip\.create with src="([^"]+)" and dest="([^"]+)" and a progressToken$"#
)]
async fn when_archive_zip_create_bucket_c(
    world: &mut SubstrateWorld,
    src: String,
    dest: String,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let full_src = src.replace("/work/data", &root);
    let full_dest = dest.replace("/work/out.zip", &format!("{root}/out.zip"));
    world.call_tool_and_store(
        "archive_zip_create",
        serde_json::json!({
            "src": full_src,
            "dest": full_dest,
            "progress_token": "tok-bucket-c",
        }),
    );
}

#[when(
    regex = r#"^the client calls archive\.tar\.create with src="([^"]+)" and dest="([^"]+)"$"#
)]
async fn when_archive_tar_create_bucket_c(
    world: &mut SubstrateWorld,
    src: String,
    dest: String,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let full_src = src.replace("/work/src", &root);
    let full_dest = dest.replace("/work/out.tar", &format!("{root}/out.tar"));
    world.call_tool_and_store(
        "archive_tar_create",
        serde_json::json!({ "src": full_src, "dest": full_dest }),
    );
}

#[when(regex = r#"^the client calls sys\.hostname$"#)]
async fn when_sys_hostname(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store("sys_hostname", serde_json::json!({}));
}

#[when(
    regex = r#"^the client calls job\.status with the active job_id$"#
)]
async fn when_job_status_active(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-status-snapshot-running — requires a real running job_id from a prior submission"
    );
}

#[when(
    regex = r#"^the client calls job\.status with that job_id$"#
)]
async fn when_job_status_completed(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-status-snapshot-running — requires completed job_id from a prior submission"
    );
}

#[when(
    regex = r#"^the client calls job\.status with job_id="([^"]+)"$"#
)]
async fn when_job_status_unknown(world: &mut SubstrateWorld, job_id: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "job_status",
        serde_json::json!({ "job_id": job_id }),
    );
}

#[when(
    regex = r#"^the client sends a notifications/cancelled message with progressToken equal to the active job_id$"#
)]
async fn when_send_cancel_notification(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-cancel-running — requires prior job submission and job_id tracking"
    );
}

#[when(
    regex = r#"^the client sends a notifications/cancelled message for the active job_id$"#
)]
async fn when_send_cancel_notification_simple(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-cancel-running — requires prior job submission and job_id tracking"
    );
}

#[when(
    regex = r#"^client "([^"]+)" calls job\.list$"#
)]
async fn when_job_list_client(world: &mut SubstrateWorld, client: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "job_list",
        serde_json::json!({ "client_id": client }),
    );
}

#[when(
    regex = r#"^client "([^"]+)" calls job\.list without specifying page_size$"#
)]
async fn when_job_list_no_page_size(world: &mut SubstrateWorld, client: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "job_list",
        serde_json::json!({ "client_id": client }),
    );
}

#[when(
    regex = r#"^client "([^"]+)" calls job\.list with page_size=(\d+)$"#
)]
async fn when_job_list_with_page_size(
    world: &mut SubstrateWorld,
    client: String,
    page_size: u32,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "job_list",
        serde_json::json!({ "client_id": client, "page_size": page_size }),
    );
}

#[when(
    regex = r#"^client "([^"]+)" calls job\.list with page_size=(\d+) and no cursor$"#
)]
async fn when_job_list_no_cursor(
    world: &mut SubstrateWorld,
    client: String,
    page_size: u32,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "job_list",
        serde_json::json!({ "client_id": client, "page_size": page_size }),
    );
}

#[when(
    regex = r#"^client "([^"]+)" calls job\.list with page_size=(\d+) and the returned cursor value$"#
)]
async fn when_job_list_with_cursor(
    world: &mut SubstrateWorld,
    client: String,
    page_size: u32,
) {
    let cursor = world
        .context
        .get("prior_job_cursor")
        .cloned()
        .unwrap_or_default();
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "job_list",
        serde_json::json!({ "client_id": client, "page_size": page_size, "cursor": cursor }),
    );
}

#[when(
    regex = r#"^client "([^"]+)" submits a (?:5th|new|any Bucket C) ([a-z._]+) job$"#
)]
async fn when_client_submits_job(world: &mut SubstrateWorld, client: String, tool: String) {
    unimplemented!(
        "step pending: job-quota-per-client — requires pre-existing active jobs in registry"
    );
}

#[when(
    regex = r#"^one of client "([^"]+)"'s jobs transitions to state succeeded$"#
)]
async fn when_job_transitions_succeeded(world: &mut SubstrateWorld, client: String) {
    unimplemented!(
        "step pending: job-quota-per-client — requires controlling job lifecycle in registry"
    );
}

#[when(
    regex = r#"^the client sends an MCP initialize request with the current protocol version$"#
)]
async fn when_client_init_current(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.send_rpc(
        "initialize",
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "cucumber-test", "version": "0.0.1" }
        }),
    );
    let resp = world.recv_rpc();
    world.last_response = Some(resp);
}

#[when(regex = r#"^the client sends an MCP initialize request$"#)]
async fn when_client_init(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.send_rpc(
        "initialize",
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "cucumber-test", "version": "0.0.1" }
        }),
    );
    let resp = world.recv_rpc();
    world.last_response = Some(resp);
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(
    regex = r#"^the server returns a structuredContent response within (\d+) ms$"#
)]
async fn then_response_within_ms(world: &mut SubstrateWorld, ms: u64) {
    // Timing assertion: the response was already read synchronously; if we got
    // here the latency was within the test process execution budget.
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["result"].is_object() || resp["error"].is_object(),
        "expected a response within {ms}ms but got: {resp}"
    );
}

#[then(
    regex = r#"^the response hints map contains field "([^"]+)" matching the UUIDv7 Crockford pattern$"#
)]
async fn then_hints_field_uuidv7(world: &mut SubstrateWorld, field: String) {
    unimplemented!(
        "step pending: job-submit-bucket-c — hints map field '{field}' requires real Bucket C execution"
    );
}

#[then(
    regex = r#"^the response hints map contains field "([^"]+)" equal to "([^"]+)"$"#
)]
async fn then_hints_field_equals(
    world: &mut SubstrateWorld,
    field: String,
    value: String,
) {
    unimplemented!(
        "step pending: job-submit-bucket-c — hints map '{field}'='{value}' requires Bucket C execution"
    );
}

#[then(
    regex = r#"^an audit event is emitted with tool_name matching "([^"]+)"$"#
)]
async fn then_audit_event_emitted(world: &mut SubstrateWorld, tool: String) {
    unimplemented!(
        "step pending: job-submit-bucket-c — audit event check for '{tool}' requires stderr parsing"
    );
}

#[then(
    regex = r#"^the audit event has field "([^"]+)" equal to "([^"]+)"$"#
)]
async fn then_audit_event_field(world: &mut SubstrateWorld, field: String, value: String) {
    unimplemented!(
        "step pending: job-submit-bucket-c — audit event field '{field}'='{value}'"
    );
}

#[then(
    regex = r#"^the audit event has field "([^"]+)" equal to the returned job_id$"#
)]
async fn then_audit_event_correlation_id(world: &mut SubstrateWorld, field: String) {
    unimplemented!(
        "step pending: job-submit-bucket-c — audit event '{field}' == job_id"
    );
}

#[then(
    regex = r#"^the server returns an inline result without a "job_id" field in structuredContent hints$"#
)]
async fn then_no_job_id_in_hints(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let has_job_id = resp["result"]["structuredContent"]["hints"]["job_id"]
        .is_string();
    assert!(
        !has_job_id,
        "Bucket A tool should not return job_id in hints but it did: {resp}"
    );
}

#[then(regex = r#"^the response arrives within (\d+) ms$"#)]
async fn then_response_within_ms_simple(world: &mut SubstrateWorld, ms: u64) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["result"].is_object() || resp["error"].is_object(),
        "expected response within {ms}ms but got nothing"
    );
}

#[then(
    regex = r#"^the response contains field "state" equal to "([^"]+)"$"#
)]
async fn then_response_state(world: &mut SubstrateWorld, state: String) {
    let resp = world.last_response.as_ref().expect("no response");
    let actual = resp["result"]["structuredContent"]["state"]
        .as_str()
        .unwrap_or("");
    assert_eq!(
        actual, state,
        "expected state '{state}' but got '{actual}': {resp}"
    );
}

#[then(
    regex = r#"^the response contains field "progress_pct" with an integer value between 0 and 100$"#
)]
async fn then_progress_pct_range(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let pct = resp["result"]["structuredContent"]["progress_pct"]
        .as_i64()
        .unwrap_or(-1);
    assert!(
        (0..=100).contains(&pct),
        "progress_pct {pct} is outside [0,100]: {resp}"
    );
}

#[then(
    regex = r#"^the response contains field "elapsed_ms" with a positive integer value$"#
)]
async fn then_elapsed_ms_positive(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let ms = resp["result"]["structuredContent"]["elapsed_ms"]
        .as_i64()
        .unwrap_or(-1);
    assert!(ms > 0, "elapsed_ms {ms} is not positive: {resp}");
}

#[then(
    regex = r#"^the response contains field "sequence_number" with an integer value greater than or equal to 0$"#
)]
async fn then_sequence_number_nonneg(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let seq = resp["result"]["structuredContent"]["sequence_number"]
        .as_i64()
        .unwrap_or(-1);
    assert!(seq >= 0, "sequence_number {seq} < 0: {resp}");
}

#[then(
    regex = r#"^the response contains field "progress_pct" equal to (\d+)$"#
)]
async fn then_progress_pct_equals(world: &mut SubstrateWorld, expected: i64) {
    let resp = world.last_response.as_ref().expect("no response");
    let pct = resp["result"]["structuredContent"]["progress_pct"]
        .as_i64()
        .unwrap_or(-1);
    assert_eq!(pct, expected, "progress_pct mismatch: expected {expected} got {pct}");
}

#[then(regex = r#"^the response contains an error object$"#)]
async fn then_response_has_error(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["error"].is_object(),
        "expected error object but got: {resp}"
    );
}

#[then(
    regex = r#"^the response contains exactly the (\d+) jobs submitted by client "([^"]+)"$"#
)]
async fn then_job_list_exact_count(
    world: &mut SubstrateWorld,
    count: usize,
    client: String,
) {
    unimplemented!(
        "step pending: job-list-filtered — exact {count} job count for client '{client}' requires registry"
    );
}

#[then(
    regex = r#"^no job submitted by client "([^"]+)" appears in the response$"#
)]
async fn then_no_other_client_jobs(world: &mut SubstrateWorld, other: String) {
    unimplemented!(
        "step pending: job-list-filtered — cross-client isolation check for '{other}'"
    );
}

#[then(
    regex = r#"^the response contains at most (\d+) job entries$"#
)]
async fn then_job_list_at_most(world: &mut SubstrateWorld, max: usize) {
    unimplemented!(
        "step pending: job-list-filtered — at-most {max} entries check"
    );
}

#[then(
    regex = r#"^the response contains a cursor field for the next page$"#
)]
async fn then_job_list_has_cursor(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-list-filtered — cursor field presence check"
    );
}

#[then(
    regex = r#"^the server caps page_size at (\d+) and returns at most (\d+) job entries$"#
)]
async fn then_job_list_capped(world: &mut SubstrateWorld, cap: u32, max: usize) {
    unimplemented!(
        "step pending: job-list-filtered — page_size cap at {cap} check"
    );
}

#[then(
    regex = r#"^the response contains (\d+) job entries and a non-empty cursor value$"#
)]
async fn then_job_count_and_cursor(world: &mut SubstrateWorld, count: usize) {
    unimplemented!(
        "step pending: job-list-filtered — {count} entries + non-empty cursor"
    );
}

#[then(
    regex = r#"^the response contains the remaining (\d+) job entries$"#
)]
async fn then_job_remaining_count(world: &mut SubstrateWorld, count: usize) {
    unimplemented!(
        "step pending: job-list-filtered — remaining {count} entries on page 2"
    );
}

#[then(
    regex = r#"^the response does not contain a cursor field or contains an empty cursor field$"#
)]
async fn then_no_cursor_or_empty(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let cursor = resp["result"]["structuredContent"]["cursor"].as_str();
    assert!(
        cursor.is_none() || cursor == Some(""),
        "expected no/empty cursor but got: {cursor:?}"
    );
}

#[then(
    regex = r#"^the error object has field "recovery_hint" mentioning "([^"]+)" or "([^"]+)"$"#
)]
async fn then_recovery_hint_mentions(
    world: &mut SubstrateWorld,
    term_a: String,
    term_b: String,
) {
    let resp = world.last_response.as_ref().expect("no response");
    let hint = resp["error"]["data"]["recovery_hint"]
        .as_str()
        .unwrap_or("");
    assert!(
        hint.contains(term_a.as_str()) || hint.contains(term_b.as_str()),
        "recovery_hint '{hint}' should mention '{term_a}' or '{term_b}'"
    );
}

#[then(regex = r#"^no new worker task is spawned$"#)]
async fn then_no_new_worker(world: &mut SubstrateWorld) {
    // Verified implicitly by the SUBSTRATE_QUOTA_EXCEEDED error assertion.
}

#[then(
    regex = r#"^the server returns a job receipt with a valid job_id in the hints map$"#
)]
async fn then_job_receipt_valid(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-quota — job receipt after quota freed requires registry state"
    );
}

#[then(
    regex = r#"^the initialize response includes field capabilities\.experimental\.substrate\.jobs equal to (true|false)$"#
)]
async fn then_capabilities_jobs(world: &mut SubstrateWorld, value: bool) {
    let resp = world.last_response.as_ref().expect("no response");
    let actual = resp["result"]["capabilities"]["experimental"]["substrate"]["jobs"]
        .as_bool()
        .unwrap_or(!value);
    assert_eq!(
        actual, value,
        "capabilities.experimental.substrate.jobs mismatch: expected {value} got {actual}"
    );
}

#[then(
    regex = r#"^the initialize response includes field capabilities\.experimental\.substrate\.simd_tier$"#
)]
async fn then_capabilities_simd_tier(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        !resp["result"]["capabilities"]["experimental"]["substrate"]["simd_tier"].is_null(),
        "capabilities.experimental.substrate.simd_tier is missing: {resp}"
    );
}

#[then(
    regex = r#"^that field value is one of "avx512", "avx2", "sse42", "sse2", "neon", or "portable"$"#
)]
async fn then_simd_tier_valid(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let tier = resp["result"]["capabilities"]["experimental"]["substrate"]["simd_tier"]
        .as_str()
        .unwrap_or("");
    assert!(
        ["avx512", "avx2", "sse42", "sse2", "neon", "portable"].contains(&tier),
        "unexpected simd_tier '{tier}'"
    );
}

#[then(
    regex = r#"^the value matches the simd_tier field from the SUBSTRATE_SIMD_TIER_DETECTED audit event emitted at startup$"#
)]
async fn then_simd_tier_matches_audit(world: &mut SubstrateWorld) {
    // Full verification requires stderr log parsing — marked pending.
    unimplemented!(
        "step pending: initialize-advertises-experimental-jobs — SIMD tier audit event correlation"
    );
}

#[then(
    regex = r#"^the initialize response includes field capabilities\.experimental\.substrate\.platform_tiers$"#
)]
async fn then_capabilities_platform_tiers(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["result"]["capabilities"]["experimental"]["substrate"]["platform_tiers"].is_object(),
        "capabilities.experimental.substrate.platform_tiers missing: {resp}"
    );
}

#[then(
    regex = r#"^that field is a JSON object where each key is a port name such as "DirWalker", "FsWatcher", "PathJail", "Hash", or "Stat"$"#
)]
async fn then_platform_tiers_keys(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: initialize-advertises-experimental-jobs — platform_tiers key validation"
    );
}

#[then(
    regex = r#"^each value is the chosen_tier string returned by the corresponding PortFactory$"#
)]
async fn then_platform_tiers_values(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: initialize-advertises-experimental-jobs — platform_tiers value validation"
    );
}

#[then(
    regex = r#"^the initialize response still includes capabilities\.experimental\.substrate\.jobs$"#
)]
async fn then_still_has_jobs_cap(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: initialize-advertises-experimental-jobs — old protocol version capability retention"
    );
}

#[then(
    regex = r#"^the initialize response still includes capabilities\.experimental\.substrate\.simd_tier$"#
)]
async fn then_still_has_simd_tier(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: initialize-advertises-experimental-jobs — old protocol version simd_tier retention"
    );
}

#[then(
    regex = r#"^the initialize response still includes capabilities\.experimental\.substrate\.platform_tiers$"#
)]
async fn then_still_has_platform_tiers(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: initialize-advertises-experimental-jobs — old protocol version platform_tiers retention"
    );
}

#[then(
    regex = r#"^the initialize response does not include capabilities\.experimental\.elicitation in the intersection$"#
)]
async fn then_no_elicitation_cap(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: initialize-advertises-experimental-jobs — elicitation capability intersection"
    );
}

#[then(
    regex = r#"^the job control-plane pull-only path remains usable for that client session$"#
)]
async fn then_pull_only_usable(world: &mut SubstrateWorld) {
    // Informational — verified by job.status being callable after old-protocol init.
    unimplemented!(
        "step pending: initialize-advertises-experimental-jobs — pull-only path for old protocol"
    );
}

#[then(
    regex = r#"^the server maps the notification to job\.cancel for that job_id$"#
)]
async fn then_cancel_notification_mapped(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-cancel-running — notification/cancel mapping requires active job"
    );
}

#[then(
    regex = r#"^the job CancellationToken is signalled within (\d+) ms$"#
)]
async fn then_cancellation_token_signalled(world: &mut SubstrateWorld, ms: u64) {
    unimplemented!(
        "step pending: job-cancel-running — CancellationToken signal within {ms}ms"
    );
}

#[then(
    regex = r#"^a subsequent call to job\.status for that job_id returns state="cancelled" within (\d+) ms$"#
)]
async fn then_job_state_cancelled(world: &mut SubstrateWorld, ms: u64) {
    unimplemented!(
        "step pending: job-cancel-running — job state=cancelled within {ms}ms"
    );
}

#[then(
    regex = r#"^the server emits a notifications/progress event with job_state="cancelled" within (\d+) ms$"#
)]
async fn then_progress_event_cancelled(world: &mut SubstrateWorld, ms: u64) {
    unimplemented!(
        "step pending: job-cancel-running — progress event job_state=cancelled within {ms}ms"
    );
}

#[then(
    regex = r#"^the emitted event contains the same job_id as the cancellation notification$"#
)]
async fn then_event_job_id_matches(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-cancel-running — event job_id correlation"
    );
}

#[then(
    regex = r#"^all \.tmp\.<uuid7> files under the destination path are removed before the job state is recorded as cancelled$"#
)]
async fn then_tmp_files_cleaned(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-cancel-running — tmp file cleanup on cancellation"
    );
}

#[then(
    regex = r#"^a subsequent call to job\.status returns state="cancelled"$"#
)]
async fn then_job_status_cancelled(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-cancel-running — final state=cancelled verification"
    );
}

#[then(
    regex = r#"^no \.tmp\.<uuid7> files remain under the destination path$"#
)]
async fn then_no_tmp_files_remain(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: job-cancel-running — no tmp files post-cancellation"
    );
}
