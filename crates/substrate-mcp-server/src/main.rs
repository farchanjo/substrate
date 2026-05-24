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
pub(crate) mod null_id_shim;
pub(crate) mod signal_handlers;
pub(crate) mod stub_ports;

use std::process::ExitCode;

use substrate_domain::JailTier;

/// Emits the ADR-0036 startup-error JSON envelope to stderr, then flushes.
///
/// Must be called before every non-zero exit so that MCP hosts and CI pipelines
/// can parse the machine-readable `$schema = "substrate-startup-error/v1"` line.
/// Per ADR-0036 §"Emit Sequence": emit → flush → exit; no MCP frames on stdout.
fn emit_startup_error(code: &str, message: &str, recovery_hint: &str, details: &serde_json::Value) {
    // ADR-0036 requires a UUIDv7 correlation_id in the startup-error envelope.
    let correlation_id = uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext)).to_string();

    // ISO 8601 UTC timestamp (seconds precision — milliseconds not needed here).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let timestamp = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        1970 + secs / 31_557_600,
        (secs % 31_557_600 / 2_628_000) + 1,
        (secs % 2_628_000 / 86_400) + 1,
        (secs % 86_400) / 3_600,
        (secs % 3_600) / 60,
        secs % 60,
    );

    // Clamp recovery_hint to ≤ 150 chars per ADR-0036 field definition.
    let hint = if recovery_hint.len() > 150 {
        &recovery_hint[..150]
    } else {
        recovery_hint
    };

    let envelope = serde_json::json!({
        "$schema": "substrate-startup-error/v1",
        "code": code,
        "message_en_us": message,
        "recovery_hint": hint,
        "correlation_id": correlation_id,
        "timestamp": timestamp,
        "details": details,
    });

    // Single-line JSON per ADR-0036: consumers grep for `"$schema"`.
    eprintln!("{envelope}");
    // stderr flush is best-effort; eprintln! auto-flushes on most platforms.
}

fn main() -> ExitCode {
    // Step 1: Initialize tracing to stderr ONLY (ADR-0005, ADR-0009).
    // This happens before the runtime so that startup failures are logged.
    if let Err(e) = logging::init() {
        // Cannot use tracing here — subscriber is not yet installed.
        // ADR-0036: emit structured envelope before exit code 70.
        emit_startup_error(
            "SUBSTRATE_RUNTIME_INIT_FAILED",
            &format!("logging subsystem initialization failed: {e}"),
            "check stderr for OS-level errors; ensure stderr is writable",
            &serde_json::json!({ "component": "logging", "cause": e.to_string() }),
        );
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
        // ADR-0036: emit structured envelope before exit code 72.
        emit_startup_error(
            "SUBSTRATE_RUNTIME_INIT_FAILED",
            &format!("SIGPIPE SIG_IGN installation failed: {e}"),
            "check OS signal configuration; this is unusual on Linux/macOS",
            &serde_json::json!({ "component": "signal_handlers", "cause": e.to_string() }),
        );
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
            // ADR-0036: emit structured envelope before exit code 71.
            emit_startup_error(
                "SUBSTRATE_RUNTIME_INIT_FAILED",
                &format!("tokio multi-thread runtime build failed: {e}"),
                "check system resource limits (fd, threads); try ulimit -n 65536",
                &serde_json::json!({ "component": "tokio_runtime", "cause": e.to_string() }),
            );
            return ExitCode::from(71);
        },
    };

    // Step 4: Drive the async main function inside the runtime.
    runtime.block_on(async_main())
}

#[expect(
    clippy::too_many_lines,
    reason = "startup sequence: each step is a distinct initialization stage per ADR-0036; \
              extracting sub-functions would obscure the sequential error-code contract"
)]
async fn async_main() -> ExitCode {
    // Step 4a: Load configuration (figment + TOML + env) (ADR-0006, ADR-0011).
    let config = match config_loader::load() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "configuration load failed (SUBSTRATE_CONFIG_INVALID)");
            // ADR-0036: SUBSTRATE_CONFIG_INVALID → exit 78 (EX_CONFIG).
            emit_startup_error(
                "SUBSTRATE_CONFIG_INVALID",
                &format!("configuration load failed: {e}"),
                "check TOML syntax and field names; run substrate --check-config to validate",
                &serde_json::json!({ "cause": e.to_string() }),
            );
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
            // ADR-0036: SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE / degraded-jail → exit 77 (EX_NOPERM).
            emit_startup_error(
                "SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE",
                "PathJail tier 1 unavailable and security.refuse_degraded_jail=true",
                "upgrade the OS kernel (openat2 requires Linux ≥5.6) or set refuse_degraded_jail=false",
                &serde_json::json!({
                    "jail_tier": "UserspaceDegraded",
                    "has_openat2": caps.has_openat2,
                    "has_o_nofollow_any": caps.has_o_nofollow_any,
                }),
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
            // ADR-0036: map allowlist-root failures → exit 77 with the specific
            // SUBSTRATE_ALLOWLIST_ROOT_MISSING / SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE code
            // and a `details.path` field so the test harness and operators can identify
            // the offending root. All other composition errors → exit 73.
            let (exit_byte, error_code, details) = match e.code() {
                "SUBSTRATE_ALLOWLIST_ROOT_MISSING" | "SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE" => {
                    // Extract the root path from the error message when present.
                    // The error Display includes the path after the colon separator.
                    let path = e
                        .to_string()
                        .split_once(": ")
                        .map_or_else(|| e.to_string(), |(_, p)| p.to_owned());
                    (
                        77u8,
                        e.code(),
                        serde_json::json!({ "path": path, "error_code": e.code() }),
                    )
                },
                _ => (
                    73u8,
                    "SUBSTRATE_RUNTIME_INIT_FAILED",
                    serde_json::json!({ "error_code": e.code(), "cause": e.to_string() }),
                ),
            };
            emit_startup_error(
                error_code,
                &format!("composition root wiring failed: {e}"),
                e.recovery_hint(),
                &details,
            );
            return ExitCode::from(exit_byte);
        },
    };

    // Step 4e: Install SIGTERM/SIGINT cooperative shutdown handler (ADR-0032).
    // Spawned as a separate task; it cancels the shutdown_token on signal receipt.
    // When the `subprocess` feature is active, the signal handler also performs
    // cascade termination of all live subprocesses before cancelling the token
    // per ADR-0032 amendment (2026-05-24) and ADR-0053.
    let drain_token = runtime_components.shutdown_token.clone();
    #[cfg(feature = "subprocess")]
    let subprocess_shutdown_port = runtime_components.subprocess_for_shutdown.clone();
    #[cfg(feature = "subprocess")]
    let cascade_drain_secs = u64::from(runtime_components.config.shutdown_drain_secs);
    tokio::spawn(signal_handlers::wait_for_shutdown(
        drain_token,
        #[cfg(feature = "subprocess")]
        subprocess_shutdown_port,
        #[cfg(feature = "subprocess")]
        cascade_drain_secs,
    ));

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
