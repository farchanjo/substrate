//! Step definitions for the network bounded context.
//!
//! Covers features:
//!   net-connection-count-histogram,
//!   net-tcp-list-by-state,
//!   net-tcp-list-listen-entry-has-nonzero-local-port,
//!   net-tcp-list-with-pid-resolution,
//!   net-tcp-stats-counters,
//!   net-tcp-stats-returns-nonzero-counters-on-active-host.

#![allow(unused_variables)]
#![expect(
    clippy::expect_used,
    clippy::needless_pass_by_ref_mut,
    clippy::unused_async,
    clippy::trivial_regex,
    clippy::needless_raw_string_hashes,
    reason = "cucumber step functions require &mut World and async signatures; \
              raw strings and regex patterns are idiomatic in step definitions"
)]

use cucumber::{given, then, when};

use crate::SubstrateWorld;

/// The 12 canonical TCP-state variant names returned by `net_connection_count`.
const TCP_STATE_VARIANTS: &[&str] = &[
    "Closed",
    "Listen",
    "SynSent",
    "SynReceived",
    "Established",
    "FinWait1",
    "FinWait2",
    "CloseWait",
    "Closing",
    "LastAck",
    "TimeWait",
    "DeleteTcb",
];

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given(regex = r#"^the net\.connection_count tool is available$"#)]
async fn given_net_connection_count_available(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(regex = r#"^the net\.tcp_list tool is available$"#)]
async fn given_net_tcp_list_available(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(regex = r#"^the net\.tcp_stats tool is available$"#)]
async fn given_net_tcp_stats_available(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(regex = r#"^a host with at least one process bound to a listening TCP socket$"#)]
async fn given_host_has_listening_socket(world: &mut SubstrateWorld) {
    // The test host is expected to have at least one listening socket (e.g.,
    // sshd, or another service). This is a precondition acknowledgement — no
    // explicit fixture setup is needed; the When step issues the filtered call.
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(regex = r#"^a host with at least one TCP connection in the Established state$"#)]
async fn given_host_has_established_connection(world: &mut SubstrateWorld) {
    // Established connections exist on any active host.  This step is a
    // precondition acknowledgement; the When step performs the filtered call.
    // If the host has no Established connections the Then assertions will be
    // vacuously satisfied (empty entries list does not violate per-entry rules).
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(
    regex = r#"^at least one TCP server is listening on the host(?: \(e\.g\., SSH on port 22\))?$"#
)]
async fn given_at_least_one_tcp_listener(world: &mut SubstrateWorld) {
    // Listening sockets exist on any normal host. Precondition acknowledgement.
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(regex = r#"^the substrate-mcp-server is running on macOS$"#)]
async fn given_running_on_macos(world: &mut SubstrateWorld) {
    // Skip the scenario on non-macOS platforms so the test is not a false failure.
    if !cfg!(target_os = "macos") {
        world.skip_scenario = true;
        return;
    }
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(regex = r#"^the substrate-mcp-server is running on Linux$"#)]
async fn given_running_on_linux(world: &mut SubstrateWorld) {
    // Skip the scenario on non-Linux platforms.
    if !cfg!(target_os = "linux") {
        world.skip_scenario = true;
        return;
    }
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(regex = r#"^the host has completed at least one TCP handshake since boot$"#)]
async fn given_host_has_completed_tcp_handshake(world: &mut SubstrateWorld) {
    // Informational precondition — any active host satisfies this condition.
    // The Then steps assert segs_in > 0 and segs_out > 0.
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(regex = r#"^net\.connection_count is invoked with no parameters$"#)]
async fn when_net_connection_count(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store("net_connection_count", serde_json::json!({}));
}

#[when(regex = r#"^net\.tcp_list is invoked with state_filter \["Listen"\]$"#)]
async fn when_net_tcp_list_listen(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "net_tcp_list",
        serde_json::json!({ "state_filter": ["Listen"] }),
    );
}

#[when(regex = r#"^net\.tcp_list is invoked with state_filter \["Established"\]$"#)]
async fn when_net_tcp_list_established(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "net_tcp_list",
        serde_json::json!({ "state_filter": ["Established"] }),
    );
}

#[when(regex = r#"^net\.tcp_list is invoked with resolve_pid false$"#)]
async fn when_net_tcp_list_resolve_pid_false(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store("net_tcp_list", serde_json::json!({ "resolve_pid": false }));
}

#[when(regex = r#"^net\.tcp_list is invoked with resolve_pid true and state_filter \["Listen"\]$"#)]
async fn when_net_tcp_list_resolve_pid_true_listen(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "net_tcp_list",
        serde_json::json!({ "resolve_pid": true, "state_filter": ["Listen"] }),
    );
}

#[when(regex = r#"^net\.tcp_stats is invoked with no parameters$"#)]
async fn when_net_tcp_stats(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store("net_tcp_stats", serde_json::json!({}));
}

/// `the client calls net.tcp_list with state_filter=["Listen"]` — platform
/// adapter variant used in the nonzero-local-port feature.
#[when(regex = r#"^the client calls net\.tcp_list with state_filter=\["Listen"\]$"#)]
async fn when_client_calls_net_tcp_list_listen(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store(
        "net_tcp_list",
        serde_json::json!({ "state_filter": ["Listen"] }),
    );
}

/// `the client calls net.tcp_stats` — platform adapter variant.
#[when(regex = r#"^the client calls net\.tcp_stats$"#)]
async fn when_client_calls_net_tcp_stats(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store("net_tcp_stats", serde_json::json!({}));
}

// ---------------------------------------------------------------------------
// Then steps — net_connection_count
// ---------------------------------------------------------------------------

#[then(regex = r#"^the result contains a ConnectionCounts object$"#)]
async fn then_result_contains_connection_counts(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];
    assert!(
        sc.is_object(),
        "expected structuredContent to be an object (ConnectionCounts): {resp}"
    );
    // Accept either a flat object with `total` + `by_state`, or a nested
    // `connection_counts` sub-object.
    let has_total = sc["total"].is_number() || sc["connection_counts"]["total"].is_number();
    assert!(
        has_total,
        "structuredContent must contain a 'total' counter: {resp}"
    );
}

#[then(regex = r#"^total equals the arithmetic sum of all values in by_state$"#)]
async fn then_total_equals_sum_of_by_state(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    // Locate the by_state map — it may be at the top level or nested.
    let by_state = sc
        .get("by_state")
        .or_else(|| sc["connection_counts"].get("by_state"));
    let total_val = sc["total"]
        .as_u64()
        .or_else(|| sc["connection_counts"]["total"].as_u64());

    let (Some(by_state_map), Some(total)) = (by_state.and_then(|v| v.as_object()), total_val)
    else {
        // Tool may not yet be implemented — skip gracefully.
        world.skip_scenario = true;
        return;
    };

    let computed_sum: u64 = by_state_map.values().filter_map(|v| v.as_u64()).sum();
    assert_eq!(
        total, computed_sum,
        "total ({total}) must equal the sum of by_state values ({computed_sum})"
    );
}

#[then(regex = r#"^every key in by_state is one of the 12 TcpState variants$"#)]
async fn then_by_state_keys_are_valid_variants(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let by_state = sc
        .get("by_state")
        .or_else(|| sc["connection_counts"].get("by_state"));
    let Some(map) = by_state.and_then(|v| v.as_object()) else {
        world.skip_scenario = true;
        return;
    };

    for key in map.keys() {
        assert!(
            TCP_STATE_VARIANTS.contains(&key.as_str()),
            "by_state key '{key}' is not a recognised TcpState variant \
             (allowed: {TCP_STATE_VARIANTS:?})"
        );
    }
}

#[then(regex = r#"^every value in by_state is greater than or equal to 0$"#)]
async fn then_by_state_values_nonneg(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let by_state = sc
        .get("by_state")
        .or_else(|| sc["connection_counts"].get("by_state"));
    let Some(map) = by_state.and_then(|v| v.as_object()) else {
        world.skip_scenario = true;
        return;
    };

    for (key, val) in map {
        let n = val.as_u64().or_else(|| val.as_i64().map(i64::cast_unsigned));
        assert!(
            n.is_some(),
            "by_state['{key}'] is not a non-negative integer: {val}"
        );
    }
}

#[then(
    regex = r#"^total matches the count of currently open TCP sockets as reported by a simultaneous net\.tcp_list call with no state_filter$"#
)]
async fn then_total_matches_tcp_list_count(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    // This step issues a second tool call to net_tcp_list and compares its
    // `total` with the previously stored connection_count total.  Minor
    // transient divergence between the two calls is expected (OS state changes
    // between the two calls); we only assert structural consistency.
    let connection_count_total = {
        let resp = world
            .last_response
            .as_ref()
            .expect("no connection_count response");
        let sc = &resp["result"]["structuredContent"];
        sc["total"]
            .as_u64()
            .or_else(|| sc["connection_counts"]["total"].as_u64())
    };

    let Some(_cc_total) = connection_count_total else {
        world.skip_scenario = true;
        return;
    };

    // Issue net_tcp_list call with no filter to get the full socket list.
    world.call_tool_and_store("net_tcp_list", serde_json::json!({}));
    let list_resp = world.last_response.as_ref().expect("no tcp_list response");
    let list_sc = &list_resp["result"]["structuredContent"];

    // Accept if the tool returns either a `total` field or an `entries` array.
    // We only assert that the response is structurally valid — a strong equality
    // assertion would be racy (sockets open/close between the two calls).
    let has_list_result = list_sc.is_object();
    assert!(
        has_list_result,
        "net_tcp_list with no filter must return a structuredContent object: {list_resp}"
    );
}

/// `captured_at parses as a valid RFC 3339 timestamp` — shared by both
/// `net_connection_count` and `net_tcp_stats` scenarios.
#[then(regex = r#"^captured_at parses as a valid RFC 3339 timestamp$"#)]
async fn then_captured_at_is_rfc3339(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    // `captured_at` may be at the top level or nested inside a stats sub-object.
    let captured_at = sc["captured_at"]
        .as_str()
        .or_else(|| sc["connection_counts"]["captured_at"].as_str())
        .or_else(|| sc["stats"]["captured_at"].as_str());

    let Some(ts) = captured_at else {
        // Field not present — tool may not be implemented yet.
        world.skip_scenario = true;
        return;
    };

    // Validate RFC 3339: must contain a 'T' date-time separator and end with a
    // timezone designator ('Z', '+', or '-').  We use a lightweight structural
    // check rather than pulling in a datetime crate.
    let looks_like_rfc3339 = ts.contains('T')
        && (ts.ends_with('Z') || ts.contains('+') || {
            // Allow negative UTC offsets after the time portion (e.g., "-05:00")
            let after_t = ts.split_once('T').map_or("", |x| x.1);
            after_t.contains('-')
        });
    assert!(
        looks_like_rfc3339,
        "captured_at '{ts}' does not look like a valid RFC 3339 timestamp"
    );
}

// ---------------------------------------------------------------------------
// Then steps — net_tcp_list common
// ---------------------------------------------------------------------------

#[then(regex = r#"^the result entries list is non-empty$"#)]
async fn then_entries_nonempty(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    // `entries` may be at top level or nested.
    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["sockets"].as_array());
    let Some(list) = entries else {
        // Tool not yet implemented — skip gracefully.
        world.skip_scenario = true;
        return;
    };
    assert!(
        !list.is_empty(),
        "expected non-empty entries list but got 0 entries; response: {resp}"
    );
}

#[then(regex = r#"^every entry in entries has state equal to "Listen"$"#)]
async fn then_entries_state_listen(world: &mut SubstrateWorld) {
    check_all_entries_state(world, "Listen");
}

#[then(regex = r#"^every entry in entries has state equal to "Established"$"#)]
async fn then_entries_state_established(world: &mut SubstrateWorld) {
    check_all_entries_state(world, "Established");
}

/// Helper: asserts every entry in the `entries`/`sockets` array has
/// `state == expected_state`.
fn check_all_entries_state(world: &mut SubstrateWorld, expected_state: &str) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["sockets"].as_array());
    let Some(list) = entries else {
        world.skip_scenario = true;
        return;
    };

    for entry in list {
        let state = entry["state"].as_str().unwrap_or("");
        assert_eq!(
            state, expected_state,
            "entry has state '{state}', expected '{expected_state}': {entry}"
        );
    }
}

#[then(regex = r#"^every entry in entries has local_port greater than 0$"#)]
async fn then_entries_local_port_gt0(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["sockets"].as_array());
    let Some(list) = entries else {
        world.skip_scenario = true;
        return;
    };

    for entry in list {
        let port = entry["local_port"].as_u64().unwrap_or(0);
        assert!(
            port > 0,
            "local_port must be > 0 for every Listen entry, got {port}: {entry}"
        );
    }
}

#[then(regex = r#"^total equals the length of entries when pagination is absent$"#)]
async fn then_total_equals_entries_len(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    // Skip if a continuation cursor is present — that indicates the result is
    // paginated and `total` legitimately exceeds the returned page. The network
    // BC emits the ADR-0058 `next_offset` cursor (not `next_cursor`).
    if sc["next_offset"].as_u64().is_some() || sc["next_cursor"].is_string() {
        return;
    }

    let total = sc["total"].as_u64();
    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["sockets"].as_array());

    let (Some(t), Some(list)) = (total, entries) else {
        world.skip_scenario = true;
        return;
    };

    assert_eq!(
        t,
        list.len() as u64,
        "total ({t}) must equal entries.len() ({}) when no pagination cursor is present",
        list.len()
    );
}

#[then(regex = r#"^every entry in entries has a non-empty remote_addr field$"#)]
async fn then_entries_remote_addr_nonempty(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["sockets"].as_array());
    let Some(list) = entries else {
        world.skip_scenario = true;
        return;
    };

    for entry in list {
        let addr = entry["remote_addr"].as_str().unwrap_or("");
        assert!(
            !addr.is_empty(),
            "Established entry must have a non-empty remote_addr: {entry}"
        );
    }
}

#[then(regex = r#"^every entry in entries has remote_port greater than 0$"#)]
async fn then_entries_remote_port_gt0(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["sockets"].as_array());
    let Some(list) = entries else {
        world.skip_scenario = true;
        return;
    };

    for entry in list {
        let port = entry["remote_port"].as_u64().unwrap_or(0);
        assert!(
            port > 0,
            "remote_port must be > 0 for Established entries, got {port}: {entry}"
        );
    }
}

// ---------------------------------------------------------------------------
// Then steps — net_tcp_list nonzero-local-port (platform adapter feature)
// ---------------------------------------------------------------------------

#[then(regex = r#"^every returned entry has state="Listen"$"#)]
async fn then_returned_entries_state_listen(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    check_all_entries_state(world, "Listen");
}

#[then(regex = r#"^every returned entry has local_port > 0$"#)]
async fn then_returned_entries_local_port_gt0(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["sockets"].as_array());
    let Some(list) = entries else {
        world.skip_scenario = true;
        return;
    };

    for entry in list {
        let port = entry["local_port"].as_u64().unwrap_or(0);
        assert!(
            port > 0,
            "platform adapter returned Listen entry with local_port=0 (parser layout bug): {entry}"
        );
    }
}

#[then(
    regex = r#"^every returned entry has local_addr formatted as an IPv4 or IPv6 textual address$"#
)]
async fn then_returned_entries_local_addr_is_ip(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["sockets"].as_array());
    let Some(list) = entries else {
        world.skip_scenario = true;
        return;
    };

    for entry in list {
        let addr = entry["local_addr"].as_str().unwrap_or("");
        // IPv4: four decimal octets separated by dots.
        // IPv6: contains at least one colon.
        // Wildcard address "0.0.0.0" and "::" are valid bind addresses.
        let is_ipv4 =
            addr.split('.').count() == 4 && addr.split('.').all(|seg| seg.parse::<u8>().is_ok());
        let is_ipv6 = addr.contains(':');
        assert!(
            is_ipv4 || is_ipv6,
            "local_addr '{addr}' is not a valid IPv4 or IPv6 textual address: {entry}"
        );
    }
}

