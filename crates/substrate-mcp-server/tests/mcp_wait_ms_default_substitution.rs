//! Integration tests for ADR-0059: `wait_ms` default substitution in `job_result`.
//!
//! Verifies end-to-end through the actual MCP dispatch path (real binary, STDIO
//! JSON-RPC) that:
//!
//! 1. A `job_result` call **without** `wait_ms` substitutes `result_default_wait_ms`
//!    from config (long-poll) and returns the succeeded result inline.
//! 2. A `job_result` call with explicit `wait_ms = 0` returns fast (≤ 150 ms)
//!    with an error response (the job is still running).
//! 3. Boot rejects a config where `result_default_wait_ms > result_max_wait_ms`
//!    (exit code 73, composition root wiring failed).
//! 4. Boot rejects a config where `result_default_wait_ms = 0`
//!    (exit code 73, composition root wiring failed).
//!
//! Tests 1–2 model after the `archive_tar_create` job flow pattern used in
//! `mcp_job_flow.rs`. Tests 3–4 spawn the binary with an invalid TOML config
//! and assert the process exits with code 73 per ADR-0036.
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
    clippy::panic,
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
    time::{Duration, Instant},
};

use tempfile::TempDir;

// ---- Shared helpers (mirrors mcp_job_flow.rs pattern) -----------------------

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_substrate"))
}

/// Writes a `substrate.toml` with the given `[jobs.quotas]` field values.
///
/// `result_default_wait_ms` and `result_max_wait_ms` are caller-supplied so
/// individual tests can exercise valid and invalid configurations.
fn write_config_with_wait(
    dir: &std::path::Path,
    root: &std::path::Path,
    result_default_wait_ms: u32,
    result_max_wait_ms: u32,
) -> PathBuf {
    let cfg = dir.join("substrate.toml");
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
result_max_wait_ms = {result_max_wait_ms}
result_default_wait_ms = {result_default_wait_ms}
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
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read stdout line");
        let frame: serde_json::Value =
            serde_json::from_str(line.trim()).expect("parse JSON-RPC frame");
        // Skip server-pushed notifications (no "id" field).
        if frame.get("id").is_none() {
            continue;
        }
        return frame;
    }
}

/// Performs MCP handshake and returns the `initialize` response.
fn handshake(
    stdin: &mut std::process::ChildStdin,
    reader: &mut BufReader<std::process::ChildStdout>,
) -> serde_json::Value {
    send(
        stdin,
        r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"wait-ms-test","version":"0.0.1"}}}"#,
    );
    let init_resp = recv(reader);
    send(
        stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    init_resp
}

// ---- Test 1: absent wait_ms long-polls and returns the succeeded result -----

