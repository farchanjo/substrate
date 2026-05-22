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

mod steps;

use std::{
    io::{BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
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
        self.send_rpc(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "cucumber-test", "version": "0.0.1" }
            }),
        );
        // Drain the initialize response.
        let _init = self.recv_rpc();
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
    pub fn call_tool_and_store(&mut self, tool: &str, arguments: serde_json::Value) {
        self.call_tool(tool, arguments);
        let resp = self.recv_rpc();
        self.last_response = Some(resp);
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
#[allow(dead_code)]
pub struct Watchdog(Arc<Mutex<Option<std::process::Child>>>);

impl Watchdog {
    pub fn arm(child: Arc<Mutex<Option<std::process::Child>>>, timeout: Duration) {
        std::thread::spawn(move || {
            std::thread::sleep(timeout);
            if let Ok(mut g) = child.lock() {
                if let Some(c) = g.as_mut() {
                    let _ = c.kill();
                }
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
