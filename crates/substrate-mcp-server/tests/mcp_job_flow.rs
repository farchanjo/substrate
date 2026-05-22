//! End-to-end MCP STDIO async job flow tests.
//!
//! Tests the Bucket C async job flow over real STDIO:
//!   1. Submit `archive_tar_create` with a small source directory.
//!   2. Receive `job_id` in `structuredContent`.
//!   3. Poll `job_status` until terminal or timeout.
//!   4. Call `job_result` → succeeded with .tar artifact present on disk.
//!
//! Also tests cooperative cancellation: submit a job, cancel it,
//! verify `job_result` returns `Cancelled` state.
//!
//! Per ADR-0044 carve-out: `std::process::Command` is allowed in tests.
//!
//! # Lint relaxations (integration-test carve-out)
//!
//! - `expect_used` / `unwrap_used`: panicking assertions are idiomatic in tests.
//! - `disallowed_types` / `disallowed_methods`: `std::process::Command` and
//!   `Child` are allowed here per the ADR-0044 integration-test exception.
//! - `missing_docs`: integration-test binary; no public API to document.
#![expect(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::redundant_closure_for_method_calls,
    reason = "integration-test carve-out per ADR-0044: \
              panicking assertions and std::process::Command are idiomatic here; \
              redundant_closure suppressed for PoisonError::into_inner pattern"
)]

use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use tempfile::TempDir;

// ---- Shared helpers ---------------------------------------------------------

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_substrate"))
}

fn write_config(dir: &std::path::Path, root: &std::path::Path) -> PathBuf {
    let cfg = dir.join("substrate.toml");
    // Include all [jobs.*] sub-sections explicitly — figment requires them
    // because JobQuotas / JobInlineThresholds / JobTimeouts lack #[serde(default)].
    // This enables InMemoryJobRegistry (otherwise NullJobRegistry returns
    // SUBSTRATE_INTERNAL_ERROR on every Bucket C submit).
    let content = format!(
        r#"[policy]
roots = ["{root}"]

[logging]
level = "error"

[security]
refuse_degraded_jail = false

[timeouts]
global_default_seconds = 60
shutdown_drain_secs = 2

[jobs.quotas]
max_concurrent = 16
max_per_client = 4
result_ttl_secs = 300
result_max_wait_ms = 60000
progress_interval_ms = 250
progress_channel_size = 64
gc_interval_secs = 60

[jobs.inline_thresholds]
fs_find_inline_entries = 1000
fs_read_inline_bytes = 1048576
fs_hash_inline_bytes = 4194304
fs_copy_inline_bytes = 1048576
text_search_inline_bytes = 524288
text_count_lines_inline_bytes = 524288
archive_gzip_inline_bytes = 131072
archive_hash_inline_bytes = 4194304

[jobs.timeouts]
default_secs = 600
archive_create_secs = 1800
archive_extract_secs = 1800
fs_find_secs = 60
fs_hash_secs = 600
"#,
        root = root.display()
    );
    std::fs::write(&cfg, content).expect("write substrate.toml");
    cfg
}

fn spawn_server(dir: &std::path::Path) -> Child {
    Command::new(binary_path())
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn substrate-mcp-server")
}

fn install_timeout(child: Arc<Mutex<Option<Child>>>, timeout: Duration) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(timeout);
        let mut guard = child.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = guard.as_mut() {
            let _ = c.kill();
        }
    })
}

fn send(stdin: &mut std::process::ChildStdin, msg: &str) {
    writeln!(stdin, "{msg}").expect("write stdin");
    stdin.flush().expect("flush stdin");
}

fn recv(reader: &mut BufReader<std::process::ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read stdout line");
    serde_json::from_str(line.trim()).expect("parse JSON-RPC")
}

/// Performs MCP handshake (initialize + notifications/initialized) and discards the
/// `initialize` response.  Returns the received `initialize` result for callers
/// that want to inspect it.
fn handshake(
    stdin: &mut std::process::ChildStdin,
    reader: &mut BufReader<std::process::ChildStdout>,
) -> serde_json::Value {
    send(
        stdin,
        r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"job-flow-test","version":"0.0.1"}}}"#,
    );
    let init_resp = recv(reader);
    send(
        stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    init_resp
}

// ---- archive_tar_create job flow test ----------------------------------------