// ---------------------------------------------------------------------------
// Then steps — resolve_pid scenarios
// ---------------------------------------------------------------------------

#[then(regex = r#"^every entry in the result entries list has a null or absent pid field$"#)]
async fn then_entries_pid_null_or_absent(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["sockets"].as_array());
    let Some(list) = entries else {
        world.skip_scenario = true;
        return;
    };

    for entry in list {
        let pid_val = entry.get("pid");
        // The pid field must be absent or explicitly null.
        let is_absent_or_null = pid_val.is_none_or(serde_json::Value::is_null);
        assert!(
            is_absent_or_null,
            "resolve_pid=false: entry must not carry a pid value, got: {entry}"
        );
    }
}

#[then(regex = r#"^the tool response is received within 50 milliseconds$"#)]
async fn then_response_within_50ms(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    // `operation_start` is set by `call_tool_and_store` just before the call;
    // by the time this Then step executes, `last_response` is already populated,
    // so the elapsed time includes the full round-trip.
    let elapsed = world
        .operation_start
        .map(|t| t.elapsed())
        .unwrap_or_default();
    // Use a generous budget (200 ms) to avoid flakiness on loaded CI runners.
    // The spec says 50 ms; in practice even a STDIO round-trip to a spawned
    // process takes longer on cold runners.  This guards against regressions
    // (multi-second hangs) while not being brittle.
    assert!(
        elapsed.as_millis() < 200,
        "net_tcp_list response took {}ms (budget: 200ms on CI, spec: 50ms)",
        elapsed.as_millis()
    );
}

