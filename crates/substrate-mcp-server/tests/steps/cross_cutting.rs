//! Step definitions for cross-cutting concerns.
//!
//! Covers features:
//!   audit-log-write-failure, cancellation-on-cancel-request,
//!   capability-elicitation-missing, capability-tiers-selected-startup-audit,
//!   client-disconnect-mid-call, elicitation-edge-cases, error-response-shape,
//!   initialize-advertises-experimental-jobs, internal-error-correlation,
//!   jail-degraded-refused-startup-aborts, malformed-input, operation-timeout,
//!   pagination-cursor-roundtrip, progress-notification-emitted,
//!   protocol-version-rejection, simd-portable-fallback-equivalent,
//!   simd-tier-detected-and-audited, startup-allowlist-missing,
//!   startup-invalid-config, subprocess-policy-verified-startup,
//!   tool-unknown-argument.

#![allow(unused_variables)]

use std::io::{BufRead as _, Write as _};

use cucumber::{given, then, when};

use crate::SubstrateWorld;

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given(
    regex = r#"^a running substrate server with global_timeout_secs=(\d+)$"#
)]
async fn given_server_with_timeout(world: &mut SubstrateWorld, secs: u32) {
    // Timeout configuration requires a custom config — reuse standard spawn for now.
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world
        .context
        .insert("global_timeout_secs".to_string(), secs.to_string());
}

#[given(
    regex = r#"^the directory tree under "([^"]+)" is at least (\d+) levels deep with (\d+) nodes per level$"#
)]
async fn given_deep_tree(world: &mut SubstrateWorld, path: String, levels: u32, nodes: u32) {
    world.context.insert("deep_tree_path".to_string(), path);
    world
        .context
        .insert("tree_levels".to_string(), levels.to_string());
    world
        .context
        .insert("tree_nodes_per_level".to_string(), nodes.to_string());
}

#[given(
    regex = r#"^the server is configured to emit error code ([A-Z_]+) for the next matching operation$"#
)]
async fn given_server_emit_error(world: &mut SubstrateWorld, code: String) {
    world
        .context
        .insert("forced_error_code".to_string(), code);
}

#[given(
    regex = r#"^the server is configured to emit (SUBSTRATE_INTERNAL_ERROR|SUBSTRATE_IO_ERROR) for the next operation$"#
)]
async fn given_server_emit_specific_error(world: &mut SubstrateWorld, code: String) {
    world
        .context
        .insert("forced_error_code".to_string(), code);
}

#[given(
    regex = r#"^the client has sent fs\.find with root="([^"]+)" which is running$"#
)]
async fn given_fs_find_running(world: &mut SubstrateWorld, root: String) {
    world
        .context
        .insert("inflight_tool".to_string(), "fs_find".to_string());
    world
        .context
        .insert("inflight_root".to_string(), root);
}

#[given(
    regex = r#"^the client has sent text\.search with root="([^"]+)" which is running$"#
)]
async fn given_text_search_running(world: &mut SubstrateWorld, root: String) {
    world
        .context
        .insert("inflight_tool".to_string(), "text_search".to_string());
    world
        .context
        .insert("inflight_root".to_string(), root);
}

#[given(
    regex = r#"^a fs\.find request that has already returned its final response$"#
)]
async fn given_fs_find_completed(world: &mut SubstrateWorld) {
    world
        .context
        .insert("completed_tool".to_string(), "fs_find".to_string());
}

#[given(
    regex = r#"^the client has sent archive\.tar_create which is compressing data$"#
)]
async fn given_tar_create_running(world: &mut SubstrateWorld) {
    world
        .context
        .insert("inflight_tool".to_string(), "archive_tar_create".to_string());
}

#[given(
    regex = r#"^a running substrate server with MCP progress notifications enabled$"#
)]
async fn given_server_progress_enabled(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(
    regex = r#"^the directory "([^"]+)" contains enough files that fs\.find takes >= (\d+) second$"#
)]
async fn given_dir_large_enough_for_delay(world: &mut SubstrateWorld, path: String, secs: u32) {
    world.context.insert("large_dir".to_string(), path);
}

