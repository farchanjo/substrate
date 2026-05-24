//! Step definition modules for the subprocess bounded context (ADR-0052).
//!
//! This module is compiled only when the `subprocess` Cargo feature is active.
//! Run subprocess cucumber scenarios with:
//!   cargo test --test cucumber --features subprocess
//!
//! Wave 2.5a owns: policy, capture, quota, elicitation, reaper.
//! Wave 2.5b owns: cancel, cascade, watchdog.

#![cfg(feature = "subprocess")]

// Wave 2.5a modules (this agent).
pub mod capture;
pub mod elicitation;
pub mod policy;
pub mod quota;
pub mod reaper;

// Wave 2.5b modules — implementations provided by the Wave 2.5b agent.
pub mod cancel;
pub mod cascade;
pub mod watchdog;

// ---------------------------------------------------------------------------
// Shared helpers used across step modules
// ---------------------------------------------------------------------------

use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use substrate_policy::Allowlist;
use substrate_subprocess::registry::{BinaryAllowlist, SubprocessRegistry};

/// Returns the platform-appropriate path for the `echo` binary.
///
/// On macOS the POSIX echo binary lives at `/bin/echo` (not `/usr/bin/echo`).
/// On Linux it is typically at `/usr/bin/echo`. This helper probes in order.
///
/// Feature files may reference `/usr/bin/echo` by name; step implementations
/// MUST use this helper to get the actual spawnable binary path.
pub fn echo_binary_path() -> PathBuf {
    for candidate in &["/usr/bin/echo", "/bin/echo"] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return p;
        }
    }
    // Final fallback — will produce a SpawnFailed if neither exists.
    PathBuf::from("/usr/bin/echo")
}

/// Builds a [`SubprocessRegistry`] whose binary allowlist contains exactly
/// the platform echo binary and whose path allowlist roots are restricted to `roots`.
///
/// The registry uses conservative defaults appropriate for unit-style tests:
/// max 4 per-client, max 8 global, 64 KiB aggregate buffer, 5 s drain.
pub fn make_registry_with_echo(roots: Vec<PathBuf>) -> Arc<SubprocessRegistry> {
    let binary_allowlist = BinaryAllowlist::new(vec![echo_binary_path()]);
    let path_allowlist = Allowlist::new(roots).expect("create test Allowlist");
    let root_cancel = CancellationToken::new();
    SubprocessRegistry::new(
        binary_allowlist,
        Vec::new(),
        4,
        8,
        65_536,
        5,
        path_allowlist,
        root_cancel,
    )
}

/// Builds a deny-all [`SubprocessRegistry`] — no binaries are permitted.
///
/// Used by the spawn-not-in-allowlist tests to verify that unrecognised binaries
/// are rejected at Layer 5 before any OS fork/exec is attempted.
pub fn make_deny_all_registry(roots: Vec<PathBuf>) -> Arc<SubprocessRegistry> {
    let path_allowlist = Allowlist::new(roots).expect("create test Allowlist");
    let root_cancel = CancellationToken::new();
    substrate_subprocess::registry::deny_all_registry(path_allowlist, root_cancel)
}

/// Builds a [`SubprocessRegistry`] that also allows the fixture binary
/// `subprocess_stdout_writer` (resolved from the Cargo-generated env var).
pub fn make_registry_with_fixture(roots: Vec<PathBuf>) -> Arc<SubprocessRegistry> {
    let fixture_path = fixture_binary_path();
    let binary_allowlist = BinaryAllowlist::new(vec![echo_binary_path(), fixture_path]);
    let path_allowlist = Allowlist::new(roots).expect("create test Allowlist");
    let root_cancel = CancellationToken::new();
    SubprocessRegistry::new(
        binary_allowlist,
        Vec::new(),
        4,
        8,
        65_536,
        5,
        path_allowlist,
        root_cancel,
    )
}

/// Resolves the path to the `subprocess_stdout_writer` fixture binary.
///
/// Cargo sets `CARGO_BIN_EXE_subprocess_stdout_writer` for example targets.
/// When that env var is absent the function falls back to the workspace debug
/// build location, which is where `cargo build --example` places examples.
pub fn fixture_binary_path() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by cargo when the example is built.
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_subprocess_stdout_writer") {
        return PathBuf::from(p);
    }
    // Fallback: locate relative to CARGO_MANIFEST_DIR (crates/substrate-mcp-server).
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(manifest).join("../../target/debug/examples/subprocess_stdout_writer")
}

/// A trivial `CancelSignal` that never cancels.
///
/// Required because `SubprocessPort::spawn` takes `&dyn CancelSignal`.
/// We define it here rather than importing the private `NoCancel` from
/// `substrate-mcp-server/src/handlers/subprocess_tools.rs`.
pub struct NoCancel;

#[async_trait::async_trait]
impl substrate_domain::ports::fs_index::CancelSignal for NoCancel {
    fn is_cancelled(&self) -> bool {
        false
    }

    async fn cancelled(&self) {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn echo_binary_path_exists() {
        let p = echo_binary_path();
        assert!(p.exists(), "echo binary not found at {p:?}");
    }
}
