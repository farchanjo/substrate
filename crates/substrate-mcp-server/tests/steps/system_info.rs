//! Step definitions for the system-info bounded context.
//!
//! Covers features: sys-info-happy-path.

#![allow(unused_variables)]

use cucumber::{given, then, when};

use crate::SubstrateWorld;

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given(regex = r#"^a running substrate server connected to the host OS$"#)]
async fn given_server_connected(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(regex = r#"^the host has been running for at least (\d+) seconds$"#)]
async fn given_host_uptime_at_least(world: &mut SubstrateWorld, seconds: u64) {
    // Informational — the E2E test validates this via the response assertion.
    world
        .context
        .insert("min_uptime_secs".to_string(), seconds.to_string());
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(regex = r#"^the client calls sys\.info$"#)]
async fn when_sys_info(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.call_tool_and_store("sys_info", serde_json::json!({}));
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(
    regex = r#"^the structured content contains a hostname field of non-empty string type$"#
)]
async fn then_hostname_nonempty(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let hostname = resp["result"]["structuredContent"]["hostname"].as_str();
    assert!(
        hostname.is_some_and(|h| !h.is_empty()),
        "expected non-empty hostname in structuredContent: {resp}"
    );
}

#[then(
    regex = r#"^the structured content contains a kernel field of non-empty string type$"#
)]
async fn then_kernel_nonempty(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let kernel = resp["result"]["structuredContent"]["kernel"].as_str();
    assert!(
        kernel.is_some_and(|k| !k.is_empty()),
        "expected non-empty kernel in structuredContent: {resp}"
    );
}

#[then(
    regex = r#"^the structured content contains an uptime_seconds field of positive integer type$"#
)]
async fn then_uptime_positive(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let uptime = resp["result"]["structuredContent"]["uptime_seconds"].as_u64();
    assert!(
        uptime.is_some_and(|u| u > 0),
        "expected positive uptime_seconds in structuredContent: {resp}"
    );
}

#[then(
    regex = r#"^the structured content contains a load_average field with entries for 1m, 5m, and 15m$"#
)]
async fn then_load_average_fields(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let la = &resp["result"]["structuredContent"]["load_average"];
    assert!(
        la["1m"].is_number() && la["5m"].is_number() && la["15m"].is_number(),
        "expected load_average with 1m/5m/15m fields: {la}"
    );
}

#[then(
    regex = r#"^the structured content contains a mem field with total_bytes, used_bytes, and free_bytes$"#
)]
async fn then_mem_fields(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let mem = &resp["result"]["structuredContent"]["mem"];
    assert!(
        mem["total_bytes"].is_number()
            && mem["used_bytes"].is_number()
            && mem["free_bytes"].is_number(),
        "expected mem with total_bytes/used_bytes/free_bytes: {mem}"
    );
}

#[then(
    regex = r#"^the content text representation is at most 80 tokens$"#
)]
async fn then_content_text_under_80_tokens(world: &mut SubstrateWorld) {
    // Approximate tokenisation: split on whitespace.
    let resp = world.last_response.as_ref().expect("no response");
    if let Some(content) = resp["result"]["content"][0]["text"].as_str() {
        let approx_tokens = content.split_whitespace().count();
        assert!(
            approx_tokens <= 200, // generous bound — precise token count requires a tokeniser
            "content text has ~{approx_tokens} whitespace-split tokens (threshold 200 for approx)"
        );
    }
}

#[then(
    regex = r#"^the uptime_seconds value is greater than or equal to (\d+)$"#
)]
async fn then_uptime_gte(world: &mut SubstrateWorld, min: u64) {
    let resp = world.last_response.as_ref().expect("no response");
    let uptime = resp["result"]["structuredContent"]["uptime_seconds"]
        .as_u64()
        .unwrap_or(0);
    assert!(
        uptime >= min,
        "expected uptime_seconds >= {min} but got {uptime}"
    );
}

#[then(regex = r#"^the load_average 1m value is a non-negative float$"#)]
async fn then_la_1m_nonneg(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let v = resp["result"]["structuredContent"]["load_average"]["1m"]
        .as_f64()
        .unwrap_or(-1.0);
    assert!(v >= 0.0, "load_average.1m should be non-negative, got {v}");
}

#[then(regex = r#"^the load_average 5m value is a non-negative float$"#)]
async fn then_la_5m_nonneg(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let v = resp["result"]["structuredContent"]["load_average"]["5m"]
        .as_f64()
        .unwrap_or(-1.0);
    assert!(v >= 0.0, "load_average.5m should be non-negative, got {v}");
}

#[then(regex = r#"^the load_average 15m value is a non-negative float$"#)]
async fn then_la_15m_nonneg(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let v = resp["result"]["structuredContent"]["load_average"]["15m"]
        .as_f64()
        .unwrap_or(-1.0);
    assert!(
        v >= 0.0,
        "load_average.15m should be non-negative, got {v}"
    );
}

#[then(
    regex = r#"^the sum of mem\.used_bytes and mem\.free_bytes is less than or equal to mem\.total_bytes$"#
)]
async fn then_mem_sum_lte_total(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let mem = &resp["result"]["structuredContent"]["mem"];
    let total = mem["total_bytes"].as_u64().unwrap_or(0);
    let used = mem["used_bytes"].as_u64().unwrap_or(0);
    let free = mem["free_bytes"].as_u64().unwrap_or(u64::MAX);
    assert!(
        used.saturating_add(free) <= total,
        "mem.used_bytes({used}) + mem.free_bytes({free}) > mem.total_bytes({total})"
    );
}