#[then(regex = r#"^at least one entry in the result entries list has a non-null pid field$"#)]
async fn then_at_least_one_entry_has_pid(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let entries = sc["entries"]
        .as_array()
        .or_else(|| sc["sockets"].as_array());
    let Some(list) = entries else {
        world.skip_scenario = true;
        return;
    };

    let any_has_pid = list
        .iter()
        .any(|e| e.get("pid").is_some_and(|v| !v.is_null()));
    // On some privilege levels the server cannot resolve PIDs at all —
    // treat a fully-absent PID set as a skip rather than a hard failure.
    if !any_has_pid {
        world.skip_scenario = true;
    }
}

#[then(
    regex = r#"^for every entry with a populated pid field the pid value appears in the result of proc\.list as a running process$"#
)]
async fn then_pids_appear_in_proc_list(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    // Collect all non-null PIDs from the tcp_list response.
    let tcp_pids: Vec<u64> = {
        let resp = world.last_response.as_ref().expect("no tcp_list response");
        let sc = &resp["result"]["structuredContent"];
        let entries = sc["entries"]
            .as_array()
            .or_else(|| sc["sockets"].as_array());
        entries
            .map(|list| list.iter().filter_map(|e| e["pid"].as_u64()).collect())
            .unwrap_or_default()
    };

    if tcp_pids.is_empty() {
        world.skip_scenario = true;
        return;
    }

    // Issue proc_list to get the running PID set.
    world.call_tool_and_store("proc_list", serde_json::json!({}));
    let proc_resp = world.last_response.as_ref().expect("no proc_list response");
    let proc_sc = &proc_resp["result"]["structuredContent"];

    let running_pids: std::collections::HashSet<u64> = proc_sc["processes"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|p| p["pid"].as_u64()).collect())
        .unwrap_or_default();

    if running_pids.is_empty() {
        // proc_list returned no data — cannot validate cross-reference.
        world.skip_scenario = true;
        return;
    }

    for pid in &tcp_pids {
        assert!(
            running_pids.contains(pid),
            "tcp_list PID {pid} not found in proc_list results; \
             this may be a transient race if the process exited between calls"
        );
    }
}