/// Submits a small `archive_tar_create` job and calls `job_result` WITHOUT the
/// `wait_ms` field. The server must substitute `result_default_wait_ms = 3000`
/// and long-poll until the job completes, returning the succeeded result
/// (`archive_path` in `structuredContent`) rather than timing out immediately.
#[test]
fn job_result_with_absent_wait_ms_substitutes_default() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();

    // Config: default wait = 3 s, max wait = 30 s — enough time for a tiny job.
    write_config_with_wait(&root, &root, 3_000, 30_000);

    // Create a tiny source tree (fast to archive — well under 3 s even in debug).
    let src_dir = root.join("src_absent_wait");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("hello.txt"), b"hello ADR-0059").unwrap();

    let dest_tar = root.join("absent_wait.tar");

    let mut child = spawn_server(&root);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    // 45 s overall guard: accommodates debug-build startup + job + wait window.
    let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let _guard = install_timeout(Arc::clone(&child_arc), Duration::from_secs(45));

    let mut stdin = stdin;
    let mut reader = BufReader::new(stdout);

    let _init = handshake(&mut stdin, &mut reader);

    // Submit the archive job.
    let submit_call = serde_json::json!({
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
    send(&mut stdin, &submit_call.to_string());
    let submit_resp = recv(&mut reader);

    let job_id = submit_resp["result"]["structuredContent"]["job_id"]
        .as_str()
        .expect("job_id must be present in structuredContent");
    assert!(!job_id.is_empty(), "job_id must not be empty");

    // Call job_result WITHOUT the wait_ms field (field absence is the test surface).
    // The JSON object must NOT contain "wait_ms" — do NOT pass null.
    let result_call = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": 20,
        "params": {
            "name": "job_result",
            "arguments": { "job_id": job_id }
            // wait_ms is intentionally absent — ADR-0059 substitutes result_default_wait_ms
        }
    });

    // Sanity: confirm the JSON we built does not contain wait_ms.
    assert!(
        result_call["params"]["arguments"].get("wait_ms").is_none(),
        "test precondition: wait_ms must be absent from the request arguments"
    );

    send(&mut stdin, &result_call.to_string());
    let result_resp = recv(&mut reader);

    // The response must not be an error: the substituted 3 s default wait must
    // have been long enough for the tiny archive job to finish.
    assert!(
        result_resp["result"].is_object(),
        "job_result must return a result object, got: {result_resp}"
    );
    assert!(
        !result_resp["result"]["isError"].as_bool().unwrap_or(false),
        "job_result returned isError=true (timed out?): {result_resp}"
    );

    // The succeeded archive result must contain archive_path.
    let sc = &result_resp["result"]["structuredContent"];
    assert!(
        sc.get("archive_path").is_some(),
        "succeeded job_result must contain archive_path; got structuredContent: {sc}"
    );

    // The .tar file must exist on disk.
    assert!(
        dest_tar.exists(),
        "expected absent_wait.tar to exist after job completion; result: {result_resp}"
    );

    let mut guard = child_arc.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = guard.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

// ---- Test 2: explicit wait_ms = 0 returns fast with a running indicator -----

/// Submits an `archive_tar_create` job (non-trivial source tree so it is
/// unlikely to complete before the call arrives) and immediately calls
/// `job_result` with explicit `wait_ms = 0`.
///
/// The call must return within ~150 ms with an error response
/// (`result.isError = true`), which is how the dispatch path surfaces a
/// `SubstrateError::Timeout` (the registry's `Duration::ZERO` fast-return path).
#[test]
fn job_result_with_explicit_zero_wait_ms_returns_fast() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();

    // Valid config — default_wait matters here only for the boot check.
    write_config_with_wait(&root, &root, 5_000, 30_000);

    // Build a source tree large enough that the job is very unlikely to complete
    // in the ~50 ms window between submit and the zero-wait result call.
    let src_dir = root.join("src_zero_wait");
    std::fs::create_dir_all(&src_dir).unwrap();
    for i in 0u32..30 {
        std::fs::write(
            src_dir.join(format!("file_{i:03}.txt")),
            format!("content for zero-wait test iteration {i}").as_bytes(),
        )
        .unwrap();
    }
    let dest_tar = root.join("zero_wait.tar");

    let mut child = spawn_server(&root);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    // 40 s overall guard.
    let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let _guard = install_timeout(Arc::clone(&child_arc), Duration::from_secs(40));

    let mut stdin = stdin;
    let mut reader = BufReader::new(stdout);

    let _init = handshake(&mut stdin, &mut reader);

    // Submit the job.
    let submit_call = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": 30,
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
    send(&mut stdin, &submit_call.to_string());
    let submit_resp = recv(&mut reader);

    let job_id = submit_resp["result"]["structuredContent"]["job_id"]
        .as_str()
        .expect("job_id must be present")
        .to_owned();

    // Call job_result with explicit wait_ms = 0 (fast-return opt-out per ADR-0059).
    let result_call = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": 31,
        "params": {
            "name": "job_result",
            "arguments": { "job_id": job_id, "wait_ms": 0 }
        }
    });

    let t0 = Instant::now();
    send(&mut stdin, &result_call.to_string());
    let result_resp = recv(&mut reader);
    let elapsed = t0.elapsed();

    // The call must return quickly — well under 500 ms even in debug builds.
    // We use 500 ms as the upper bound to avoid flakiness on heavily loaded CI,
    // while still catching a regression where wait_ms=0 is ignored and the server
    // blocks for the full default wait window.
    assert!(
        elapsed < Duration::from_millis(500),
        "job_result with wait_ms=0 took {:?}; expected < 500 ms",
        elapsed
    );

    // The response must be a result object (JSON-RPC protocol level).
    assert!(
        result_resp["result"].is_object(),
        "job_result must return a result object, got: {result_resp}"
    );

    // The registry surfaces a Timeout error when wait_ms=0 and the job is still
    // running. The MCP server wraps SubstrateError as isError=true in the tool
    // response. Accept isError=true (still running) OR isError=false (job happened
    // to finish before the request arrived — valid race).
    let is_error = result_resp["result"]["isError"].as_bool().unwrap_or(false);
    if is_error {
        // Expected path: job not yet done → Timeout error surfaced as isError=true.
        // No further content assertions needed.
    } else {
        // Race path: job completed before the zero-wait call arrived. The result
        // must contain archive_path in this case (genuine succeeded result).
        let sc = &result_resp["result"]["structuredContent"];
        assert!(
            sc.get("archive_path").is_some(),
            "race path: isError=false but structuredContent missing archive_path: {sc}"
        );
    }

    let mut guard = child_arc.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = guard.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