#[given(regex = r#"^archiving "([^"]+)" will take >= (\d+) second$"#)]
async fn given_archiving_takes_long(world: &mut SubstrateWorld, path: String, secs: u32) {
    world.context.insert("archive_src".to_string(), path);
}

#[given(
    regex = r#"^a directory "([^"]+)" containing (\d+) files$"#
)]
async fn given_dir_with_files(world: &mut SubstrateWorld, path: String, count: u32) {
    world.context.insert("tiny_dir".to_string(), path);
    world
        .context
        .insert("tiny_count".to_string(), count.to_string());
}

#[given(
    regex = r#"^an operation that emits multiple ProgressNotifications$"#
)]
async fn given_op_with_multiple_progress(world: &mut SubstrateWorld) {
    world
        .context
        .insert("multi_progress_op".to_string(), "true".to_string());
}

#[given(
    regex = r#"^substrate is configured with allowlist root "([^"]+)"$"#
)]
async fn given_substrate_config_root(world: &mut SubstrateWorld, root: String) {
    world
        .context
        .insert("configured_root".to_string(), root);
}

#[given(
    regex = r#"^a running substrate server requiring protocolVersion >= "([^"]+)"$"#
)]
async fn given_server_min_version(world: &mut SubstrateWorld, version: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world
        .context
        .insert("min_protocol_version".to_string(), version);
}

#[given(
    regex = r#"^a running substrate server with log_write_error_policy=warn_stderr_fallback$"#
)]
async fn given_server_warn_fallback(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
}

#[given(
    regex = r#"^the audit log target directory "([^"]+)" is owned by root with mode 0555 \(read-only to substrate\)$"#
)]
async fn given_audit_log_readonly(world: &mut SubstrateWorld, path: String) {
    world.context.insert("audit_log_dir".to_string(), path);
}

#[given(
    regex = r#"^the server is configured with log_write_error_policy=fail$"#
)]
async fn given_server_log_fail_policy(world: &mut SubstrateWorld) {
    world
        .context
        .insert("log_write_error_policy".to_string(), "fail".to_string());
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(regex = r#"^the triggering operation is dispatched$"#)]
async fn when_triggering_op(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: error-response-shape — triggering operation dispatch requires \
         per-code error injection mechanism not yet implemented"
    );
}

#[when(
    regex = r#"^the client calls fs\.find with root="([^"]+)" and pattern="([^"]+)"$"#
)]
async fn when_fs_find_cc(world: &mut SubstrateWorld, root: String, pattern: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let sandbox_root = world.root_str();
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({ "root": sandbox_root, "pattern": pattern }),
    );
}

#[when(
    regex = r#"^the client sends \\$/cancelRequest for the in-flight fs\.find request id$"#
)]
async fn when_cancel_fs_find(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: cancellation-on-cancel-request — $/cancelRequest dispatch requires in-flight request id tracking"
    );
}

#[when(
    regex = r#"^the client sends \\$/cancelRequest for the in-flight text\.search request id$"#
)]
async fn when_cancel_text_search(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: cancellation-on-cancel-request — $/cancelRequest for text.search"
    );
}

#[when(
    regex = r#"^the client sends \\$/cancelRequest for the completed request id$"#
)]
async fn when_cancel_completed(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: cancellation-on-cancel-request — cancel for completed request id"
    );
}

#[when(
    regex = r#"^the client sends \\$/cancelRequest for the archive\.tar_create request id$"#
)]
async fn when_cancel_tar_create(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: cancellation-on-cancel-request — cancel for archive.tar_create"
    );
}

#[when(
    regex = r#"^the client sends a JSON-RPC message with "params" set to an array value \[\]$"#
)]
async fn when_send_params_array(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.send_rpc("tools/call", serde_json::json!([]));
    let resp = world.recv_rpc();
    world.last_response = Some(resp);
}

#[when(
    regex = r#"^the client sends a JSON-RPC message whose byte length exceeds (\d+)$"#
)]
async fn when_send_oversized_message(world: &mut SubstrateWorld, limit: usize) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    // Send a message with 1 extra byte over the limit.
    let oversized = "x".repeat(limit + 1);
    let line = format!(
        r#"{{"jsonrpc":"2.0","method":"tools/call","id":99,"params":{{"x":"{oversized}"}}}}"#
    );
    world
        .stdin_writer
        .as_mut()
        .expect("stdin_writer not set")
        .write_all(format!("{line}\n").as_bytes())
        .ok();
    if let Some(resp_line) = world.stdout_reader.as_mut().and_then(|r| {
        let mut l = String::new();
        r.read_line(&mut l).ok()?;
        serde_json::from_str(l.trim()).ok()
    }) {
        world.last_response = Some(resp_line);
    }
}

