//! substrate-mcp-server — composition root binary.
//!
//! Exposes POSIX baseutils-equivalent OS management over the MCP protocol via
//! STDIO transport (ADR-0005). Stdout is sacred: it carries the JSON-RPC channel.
//! All diagnostic output routes to stderr via the tracing subscriber.
//!
//! # Startup sequence
//!
//! 1. Initialize tracing to stderr (ADR-0005, ADR-0009).
//! 2. Install SIGPIPE `SIG_IGN` — single-threaded, before runtime (ADR-0032).
//! 3. Build tokio multi-thread runtime.
//! 4. Inside runtime: load config, probe capabilities, check degraded jail, wire
//!    composition root, install SIGTERM/SIGINT handlers, start MCP server.
//!
//! # Exit codes
//!
//! | Code | Meaning |
//! |------|---------|
//! | 0    | Clean shutdown |
//! | 70   | Logging subsystem init failed |
//! | 71   | Tokio runtime construction failed |
//! | 72   | SIGPIPE `SIG_IGN` failed |
//! | 73   | Composition root wiring failed |
//! | 74   | MCP server fatal error |
//! | 77   | `PathJail` degraded tier refused (`refuse_degraded_jail = true`) |
//! | 78   | Configuration invalid or not found |
//!
//! Per ADR-0036 startup error contract.

// `unsafe_code = "forbid"` is inherited from workspace [lints.rust].
// The sole ADR-0032 unsafe block (SIGPIPE SIG_IGN) is documented in
// signal_handlers.rs with a TODO for Wave D resolution.
#![cfg_attr(not(test), forbid(unsafe_code))]
#![warn(missing_docs)]

pub(crate) mod audit;
pub(crate) mod capability_probe;
pub(crate) mod composition;
pub(crate) mod config_loader;
pub(crate) mod handlers;
pub(crate) mod logging;
pub(crate) mod signal_handlers;
pub(crate) mod stub_ports;

use std::process::ExitCode;

use substrate_domain::JailTier;

fn main() -> ExitCode {
    // Step 1: Initialize tracing to stderr ONLY (ADR-0005, ADR-0009).
    // This happens before the runtime so that startup failures are logged.
    if let Err(e) = logging::init() {
        // Cannot use tracing here — subscriber is not yet installed.
        eprintln!("substrate: logging init failed: {e}");
        return ExitCode::from(70);
    }

    // Step 2: Ignore SIGPIPE — MUST happen in single-threaded context before
    // any additional threads are spawned (ADR-0032; ADR-0042 amendment clarifies
    // ordering: probe completes BEFORE this call when inside the runtime, but the
    // spec says SIG_IGN must be set before the runtime starts).
    // Resolution: set SIG_IGN before the runtime; capability probe runs inside
    // the runtime before accepting any connections.
    if let Err(e) = signal_handlers::ignore_sigpipe() {
        tracing::error!(?e, "SIGPIPE SIG_IGN installation failed");
        return ExitCode::from(72);
    }

    // Step 3: Build the tokio multi-thread work-stealing runtime (ADR-0003).
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("substrate-worker")
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(?e, "tokio runtime build failed");
            return ExitCode::from(71);
        },
    };

    // Step 4: Drive the async main function inside the runtime.
    runtime.block_on(async_main())
}

async fn async_main() -> ExitCode {
    // Step 4a: Load configuration (figment + TOML + env) (ADR-0006, ADR-0011).
    let config = match config_loader::load() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "configuration load failed (SUBSTRATE_CONFIG_INVALID)");
            return ExitCode::from(78);
        },
    };

    // Step 4b: Probe capabilities — runs once and caches in OnceLock (ADR-0042).
    // The probe MUST complete before any tool call is accepted; it runs here,
    // before the MCP transport is opened.
    let caps = capability_probe::probe();
    tracing::info!(
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        simd_tier = ?caps.simd_tier,
        walker_tier = ?caps.walker_tier,
        watcher_tier = ?caps.watcher_tier,
        jail_tier = ?caps.jail_tier,
        hash_tier = ?caps.hash_tier,
        stat_tier = ?caps.stat_tier,
        "capability tiers selected"
    );

    // Step 4c: PathJail degraded-tier policy (ADR-0042, ADR-0035).
    // Emit audit event unconditionally; abort if refused.
    if matches!(caps.jail_tier, JailTier::UserspaceDegraded) {
        audit::emit_jail_degraded(caps).await;
        if config.security.refuse_degraded_jail {
            tracing::error!(
                "PathJail tier 1 unavailable and security.refuse_degraded_jail=true — aborting"
            );
            return ExitCode::from(77);
        }
        tracing::warn!(
            "PathJail is userspace-degraded; TOCTOU window is not atomically closed. \
             Set security.refuse_degraded_jail=true to abort instead."
        );
    }

    // Step 4d: Wire the composition root (ADR-0022).
    let runtime_components = match composition::wire(&config, caps).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(code = e.code(), recovery_hint = e.recovery_hint(), "{e}");
            return ExitCode::from(73);
        },
    };

    // Step 4e: Install SIGTERM/SIGINT cooperative shutdown handler (ADR-0032).
    // Spawned as a separate task; it cancels the shutdown_token on signal receipt.
    let drain_token = runtime_components.shutdown_token.clone();
    tokio::spawn(signal_handlers::wait_for_shutdown(drain_token));

    // Step 4f: Run the MCP STDIO server (ADR-0005).
    match handlers::run_stdio_server(runtime_components).await {
        Ok(()) => {
            tracing::info!("substrate-mcp-server exiting cleanly");
            ExitCode::SUCCESS
        },
        Err(e) => {
            tracing::error!(code = e.code(), recovery_hint = e.recovery_hint(), "{e}");
            ExitCode::from(74)
        },
    }
}