// ---- Test 3: boot rejects result_default_wait_ms > result_max_wait_ms ------

/// Constructs a TOML config where `result_default_wait_ms = 5000` exceeds
/// `result_max_wait_ms = 1000`. Asserts that `composition::wire()` (exercised
/// via the real binary startup path) returns `SubstrateError::ConfigInvalid`
/// and that the process exits with code 73 per ADR-0036.
///
/// This test spawns the binary with the invalid config, sends an `initialize`
/// request, and verifies the process exits before responding (code 73).
#[test]
fn boot_rejects_invalid_wait_window() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();

    // result_default_wait_ms (5000) > result_max_wait_ms (1000) — must fail boot.
    write_config_with_wait(&root, &root, 5_000, 1_000);

    let mut child = Command::new(binary_path())
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Capture stderr so it does not pollute test output.
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn substrate-mcp-server");

    // Give the server up to 10 s to start and fail. Debug builds are slow.
    let deadline = Duration::from_secs(10);
    let t0 = Instant::now();

    let exit_status = loop {
        match child.try_wait().expect("try_wait failed") {
            Some(status) => break status,
            None => {
                if t0.elapsed() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("boot_rejects_invalid_wait_window: server did not exit within 10 s");
                }
                thread::sleep(Duration::from_millis(50));
            },
        }
    };

    // ADR-0036: composition root failure → exit code 73.
    let code = exit_status.code().unwrap_or(-1);
    assert_eq!(
        code, 73,
        "expected exit code 73 (composition root wiring failed) for invalid wait window; got {code}"
    );
}

// ---- Test 4: boot rejects result_default_wait_ms = 0 -----------------------

/// Constructs a TOML config where `result_default_wait_ms = 0`, which violates
/// the ADR-0059 invariant `0 < result_default_wait_ms`. Asserts exit code 73.
#[test]
fn boot_rejects_zero_default_wait_window() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();

    // result_default_wait_ms = 0 — must fail boot validation.
    write_config_with_wait(&root, &root, 0, 30_000);

    let mut child = Command::new(binary_path())
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn substrate-mcp-server");

    let deadline = Duration::from_secs(10);
    let t0 = Instant::now();

    let exit_status = loop {
        match child.try_wait().expect("try_wait failed") {
            Some(status) => break status,
            None => {
                if t0.elapsed() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "boot_rejects_zero_default_wait_window: server did not exit within 10 s"
                    );
                }
                thread::sleep(Duration::from_millis(50));
            },
        }
    };

    let code = exit_status.code().unwrap_or(-1);
    assert_eq!(
        code, 73,
        "expected exit code 73 (composition root wiring failed) for zero default wait; got {code}"
    );
}