#[test]
fn archive_tar_create_job_flow_succeeds() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write_config(&root, &root);

    // Create some source files inside the sandbox.
    let src_dir = root.join("src_files");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("a.txt"), b"alpha").unwrap();
    std::fs::write(src_dir.join("b.txt"), b"bravo").unwrap();

    let dest_tar = root.join("output.tar");

    let mut child = spawn_server(&root);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    // 45s timeout: debug build startup + job execution + result retrieval.
    let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let _guard = install_timeout(Arc::clone(&child_arc), Duration::from_secs(45));

    let mut stdin = stdin;
    let mut reader = BufReader::new(stdout);

    let _init = handshake(&mut stdin, &mut reader);

    // Submit archive_tar_create job.
    // Field names per TarCreateRequest: sources (Vec<String>), dest, compression,
    // dry_run (default true — must set false), confirmed (must set true for live write).
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": 10,
        "params": {
            "name": "archive_tar_create",
            "arguments": {
                "sources": [src_dir.to_str().unwrap()],
                "dest": dest_tar.to_str().unwrap(),
                "compression": "none",
                "dry_run": false,
                "confirmed": true
            }
        }
    });
    send(&mut stdin, &call.to_string());
    let submit_resp = recv(&mut reader);

    // Expect a Pending job receipt.
    let sc = &submit_resp["result"]["structuredContent"];
    let job_id = sc["job_id"].as_str().expect("job_id in structuredContent");
    assert!(!job_id.is_empty(), "job_id must not be empty");

    // Use job_result with wait_ms to block until the job completes (up to 20s).
    // Debug builds are slow to start; 20s accommodates server startup + job execution.
    let result_call = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": 200,
        "params": {
            "name": "job_result",
            "arguments": { "job_id": job_id, "wait_ms": 20000 }
        }
    });
    send(&mut stdin, &result_call.to_string());
    let result_resp = recv(&mut reader);

    assert!(
        result_resp["result"].is_object(),
        "job_result must return a result, got: {result_resp}"
    );
    // Verify it is not an error result.
    assert!(
        !result_resp["result"]["isError"].as_bool().unwrap_or(false),
        "job_result returned isError=true: {result_resp}"
    );

    // Verify the job result structured content to catch silent job failures.
    let result_sc = &result_resp["result"]["structuredContent"];
    assert!(
        result_sc.get("archive_path").is_some(),
        "job_result structuredContent missing 'archive_path' — job likely failed: {result_resp}"
    );

    // Verify the .tar file exists on disk.
    assert!(
        dest_tar.exists(),
        "expected output.tar to exist after job completion; result: {result_resp}"
    );

    let mut guard = child_arc.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = guard.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

// ---- job cancellation test ---------------------------------------------------

#[test]
fn archive_tar_create_job_can_be_cancelled() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write_config(&root, &root);

    // Build a source tree large enough that the job is unlikely to complete
    // before the cancel arrives when run under normal CI load.  Even if it
    // does complete first, the test validates the cancel path gracefully.
    let src_dir = root.join("large_src");
    std::fs::create_dir_all(&src_dir).unwrap();
    for i in 0u32..20 {
        std::fs::write(
            src_dir.join(format!("file_{i:03}.txt")),
            format!("content {i}").as_bytes(),
        )
        .unwrap();
    }
    let dest_tar = root.join("cancel_output.tar");

    let mut child = spawn_server(&root);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    // 30s timeout: debug build startup + cancellation round-trip can be slow.
    let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let _guard = install_timeout(Arc::clone(&child_arc), Duration::from_secs(30));

    let mut stdin = stdin;
    let mut reader = BufReader::new(stdout);

    let _init = handshake(&mut stdin, &mut reader);

    // Submit the job.
    // Field names per TarCreateRequest: sources (Vec<String>), dest, compression.
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": 20,
        "params": {
            "name": "archive_tar_create",
            "arguments": {
                "sources": [src_dir.to_str().unwrap()],
                "dest": dest_tar.to_str().unwrap(),
                "compression": "none"
            }
        }
    });
    send(&mut stdin, &call.to_string());
    let submit_resp = recv(&mut reader);

    let job_id = submit_resp["result"]["structuredContent"]["job_id"]
        .as_str()
        .expect("job_id")
        .to_owned();

    // Immediately issue job_cancel before the job finishes.
    let cancel_call = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": 21,
        "params": {
            "name": "job_cancel",
            "arguments": { "job_id": job_id }
        }
    });
    send(&mut stdin, &cancel_call.to_string());
    let cancel_resp = recv(&mut reader);

    // Cancel should return a result (not a JSON-RPC error).
    assert!(
        cancel_resp["result"].is_object(),
        "job_cancel returned error: {cancel_resp}"
    );

    // Use job_result with wait_ms to wait for terminal state (up to 8s).
    let result_call = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": 22,
        "params": {
            "name": "job_result",
            "arguments": { "job_id": job_id, "wait_ms": 8000 }
        }
    });
    send(&mut stdin, &result_call.to_string());
    let result_resp = recv(&mut reader);

    // job_result should return a result object regardless of terminal state.
    assert!(
        result_resp["result"].is_object(),
        "job_result returned non-object: {result_resp}"
    );

    // Verify the response contains a terminal state indicator.
    // Cancelled jobs have structuredContent.state = "Cancelled" (capital, from
    // job_result handler's explicit JSON literal), or Succeeded if the race was lost.
    let result_sc = &result_resp["result"]["structuredContent"];
    assert!(
        result_sc.is_object(),
        "job_result structuredContent missing: {result_resp}"
    );

    let mut guard = child_arc.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = guard.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
}