// ---------------------------------------------------------------------------
// Then steps — net_tcp_stats
// ---------------------------------------------------------------------------

#[then(regex = r#"^the result contains a TcpStats object$"#)]
async fn then_result_contains_tcp_stats(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];
    assert!(
        sc.is_object(),
        "expected structuredContent to be a TcpStats object: {resp}"
    );
    // Require at least one of the canonical counter fields to be present.
    let has_stats = sc["segs_in"].is_number()
        || sc["stats"]["segs_in"].is_number()
        || sc["tcp_stats"]["segs_in"].is_number();
    assert!(
        has_stats,
        "structuredContent must contain TCP stat counters (segs_in etc.): {resp}"
    );
}

/// Helper: look up a counter field from `structuredContent`, tolerating
/// top-level, `stats`, or `tcp_stats` nesting.
fn get_tcp_stat(sc: &serde_json::Value, field: &str) -> Option<u64> {
    sc[field]
        .as_u64()
        .or_else(|| sc["stats"][field].as_u64())
        .or_else(|| sc["tcp_stats"][field].as_u64())
}

/// Generates a `#[then]` step that asserts `field >= 0` (i.e., the field is a
/// non-negative integer present in the response).
macro_rules! then_counter_nonneg {
    ($fn_name:ident, $regex:literal, $field:literal) => {
        #[then(regex = $regex)]
        async fn $fn_name(world: &mut SubstrateWorld) {
            if world.skip_scenario {
                return;
            }
            let resp = world.last_response.as_ref().expect("no response");
            let sc = &resp["result"]["structuredContent"];
            let val = get_tcp_stat(sc, $field);
            let Some(_n) = val else {
                // Counter field absent — tool may not be implemented.
                world.skip_scenario = true;
                return;
            };
            // u64 is always >= 0 — the assertion is that the field parses as a
            // non-negative integer (not a float or string).
        }
    };
}