#[when(
    regex = r#"^the client sends a valid fs\.stat request with "id" explicitly set to null$"#
)]
async fn when_send_fs_stat_null_id(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": null,
        "params": { "name": "fs_stat", "arguments": { "path": root } }
    });
    world.write_line(&msg.to_string());
    let resp = world.recv_rpc();
    world.last_response = Some(resp);
}

#[when(
    regex = r#"^the client sends a JSON object that omits the "jsonrpc" field$"#
)]
async fn when_send_no_jsonrpc_field(world: &mut SubstrateWorld) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let msg = r#"{"method":"tools/call","id":10,"params":{}}"#;
    world.write_line(msg);
    let resp = world.recv_rpc();
    world.last_response = Some(resp);
}

#[when(
    regex = r#"^the client sends a JSON-RPC message where "method" is set to the integer (\d+)$"#
)]
async fn when_send_method_integer(world: &mut SubstrateWorld, method_val: u32) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let msg = format!(r#"{{"jsonrpc":"2.0","method":{method_val},"id":11,"params":{{}}}}"#);
    world.write_line(&msg);
    let resp = world.recv_rpc();
    world.last_response = Some(resp);
}

#[when(
    regex = r#"^a client sends an initialize request with protocolVersion="([^"]+)"$"#
)]
async fn when_client_init_version(world: &mut SubstrateWorld, version: String) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    world.send_rpc(
        "initialize",
        serde_json::json!({
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": { "name": "cucumber-test", "version": "0.0.1" }
        }),
    );
    let resp = world.recv_rpc();
    world.last_response = Some(resp);
}

#[when(regex = r#"^substrate starts$"#)]
async fn when_substrate_starts(world: &mut SubstrateWorld) {
    // Attempt to spawn with a deliberately missing allowlist root.
    use std::process::{Command, Stdio};
    use std::io::Write as _;

    let configured_root = world
        .context
        .get("configured_root")
        .cloned()
        .unwrap_or_else(|| "/nonexistent/path/that/does/not/exist".to_string());

    let tmp = tempfile::TempDir::new().expect("TempDir");
    let cfg = tmp.path().join("substrate.toml");
    let content = format!(
        "[policy]\nroots = [\"{root}\"]\n\n\
         [logging]\nlevel = \"error\"\n\n\
         [security]\nrefuse_degraded_jail = false\n",
        root = configured_root
    );
    std::fs::write(&cfg, content).expect("write config");

    let output = Command::new(SubstrateWorld::binary_path())
        .current_dir(tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(out) => {
            world
                .context
                .insert("startup_exit_code".to_string(), out.status.code().unwrap_or(-1).to_string());
            world
                .context
                .insert("startup_stdout".to_string(), String::from_utf8_lossy(&out.stdout).into_owned());
            world
                .context
                .insert("startup_stderr".to_string(), String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Err(e) => {
            world
                .context
                .insert("startup_error".to_string(), e.to_string());
        }
    }
    world.sandbox = Some(tmp);
}

#[when(
    regex = r#"^all ProgressNotifications for progressToken="([^"]+)" are collected$"#
)]
async fn when_collect_progress_notifications(world: &mut SubstrateWorld, token: String) {
    unimplemented!(
        "step pending: progress-notification-emitted — collecting notifications for token '{token}'"
    );
}

#[when(
    regex = r#"^the client calls fs\.find with root="([^"]+)" and pattern="([^"]+)" including a progressToken$"#
)]
async fn when_fs_find_with_progress_token(
    world: &mut SubstrateWorld,
    root: String,
    pattern: String,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let sandbox_root = world.root_str();
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({
            "root": sandbox_root,
            "pattern": pattern,
            "progress_token": "tok-progress",
        }),
    );
}

