// Cucumber end-to-end test runner for the substrate MCP server.
//
// PREREQUISITE: the `substrate` binary must be built before running these
// tests.  The test driver spawns the real binary over STDIO and validates
// JSON-RPC responses — no mocking.
//
//   cargo build --bin substrate --profile dev
//   cargo nextest run --test cucumber
//
// Feature files live under docs/arch/specs/features/ (relative to workspace
// root).  The path below is resolved at runtime from CARGO_MANIFEST_DIR.
//
// Lint relaxations (integration-test carve-out per ADR-0044):
//   - expect_used / unwrap_used: panicking assertions are idiomatic in tests.
//   - disallowed_types / disallowed_methods: std::process::Command and Child
//     are required here to spawn the binary under test.
//   - missing_docs / unreachable_pub: internal test harness; no public API.
#![expect(
    clippy::expect_used,
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::redundant_closure_for_method_calls,
    clippy::needless_pass_by_value,
    unreachable_pub,
    missing_docs,
    reason = "integration-test carve-out per ADR-0044: \
              panicking assertions and std::process::Command are idiomatic here; \
              unreachable_pub suppressed for step module re-exports; \
              missing_docs / missing_panics_doc / must_use suppressed for test harness internals"
)]

mod steps;

use std::{
    io::{BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use cucumber::World;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// World
// ---------------------------------------------------------------------------

/// Shared state threaded through every scenario.
///
/// Each scenario receives a fresh `SubstrateWorld` via `World::new()`.
/// Helper methods expose the running server subprocess and the last
/// JSON-RPC response received.
#[derive(Debug, Default, World)]
pub struct SubstrateWorld {
    /// Temporary directory used as the path-jail allowlist root.
    pub sandbox: Option<TempDir>,

    /// Handle to the running `substrate` subprocess.
    pub child: Option<Child>,

    /// Buffered reader around the subprocess stdout.
    pub stdout_reader: Option<BufReader<ChildStdout>>,

    /// Write-half of the subprocess stdin.
    pub stdin_writer: Option<ChildStdin>,

    /// Latest raw JSON-RPC response line received (success or error).
    pub last_response: Option<serde_json::Value>,

    /// Whether the MCP initialization handshake has been completed.
    pub initialized: bool,

    /// Running sequence counter for JSON-RPC `id` fields.
    pub rpc_id: u64,

    /// Allowlist root path used for this scenario (mirrors into sandbox).
    pub allowlist_root: Option<PathBuf>,

    /// Arbitrary context tags stored by Given steps for use in When/Then.
    pub context: std::collections::HashMap<String, String>,

    // -----------------------------------------------------------------------
    // Cancellation tracking (feature: cancellation-on-cancel-request)
    // -----------------------------------------------------------------------

    /// JSON-RPC id of the most recently dispatched in-flight request, held so
    /// that a subsequent `$/cancelRequest` notification can reference it.
    pub pending_request_id: Option<u64>,

    // -----------------------------------------------------------------------
    // Interleaved notification buffer (feature: progress-notification-emitted)
    // -----------------------------------------------------------------------

    /// All `notifications/progress` frames received since the last reset.
    /// Populated by `drain_until_response` before storing `last_response`.
    pub progress_notifications: Vec<serde_json::Value>,

    /// Timestamp of the last call to `drain_until_response`, used by assertions
    /// that need to scope notifications to "after the current operation started".
    pub operation_start: Option<Instant>,
}

// The `#[derive(World)]` macro from cucumber generates the WorldInventory
// implementation and the `cucumber()` builder method.  Manual `World` impl
// is not required when the derive is used.

// ---------------------------------------------------------------------------
// Subprocess helpers (shared across step modules via pub methods on World)
// ---------------------------------------------------------------------------

impl SubstrateWorld {
    /// Returns the path to the `substrate` binary under test.
    pub fn binary_path() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_substrate"))
    }

    /// Creates a temporary sandbox, writes a minimal `substrate.toml` inside
    /// it, and returns the sandbox `TempDir` plus the config file path.
    pub fn prepare_sandbox() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().expect("create TempDir");
        let root = tmp.path().canonicalize().expect("canonicalize tmpdir");
        let cfg = Self::write_config(&root, &root);
        (tmp, root, cfg)
    }

    /// Writes a minimal `substrate.toml` inside `config_dir` allowing `root`
    /// as the only allowlist root.
    pub fn write_config(config_dir: &Path, root: &Path) -> PathBuf {
        let cfg = config_dir.join("substrate.toml");
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

    /// Spawns the substrate binary with cwd = `dir` (where `substrate.toml`
    /// lives) and wires stdin/stdout pipes.
    pub fn spawn_server(dir: &Path) -> Child {
        Command::new(Self::binary_path())
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn substrate binary")
    }

    /// Spawns the substrate binary, stores handles in self, then performs the
    /// full MCP initialize + notifications/initialized handshake.
    pub fn spawn_and_initialize(&mut self) {
        let (tmp, root, _cfg) = Self::prepare_sandbox();
        let mut child = Self::spawn_server(tmp.path());
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        self.sandbox = Some(tmp);
        self.allowlist_root = Some(root);
        self.stdin_writer = Some(stdin);
        self.stdout_reader = Some(BufReader::new(stdout));
        self.child = Some(child);
        self.perform_initialize();
    }

    /// Creates a directory tree totalling > 10 MiB inside `parent` so that
    /// archive Bucket C threshold (10 MiB input) is triggered.
    ///
    /// Strategy: write 1 file of 11 MiB (11 × 1024 × 1024 bytes of zeros).
    /// This is cheaper than 1 024 small files and equally valid for the
    /// Bucket C size check.
    pub fn create_large_fixture_tree(parent: &Path) -> PathBuf {
        let data_dir = parent.join("large_data");
        std::fs::create_dir_all(&data_dir)
            .expect("create large_data directory");
        let file = data_dir.join("payload.bin");
        // 11 MiB of zero bytes, written in 64 KiB chunks to avoid stack overflow.
        let chunk = vec![0u8; 65_536];
        let target_bytes: usize = 11 * 1024 * 1024;
        let mut written = 0usize;
        let mut f = std::fs::File::create(&file)
            .expect("create payload.bin");
        while written < target_bytes {
            use std::io::Write as _;
            let to_write = (target_bytes - written).min(chunk.len());
            f.write_all(&chunk[..to_write])
                .expect("write payload chunk");
            written += to_write;
        }
        data_dir
    }

    /// Spawns the substrate binary with a user-supplied allowlist root.
    pub fn spawn_and_initialize_with_root(&mut self, root: &Path) {
        let tmp = TempDir::new().expect("TempDir");
        Self::write_config(tmp.path(), root);
        let mut child = Self::spawn_server(tmp.path());
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        self.sandbox = Some(tmp);
        self.allowlist_root = Some(root.to_path_buf());
        self.stdin_writer = Some(stdin);
        self.stdout_reader = Some(BufReader::new(stdout));
        self.child = Some(child);
        self.perform_initialize();
    }

    /// Sends the MCP initialize + notifications/initialized handshake.
    pub fn perform_initialize(&mut self) {
        let id = self.send_rpc(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "cucumber-test", "version": "0.0.1" }
            }),
        );
        // Drain any notifications before the initialize response arrives.
        let _init = self.drain_until_response(id);
        self.send_notification("notifications/initialized");
        self.initialized = true;
    }

    /// Sends a JSON-RPC request with the next available id; returns the id.
    pub fn send_rpc(&mut self, method: &str, params: serde_json::Value) -> u64 {
        self.rpc_id += 1;
        let id = self.rpc_id;
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "id": id,
            "params": params,
        });
        self.write_line(&msg.to_string());
        id
    }

    /// Sends a JSON-RPC notification (no id, no response expected).
    pub fn send_notification(&mut self, method: &str) {
        let msg = serde_json::json!({ "jsonrpc": "2.0", "method": method });
        self.write_line(&msg.to_string());
    }

    /// Sends a `tools/call` request for the given tool and arguments.
    pub fn call_tool(&mut self, tool: &str, arguments: serde_json::Value) -> u64 {
        self.send_rpc(
            "tools/call",
            serde_json::json!({ "name": tool, "arguments": arguments }),
        )
    }

    /// Writes a single newline-terminated line to the subprocess stdin.
    fn write_line(&mut self, line: &str) {
        let w = self
            .stdin_writer
            .as_mut()
            .expect("stdin_writer not initialised");
        writeln!(w, "{line}").expect("write to subprocess stdin");
        w.flush().expect("flush subprocess stdin");
    }

    /// Reads one newline-delimited JSON line from stdout and parses it.
    pub fn recv_rpc(&mut self) -> serde_json::Value {
        let r = self
            .stdout_reader
            .as_mut()
            .expect("stdout_reader not initialised");
        let mut line = String::new();
        r.read_line(&mut line).expect("read from subprocess stdout");
        serde_json::from_str(line.trim()).expect("parse JSON-RPC line")
    }

    /// Calls `call_tool`, then reads and stores the response as `last_response`.
    /// Notifications interleaved before the response are collected into
    /// `progress_notifications`.
    pub fn call_tool_and_store(&mut self, tool: &str, arguments: serde_json::Value) {
        let id = self.call_tool(tool, arguments);
        self.pending_request_id = Some(id);
        self.operation_start = Some(Instant::now());
        let resp = self.drain_until_response(id);
        self.last_response = Some(resp);
        self.pending_request_id = None;
    }

    /// Reads newline-delimited JSON frames from stdout until the frame whose
    /// `id` matches `expected_id` is found.  Frames without an `id` (i.e.,
    /// notifications) are appended to `progress_notifications`.
    ///
    /// This is the interleaved-frame reader required by feature
    /// `progress-notification-emitted`.
    pub fn drain_until_response(&mut self, expected_id: u64) -> serde_json::Value {
        self.progress_notifications.clear();
        loop {
            let frame = self.recv_rpc();
            let frame_id = frame.get("id").and_then(|v| v.as_u64());
            match frame_id {
                Some(id) if id == expected_id => return frame,
                None => {
                    // Notification (no `id` field) — buffer it.
                    self.progress_notifications.push(frame);
                }
                Some(_other_id) => {
                    // Response for a different request (should not happen in
                    // single-flight tests, but drop gracefully).
                }
            }
        }
    }

    /// Sends a `$/cancelRequest` notification for the given JSON-RPC id.
    ///
    /// Per the MCP spec the notification carries the `id` of the request to
    /// cancel in `params.id`.
    pub fn send_cancel_request(&mut self, request_id: u64) {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "$/cancelRequest",
            "params": { "id": request_id }
        });
        self.write_line(&msg.to_string());
    }

    /// Kills the subprocess if still running.
    pub fn kill_child(&mut self) {
        if let Some(ref mut c) = self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Returns the `structuredContent` map from the last response, if present.
    pub fn structured_content(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.last_response
            .as_ref()
            .and_then(|r| r["result"]["structuredContent"].as_object())
    }

    /// Returns the `hints` map from `structuredContent`, if present.
    pub fn hints(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.structured_content()
            .and_then(|sc| sc.get("hints").and_then(|h| h.as_object()))
    }

    /// Returns the error object from the last response, if present.
    pub fn error_obj(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.last_response
            .as_ref()
            .and_then(|r| r["error"].as_object())
    }

    /// Returns the sandbox root path as a string (panics if not set).
    pub fn root_str(&self) -> String {
        self.allowlist_root
            .as_ref()
            .expect("allowlist_root not set")
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for SubstrateWorld {
    fn drop(&mut self) {
        self.kill_child();
    }
}

// ---------------------------------------------------------------------------
// Watchdog: kills the child after a fixed deadline per scenario.
// ---------------------------------------------------------------------------

/// Wraps a `Child` for the global watchdog timeout (8 seconds per scenario).
#[expect(dead_code, reason = "Watchdog is kept for future scenario timeout enforcement")]
pub struct Watchdog(Arc<Mutex<Option<std::process::Child>>>);

impl Watchdog {
    pub fn arm(child: Arc<Mutex<Option<std::process::Child>>>, timeout: Duration) {
        std::thread::spawn(move || {
            std::thread::sleep(timeout);
            if let Ok(mut g) = child.lock()
                && let Some(c) = g.as_mut()
            {
                let _ = c.kill();
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Runner entry point
// ---------------------------------------------------------------------------

/// The path to the feature directory, resolved relative to the workspace root.
fn features_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points to crates/substrate-mcp-server.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .expect("workspace root ancestor")
        .join("docs/arch/specs/features")
}

#[tokio::main]
async fn main() {
    SubstrateWorld::cucumber()
        .max_concurrent_scenarios(1)
        .run(features_dir())
        .await;
}