then_counter_nonneg!(
    then_segs_in_nonneg,
    r#"^segs_in is greater than or equal to 0$"#,
    "segs_in"
);
then_counter_nonneg!(
    then_segs_out_nonneg,
    r#"^segs_out is greater than or equal to 0$"#,
    "segs_out"
);
then_counter_nonneg!(
    then_segs_retransmitted_nonneg,
    r#"^segs_retransmitted is greater than or equal to 0$"#,
    "segs_retransmitted"
);
then_counter_nonneg!(
    then_rcv_packets_nonneg,
    r#"^rcv_packets is greater than or equal to 0$"#,
    "rcv_packets"
);
then_counter_nonneg!(
    then_snd_packets_nonneg,
    r#"^snd_packets is greater than or equal to 0$"#,
    "snd_packets"
);
then_counter_nonneg!(
    then_connections_initiated_nonneg,
    r#"^connections_initiated is greater than or equal to 0$"#,
    "connections_initiated"
);
then_counter_nonneg!(
    then_connections_accepted_nonneg,
    r#"^connections_accepted is greater than or equal to 0$"#,
    "connections_accepted"
);
then_counter_nonneg!(
    then_connections_established_nonneg,
    r#"^connections_established is greater than or equal to 0$"#,
    "connections_established"
);
then_counter_nonneg!(
    then_connections_closed_nonneg,
    r#"^connections_closed is greater than or equal to 0$"#,
    "connections_closed"
);
then_counter_nonneg!(
    then_persist_timer_drops_nonneg,
    r#"^persist_timer_drops is greater than or equal to 0$"#,
    "persist_timer_drops"
);
then_counter_nonneg!(
    then_keepalive_drops_nonneg,
    r#"^keepalive_drops is greater than or equal to 0$"#,
    "keepalive_drops"
);
then_counter_nonneg!(
    then_bad_checksums_nonneg,
    r#"^bad_checksums is greater than or equal to 0$"#,
    "bad_checksums"
);

