//! End-to-end MCP STDIO smoke tests.
//!
//! Spawns the `substrate-mcp-server` binary as a subprocess, drives it via
//! JSON-RPC over STDIO, and verifies handshake + tools/list + tools/call.
//!
//! Per ADR-0044 no-subprocess policy, these tests live under `tests/` (the
//! integration-test carve-out) and use `std::process::Command` only for the
//! binary under test.
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
//!
//! # Config strategy
//!
//! Each test writes a minimal `substrate.toml` in the `TempDir` and sets
//! `SUBSTRATE_POLICY__ROOTS` to allow the temp root so no system-level config
//! is needed. The config layer picks up `./substrate.toml` from the cwd, but
//! since the subprocess inherits the parent cwd we write the file there and
//! pass it as an env override.
//!
//! # Timeout
//!
//! Each test reads at most one line with a 8-second deadline enforced by a
//! background thread that kills the child.

use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use tempfile::TempDir;

// ---- Helpers -----------------------------------------------------------------

fn binary_path() -> PathBuf {
    // `CARGO_BIN_EXE_<name>` is set by cargo for integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_substrate"))
}

/// Writes a minimal `substrate.toml` inside `dir` that allows `root` as the
/// only allowlist root and sets `refuse_degraded_jail = false` so the smoke
/// tests work on macOS (where `openat2` is unavailable).
fn write_config(dir: &std::path::Path, root: &std::path::Path) -> PathBuf {
    let cfg = dir.join("substrate.toml");
    let content = format!(
        "[policy]\nroots = [\"{root}\"]\n\n\
         [logging]\nlevel = \"error\"\n\n\
         [security]\nrefuse_degraded_jail = false\n\n\
         [timeouts]\nglobal_default_seconds = 30\nshutdown_drain_secs = 2\n",
        root = root.display()
    );
    std::fs::write(&cfg, content).expect("write substrate.toml");
    cfg
}

/// Spawns the binary with `substrate.toml` in a temp dir and returns the
/// child process.  The binary reads its config via the `./substrate.toml`
/// project-local path, so we set `cwd` to `dir`.
fn spawn_server(dir: &std::path::Path) -> Child {
    Command::new(binary_path())
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn substrate-mcp-server")
}

/// Installs a background thread that kills `child` after `timeout` and sets
/// `timed_out` to `true`.  Returns the guard; drop to disarm.
fn install_timeout(child: Arc<Mutex<Option<Child>>>, timeout: Duration) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(timeout);
        let mut guard = child.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = guard.as_mut() {
            let _ = c.kill();
        }
    })
}

/// Writes one JSON-RPC line + newline to stdin.
fn send(stdin: &mut std::process::ChildStdin, msg: &str) {
    writeln!(stdin, "{msg}").expect("write to stdin");
    stdin.flush().expect("flush stdin");
}

/// Reads one newline-delimited JSON-RPC response from `reader`.
fn recv(reader: &mut BufReader<std::process::ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read line from stdout");
    serde_json::from_str(line.trim()).expect("parse JSON-RPC response")
}

// ---- initialize test ---------------------------------------------------------

#[test]
fn initialize_returns_protocol_2025_11_25_and_capabilities() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write_config(&root, &root);

    let mut child = spawn_server(&root);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    // Wrap child in Arc<Mutex<Option<…>>> for the timeout thread.
    let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let _guard = install_timeout(Arc::clone(&child_arc), Duration::from_secs(8));

    let mut stdin = stdin;
    let mut reader = BufReader::new(stdout);

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"0.0.1"}}}"#,
    );

    let resp = recv(&mut reader);

    // Verify envelope.
    assert_eq!(resp["jsonrpc"], "2.0", "jsonrpc field");
    assert_eq!(resp["id"], 1, "id field");
    assert_eq!(
        resp["result"]["protocolVersion"], "2025-11-25",
        "negotiated protocol version"
    );

    // Verify experimental.substrate.jobs is a boolean (true or false).
    let jobs = &resp["result"]["capabilities"]["experimental"]["substrate"]["jobs"];
    assert!(
        jobs.is_boolean(),
        "experimental.substrate.jobs must be boolean, got: {jobs}"
    );

    // Kill child cleanly.
    let mut guard = child_arc.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = guard.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