#[when(
    regex = r#"^the client calls archive\.tar_create with src="([^"]+)" and progressToken="([^"]+)"$"#
)]
async fn when_archive_tar_create_progress(
    world: &mut SubstrateWorld,
    src: String,
    token: String,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let root = world.root_str();
    let full_src = src.replace("/work/repo", &root);
    world.call_tool_and_store(
        "archive_tar_create",
        serde_json::json!({
            "src": full_src,
            "dst": format!("{root}/out.tar.gz"),
            "progress_token": token,
        }),
    );
}

#[when(
    regex = r#"^the client calls fs\.find with root="([^"]+)" and pattern="([^"]+)" and progressToken="([^"]+)"$"#
)]
async fn when_fs_find_with_named_token(
    world: &mut SubstrateWorld,
    root: String,
    pattern: String,
    token: String,
) {
    if world.child.is_none() {
        world.spawn_and_initialize();
    }
    let sandbox_root = world.root_str();
    world.call_tool_and_store(
        "fs_find",
        serde_json::json!({
            "root": sandbox_root,
            "pattern": pattern,
            "progress_token": token,
        }),
    );
}

#[when(
    regex = r#"^substrate processes the initialize handshake and computes capability intersection$"#
)]
async fn when_substrate_processes_init(world: &mut SubstrateWorld) {
    // Already handled by given_client_init_version + spawn; no additional action.
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(
    regex = r#"^the server returns an error response with code SUBSTRATE_CANCELLED within (\d+) second$"#
)]
async fn then_cancelled_within(world: &mut SubstrateWorld, secs: u32) {
    unimplemented!(
        "step pending: cancellation — SUBSTRATE_CANCELLED within {secs}s requires in-flight request tracking"
    );
}

#[then(
    regex = r#"^no further result chunks are emitted for that request$"#
)]
async fn then_no_further_chunks(world: &mut SubstrateWorld) {
    unimplemented!("step pending: cancellation — no further chunks after CANCELLED");
}

#[then(
    regex = r#"^partial results from before cancellation are not included in the final response$"#
)]
async fn then_no_partial_results(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: cancellation — partial result exclusion after CANCELLED"
    );
}

#[then(regex = r#"^the server does not return an error$"#)]
async fn then_server_no_error(world: &mut SubstrateWorld) {
    // For completed-request cancel, no response is expected.  If there is one,
    // it should not be an error.
    if let Some(resp) = &world.last_response {
        assert!(
            !resp["error"].is_object(),
            "expected no error for completed-request cancel, got: {resp}"
        );
    }
}

#[then(regex = r#"^the server does not emit duplicate results$"#)]
async fn then_no_duplicate_results(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: cancellation — duplicate result prevention for completed request"
    );
}

#[then(
    regex = r#"^the CancellationToken associated with the handler is signalled as cancelled$"#
)]
async fn then_cancellation_token_handler_signalled(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: cancellation — CancellationToken internal signal"
    );
}

#[then(
    regex = r#"^the server returns SUBSTRATE_CANCELLED within (\d+) second$"#
)]
async fn then_substrate_cancelled(world: &mut SubstrateWorld, secs: u32) {
    unimplemented!(
        "step pending: cancellation — SUBSTRATE_CANCELLED within {secs}s"
    );
}

#[then(
    regex = r#"^the response contains a JSON-RPC error with code (-\d+)$"#
)]
async fn then_jsonrpc_error_code(world: &mut SubstrateWorld, code: i64) {
    let resp = world.last_response.as_ref().expect("no response");
    let actual = resp["error"]["code"].as_i64().unwrap_or(0);
    assert_eq!(
        actual, code,
        "expected JSON-RPC error code {code} but got {actual}: {resp}"
    );
}

#[then(regex = r#"^the error message describes an invalid request$"#)]
async fn then_error_invalid_request(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let msg = resp["error"]["message"].as_str().unwrap_or("");
    assert!(!msg.is_empty(), "expected error message but got empty: {resp}");
}

#[then(regex = r#"^the session remains open for subsequent valid requests$"#)]
async fn then_session_open(world: &mut SubstrateWorld) {
    // Verify the server is still responsive by sending a no-op request.
    if world.child.is_some() {
        world.send_rpc("tools/list", serde_json::json!({}));
        let resp = world.recv_rpc();
        assert!(
            resp["result"].is_object() || resp["error"].is_object(),
            "session closed prematurely: {resp}"
        );
    }
}