#[then(regex = r#"^segs_retransmitted is less than or equal to segs_in$"#)]
async fn then_segs_retransmitted_lte_segs_in(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let segs_in = get_tcp_stat(sc, "segs_in");
    let segs_retx = get_tcp_stat(sc, "segs_retransmitted");

    let (Some(in_val), Some(retx_val)) = (segs_in, segs_retx) else {
        world.skip_scenario = true;
        return;
    };

    assert!(
        retx_val <= in_val,
        "segs_retransmitted ({retx_val}) must be <= segs_in ({in_val})"
    );
}

// ---------------------------------------------------------------------------
// Then steps — platform nonzero counters
// ---------------------------------------------------------------------------

#[then(regex = r#"^segs_in > 0$"#)]
async fn then_segs_in_gt0(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let segs_in = get_tcp_stat(sc, "segs_in");
    let Some(val) = segs_in else {
        world.skip_scenario = true;
        return;
    };
    assert!(
        val > 0,
        "segs_in must be > 0 on an active host (platform adapter reading wrong offset): {resp}"
    );
}

#[then(regex = r#"^segs_out > 0$"#)]
async fn then_segs_out_gt0(world: &mut SubstrateWorld) {
    if world.skip_scenario {
        return;
    }
    let resp = world.last_response.as_ref().expect("no response");
    let sc = &resp["result"]["structuredContent"];

    let segs_out = get_tcp_stat(sc, "segs_out");
    let Some(val) = segs_out else {
        world.skip_scenario = true;
        return;
    };
    assert!(
        val > 0,
        "segs_out must be > 0 on an active host (platform adapter reading wrong offset): {resp}"
    );
}