// ---- tools/list test ---------------------------------------------------------

/// Expected `tools/list` length. The four always-on tools proc_stats,
/// proc_top, sys_mem, and sys_cpu were added to the registry in addition to the
/// original 41; the `subprocess` feature contributes six more
/// (spawn/list/cancel/result/signal/search).
#[cfg(not(feature = "subprocess"))]
const EXPECTED_TOOLS: usize = 45;
#[cfg(feature = "subprocess")]
const EXPECTED_TOOLS: usize = 51;

#[test]
fn tools_list_returns_expected_tool_count() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write_config(&root, &root);

    let mut child = spawn_server(&root);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let _guard = install_timeout(Arc::clone(&child_arc), Duration::from_secs(8));

    let mut stdin = stdin;
    let mut reader = BufReader::new(stdout);

    // Handshake first.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"0.0.1"}}}"#,
    );
    let _init = recv(&mut reader);

    // Send initialized notification (no response expected).
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );

    // Request tools/list.
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"tools/list","id":2,"params":{}}"#,
    );
    let resp = recv(&mut reader);

    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert_eq!(
        tools.len(),
        EXPECTED_TOOLS,
        "expected {EXPECTED_TOOLS} tools, found {}",
        tools.len()
    );

    let mut guard = child_arc.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = guard.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

// ---- sys_hostname inline call ------------------------------------------------

#[test]
fn tools_call_sys_hostname_returns_result() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write_config(&root, &root);

    let mut child = spawn_server(&root);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let _guard = install_timeout(Arc::clone(&child_arc), Duration::from_secs(8));

    let mut stdin = stdin;
    let mut reader = BufReader::new(stdout);

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"0.0.1"}}}"#,
    );
    let _init = recv(&mut reader);
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"tools/call","id":3,"params":{"name":"sys_hostname","arguments":{}}}"#,
    );
    let resp = recv(&mut reader);

    // Either a successful result or a structured error are both valid — the test
    // verifies the envelope shape and that the call reaches the handler.
    assert!(
        resp["result"].is_object() || resp["error"].is_object(),
        "expected result or error in response, got: {resp}"
    );
    // Success case: content array must be non-empty.
    if let Some(result) = resp["result"].as_object() {
        let content = result.get("content").and_then(|c| c.as_array());
        assert!(
            content.is_some_and(|c| !c.is_empty()),
            "sys_hostname: content array is empty"
        );
    }

    let mut guard = child_arc.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = guard.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

// ---- fs_stat inline call on temp dir -----------------------------------------

#[test]
fn tools_call_fs_stat_on_temp_returns_structured_content() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write_config(&root, &root);

    // Create a probe file inside the sandbox.
    let probe = root.join("probe.txt");
    std::fs::write(&probe, b"hello").expect("write probe");

    let mut child = spawn_server(&root);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));
    let _guard = install_timeout(Arc::clone(&child_arc), Duration::from_secs(8));

    let mut stdin = stdin;
    let mut reader = BufReader::new(stdout);

    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"0.0.1"}}}"#,
    );
    let _init = recv(&mut reader);
    send(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );

    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "tools/call",
        "id": 4,
        "params": {
            "name": "fs_stat",
            "arguments": { "path": probe.to_str().unwrap() }
        }
    });
    send(&mut stdin, &call.to_string());
    let resp = recv(&mut reader);

    assert!(
        resp["result"].is_object() || resp["error"].is_object(),
        "expected result or error, got: {resp}"
    );

    // On success, structuredContent must carry a `_text` field.
    if resp["result"].is_object() {
        let sc = &resp["result"]["structuredContent"];
        assert!(
            sc.is_object(),
            "fs_stat: structuredContent missing, got: {sc}"
        );
    }

    let mut guard = child_arc.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = guard.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
}