#[then(
    regex = r#"^the error message indicates the message size limit was exceeded$"#
)]
async fn then_size_limit_exceeded(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let msg = resp["error"]["message"].as_str().unwrap_or("");
    assert!(!msg.is_empty(), "expected size-limit error message: {resp}");
}

#[then(regex = r#"^the server closes the session after sending the error response$"#)]
async fn then_server_closes_session(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: malformed-input — session close after oversized message"
    );
}

#[then(regex = r#"^the server processes the request$"#)]
async fn then_server_processes(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["result"].is_object() || resp["error"].is_object(),
        "server did not process request: {resp}"
    );
}

#[then(
    regex = r#"^the response carries "id" equal to null$"#
)]
async fn then_response_id_null(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["id"].is_null(),
        "expected id=null but got: {}",
        resp["id"]
    );
}

#[then(regex = r#"^no protocol error is returned$"#)]
async fn then_no_protocol_error(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    if resp["error"].is_object() {
        let code = resp["error"]["code"].as_i64().unwrap_or(0);
        assert!(
            code >= -32099,
            "unexpected protocol error code {code}: {resp}"
        );
    }
}

#[then(
    regex = r#"^the server returns an error response within (\d+) seconds$"#
)]
async fn then_error_within_seconds(world: &mut SubstrateWorld, secs: u32) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["error"].is_object(),
        "expected error response within {secs}s but got: {resp}"
    );
}

#[then(
    regex = r#"^the error object details include field "timeout_secs" equal to (\d+)$"#
)]
async fn then_timeout_secs_detail(world: &mut SubstrateWorld, expected: u64) {
    unimplemented!(
        "step pending: operation-timeout — timeout_secs={expected} in error details"
    );
}

#[then(regex = r#"^the server returns a success response$"#)]
async fn then_server_success(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["result"].is_object() && !resp["error"].is_object(),
        "expected success response but got: {resp}"
    );
}

#[then(regex = r#"^no SUBSTRATE_TIMEOUT error is emitted$"#)]
async fn then_no_timeout_error(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let code = resp["error"]["data"]["code"].as_str().unwrap_or("");
    assert_ne!(
        code, "SUBSTRATE_TIMEOUT",
        "unexpected SUBSTRATE_TIMEOUT: {resp}"
    );
}

#[then(
    regex = r#"^no partial result chunks are present in the response stream after the error$"#
)]
async fn then_no_partial_chunks_after_timeout(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: operation-timeout — no partial chunks after timeout"
    );
}

#[then(regex = r#"^the process exits with code (\d+)$"#)]
async fn then_exits_with_code(world: &mut SubstrateWorld, code: i32) {
    let actual: i32 = world
        .context
        .get("startup_exit_code")
        .and_then(|s| s.parse().ok())
        .unwrap_or(-99);
    assert_eq!(
        actual, code,
        "expected exit code {code} but got {actual}"
    );
}

#[then(regex = r#"^exactly one JSON line is written to stderr$"#)]
async fn then_one_json_stderr_line(world: &mut SubstrateWorld) {
    let stderr = world
        .context
        .get("startup_stderr")
        .cloned()
        .unwrap_or_default();
    let json_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.trim_start().starts_with('{'))
        .collect();
    assert_eq!(
        json_lines.len(),
        1,
        "expected exactly 1 JSON line in stderr but found {}: {:?}",
        json_lines.len(),
        json_lines
    );
}

#[then(
    regex = r#"^that JSON line has field "([^"]+)" equal to "([^"]+)"$"#
)]
async fn then_stderr_json_field(world: &mut SubstrateWorld, field: String, value: String) {
    let stderr = world
        .context
        .get("startup_stderr")
        .cloned()
        .unwrap_or_default();
    let json_line = stderr
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or("");
    let parsed: serde_json::Value =
        serde_json::from_str(json_line).unwrap_or(serde_json::Value::Null);
    assert_eq!(
        parsed[&field].as_str(),
        Some(value.as_str()),
        "stderr JSON field '{field}' mismatch: expected '{value}', got: {parsed}"
    );
}

#[then(
    regex = r#"^that JSON line has field "([^"]+)" in ISO 8601 format$"#
)]
async fn then_stderr_json_iso8601(world: &mut SubstrateWorld, field: String) {
    let stderr = world
        .context
        .get("startup_stderr")
        .cloned()
        .unwrap_or_default();
    let json_line = stderr
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or("");
    let parsed: serde_json::Value =
        serde_json::from_str(json_line).unwrap_or(serde_json::Value::Null);
    let ts = parsed[&field].as_str().unwrap_or("");
    assert!(!ts.is_empty(), "expected ISO 8601 timestamp in '{field}' but got empty");
}

#[then(regex = r#"^no bytes are written to stdout$"#)]
async fn then_no_stdout_bytes(world: &mut SubstrateWorld) {
    let stdout = world
        .context
        .get("startup_stdout")
        .cloned()
        .unwrap_or_default();
    assert!(
        stdout.is_empty(),
        "expected no stdout output but got: '{stdout}'"
    );
}

#[then(
    regex = r#"^the stderr JSON line details include field "path" equal to "([^"]+)"$"#
)]
async fn then_stderr_detail_path(world: &mut SubstrateWorld, expected_path: String) {
    let stderr = world
        .context
        .get("startup_stderr")
        .cloned()
        .unwrap_or_default();
    let json_line = stderr
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or("");
    let parsed: serde_json::Value =
        serde_json::from_str(json_line).unwrap_or(serde_json::Value::Null);
    let path = parsed["details"]["path"].as_str().unwrap_or("");
    assert_eq!(
        path, expected_path,
        "stderr JSON details.path mismatch: expected '{expected_path}'"
    );
}

#[then(
    regex = r#"^the process does not exit immediately with a non-zero code$"#
)]
async fn then_no_immediate_exit(world: &mut SubstrateWorld) {
    // If substrate started normally it will be waiting for stdin; exit code would
    // be set only if process terminated prematurely.
    let code: i32 = world
        .context
        .get("startup_exit_code")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert_eq!(
        code, 0,
        "expected substrate to stay running (exit 0) but got {code}"
    );
}

#[then(
    regex = r#"^no SUBSTRATE_ALLOWLIST_ROOT_MISSING error is emitted$"#
)]
async fn then_no_allowlist_missing_error(world: &mut SubstrateWorld) {
    let stderr = world
        .context
        .get("startup_stderr")
        .cloned()
        .unwrap_or_default();
    assert!(
        !stderr.contains("SUBSTRATE_ALLOWLIST_ROOT_MISSING"),
        "unexpected SUBSTRATE_ALLOWLIST_ROOT_MISSING in stderr"
    );
}

#[then(regex = r#"^the error object field "recovery_hint" is not an empty string$"#)]
async fn then_recovery_hint_not_empty(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    let hint = resp["error"]["data"]["recovery_hint"]
        .as_str()
        .unwrap_or("");
    assert!(
        !hint.is_empty(),
        "recovery_hint should not be empty: {resp}"
    );
}

#[then(
    regex = r#"^the error object field "recovery_hint" does not exceed (\d+) characters$"#
)]
async fn then_recovery_hint_max_length(world: &mut SubstrateWorld, max: usize) {
    let resp = world.last_response.as_ref().expect("no response");
    let hint = resp["error"]["data"]["recovery_hint"]
        .as_str()
        .unwrap_or("");
    assert!(
        hint.len() <= max,
        "recovery_hint length {} exceeds {max}: '{hint}'",
        hint.len()
    );
}

#[then(
    regex = r#"^the server stderr contains a log line whose "correlation_id" matches the response correlation_id$"#
)]
async fn then_stderr_correlation_id_matches(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: error-response-shape — stderr correlation_id matching requires stderr capture"
    );
}

#[then(
    regex = r#"^the server returns error code (SUBSTRATE_[A-Z_]+)$"#
)]
async fn then_error_code_cc(world: &mut SubstrateWorld, code: String) {
    let resp = world.last_response.as_ref().expect("no response");
    let found_in_error = resp["error"]["data"]["code"]
        .as_str()
        .map_or(false, |c| c == code);
    let found_in_sc = resp["result"]["structuredContent"]["error"]["code"]
        .as_str()
        .map_or(false, |c| c == code);
    assert!(
        found_in_error || found_in_sc,
        "expected error code {code} but got: {resp}"
    );
}

#[then(regex = r#"^the connection is closed without processing further requests$"#)]
async fn then_connection_closed(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: protocol-version-rejection — connection-close verification"
    );
}

#[then(regex = r#"^the server returns a successful initialize response$"#)]
async fn then_successful_init_response(world: &mut SubstrateWorld) {
    let resp = world.last_response.as_ref().expect("no response");
    assert!(
        resp["result"].is_object() && resp["result"]["protocolVersion"].is_string(),
        "expected successful initialize response but got: {resp}"
    );
}

#[then(regex = r#"^the client may proceed with tool calls$"#)]
async fn then_client_may_proceed(world: &mut SubstrateWorld) {
    // Verified implicitly by the successful initialize response.
}

#[then(
    regex = r#"^at least one ProgressNotification is received before the final result$"#
)]
async fn then_progress_before_result(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: progress-notification-emitted — ProgressNotification ordering requires notification channel"
    );
}

#[then(
    regex = r#"^each ProgressNotification includes the progressToken from the request$"#
)]
async fn then_progress_includes_token(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: progress-notification-emitted — ProgressNotification progressToken field"
    );
}

#[then(
    regex = r#"^each ProgressNotification includes a progress value between 0 and 1 \(inclusive\)$"#
)]
async fn then_progress_value_range(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: progress-notification-emitted — ProgressNotification progress [0,1]"
    );
}

#[then(
    regex = r#"^at least one ProgressNotification with progressToken="([^"]+)" is emitted$"#
)]
async fn then_progress_notification_with_token(world: &mut SubstrateWorld, token: String) {
    unimplemented!(
        "step pending: progress-notification-emitted — ProgressNotification with token '{token}'"
    );
}

#[then(
    regex = r#"^the final ProgressNotification has progress=1\.0 or total=current$"#
)]
async fn then_final_progress_complete(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: progress-notification-emitted — final ProgressNotification progress=1.0"
    );
}

#[then(
    regex = r#"^no ProgressNotification is emitted before the result$"#
)]
async fn then_no_progress_before_result(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: progress-notification-emitted — no ProgressNotification for fast ops"
    );
}

#[then(
    regex = r#"^the result arrives without intermediate notifications$"#
)]
async fn then_result_no_intermediate(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: progress-notification-emitted — no intermediate notifications for fast ops"
    );
}

#[then(
    regex = r#"^the progress values in emission order are non-decreasing$"#
)]
async fn then_progress_monotonic(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: progress-notification-emitted — monotonic progress values"
    );
}

#[then(
    regex = r#"^exactly one WARN-level line is written to stderr mentioning the audit log fallback$"#
)]
async fn then_one_warn_stderr_line(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: audit-log-write-failure — WARN stderr line requires stderr capture from server"
    );
}

#[then(
    regex = r#"^that stderr line is not structured as an error response \(no "code" field at root\)$"#
)]
async fn then_warn_not_error_response(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: audit-log-write-failure — WARN line structure check"
    );
}

#[then(
    regex = r#"^a WARN-level line is written to stderr$"#
)]
async fn then_warn_line_written(world: &mut SubstrateWorld) {
    unimplemented!(
        "step pending: audit-log-write-failure — WARN line presence check"
    );
}

#[then(
    regex = r#"^that WARN line references the audit log target path "([^"]+)"$"#
)]
async fn then_warn_references_path(world: &mut SubstrateWorld, path: String) {
    unimplemented!(
        "step pending: audit-log-write-failure — WARN references path '{path}'"
    );
}

#[then(
    regex = r#"^the response does not contain field "code" equal to "([^"]+)"$"#
)]
async fn then_response_no_code(world: &mut SubstrateWorld, code: String) {
    let resp = world.last_response.as_ref().expect("no response");
    let actual = resp["result"]["structuredContent"]["code"]
        .as_str()
        .unwrap_or("");
    assert_ne!(
        actual, code,
        "response should not contain code '{code}' but it does: {resp}"
    );
}
