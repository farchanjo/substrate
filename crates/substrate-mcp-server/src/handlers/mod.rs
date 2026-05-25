//! MCP request handlers — the inbound adapter layer.
//!
//! `run_stdio_server` opens the rmcp STDIO transport and drives the JSON-RPC
//! dispatch loop until the shutdown token is cancelled.
//!
//! Per ADR-0005: stdout is the sacred MCP channel. No `println!` or `print!`
//! macro may appear anywhere in this module or its children.
//! Per ADR-0013: protocol version negotiation uses the `initialize` handler in
//! `initialize.rs`.
//! Per ADR-0022: all tool routing is centralised in `dispatcher.rs`.

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

pub(crate) mod dispatcher;
pub(crate) mod initialize;
pub(crate) mod job_tools;
pub(crate) mod rmcp_progress_notifier;
#[cfg(feature = "subprocess")]
pub(crate) mod rmcp_stream_notifier;
pub(crate) mod service;
#[cfg(feature = "subprocess")]
pub(crate) mod subprocess_tools;

use std::sync::Arc;

use rmcp::ServiceExt as _;
use substrate_domain::SubstrateResult;

use crate::composition::RuntimeComponents;
use service::SubstrateService;

/// Opens the STDIO transport and runs the MCP JSON-RPC dispatch loop.
///
/// This is the production STDIO entry point per ADR-0005.  It:
///
/// 1. Constructs a `SubstrateService` wrapping the `ToolDispatcher`.
/// 2. Opens rmcp's STDIO transport (`stdin` + `stdout`).
/// 3. Calls `ServiceExt::serve_with_ct` so the service loop exits when
///    `shutdown_token` is cancelled (SIGTERM/SIGINT handler; ADR-0032).
/// 4. Awaits the `RunningService` until the transport closes or the CT fires.
/// 5. Drains: logs drain-start event, waits up to `shutdown_drain_secs` for
///    in-flight Bucket B/C jobs to complete (they observe child tokens from
///    `shutdown_token` via `InMemoryJobRegistry`; ADR-0037).
///
/// # `notifications/cancelled` wiring (rmcp 1.7)
///
/// rmcp 1.7 exposes `ServerHandler::on_cancelled` which fires for every
/// `notifications/cancelled` message.  `SubstrateService::on_cancelled`
/// parses the `request_id` as a `JobId` and calls
/// `ToolDispatcher.jobs.cancel(job_id)` per ADR-0040 triple-equality.
///
/// # Returns
///
/// `Ok(())` on clean shutdown.  `Err(SUBSTRATE_INTERNAL_ERROR)` when the
/// rmcp initialization handshake fails (e.g. client sends a wrong first
/// message before `initialize`).
pub(crate) async fn run_stdio_server(rt: RuntimeComponents) -> SubstrateResult<()> {
    // The async job control-plane is ALWAYS wired (ADR-0040). The composition
    // root unconditionally constructs `InMemoryJobRegistry` regardless of whether
    // the operator supplied a `[jobs]` TOML section — absent config falls back to
    // `JobConfig::default()`. Setting this to `rt.config.jobs.is_some()` was wrong:
    // it returned `false` for default-config deployments, causing Cucumber's
    // `capabilities.experimental.substrate.jobs` assertion to fail.
    let jobs_wired = true;

    tracing::info!(
        max_in_flight = rt.config.protocol.max_in_flight_requests,
        shutdown_drain_secs = rt.config.shutdown_drain_secs,
        jobs_wired,
        "MCP STDIO server starting — opening transport"
    );

    // Wrap the dispatcher in an Arc so `SubstrateService` can be cloned cheaply
    // by rmcp for concurrent request dispatch.
    let dispatcher = Arc::new(rt.dispatcher);

    let service = SubstrateService::new(
        Arc::clone(&dispatcher),
        rt.caps,
        rt.shutdown_token.clone(),
        jobs_wired,
        rt.notifier,
    );

    // Open STDIO transport with the null-id shim applied (ADR-0013 §null-id).
    //
    // JSON-RPC 2.0 §4 permits `"id": null`.  rmcp 1.7 models RequestId as
    // NumberOrString and therefore silently drops requests with id=null (they
    // deserialise as Notifications).  The shim intercepts each JSON line:
    //   - inbound:  rewrites `"id": null` → `"id": <sentinel>` (i64::MIN)
    //   - outbound: restores `"id": <sentinel>` → `"id": null`
    // This is transparent to rmcp and satisfies the JSON-RPC spec invariant.
    //
    // `stdout` is still sacred per ADR-0005 — the shim is the only writer.
    let (shim_stdin, shim_stdout) = crate::null_id_shim::null_id_pair();
    let transport = (shim_stdin, shim_stdout);

    // `serve_with_ct` runs the initialize handshake then enters the main loop.
    // When `shutdown_token` is cancelled the loop exits after the current
    // in-flight request completes.
    let running = match service
        .serve_with_ct(transport, rt.shutdown_token.clone())
        .await
    {
        Ok(svc) => svc,
        Err(e) => {
            tracing::error!(error = %e, "rmcp STDIO server initialization failed");
            return Err(substrate_domain::SubstrateError::InternalError {
                reason: format!("rmcp initialization failed: {e}"),
                correlation_id: None,
            });
        },
    };

    tracing::info!("MCP STDIO server initialized — serving requests");

    // Wait until the transport closes (client disconnect) or the CT fires.
    // `RunningService::waiting` consumes self and resolves with `QuitReason`.
    let quit = running.waiting().await;
    tracing::debug!(?quit, "rmcp service loop exited");

    tracing::info!(
        drain_secs = rt.config.shutdown_drain_secs,
        "shutdown token fired — draining in-flight requests"
    );

    // Cancel the root token so all Bucket B/C job workers observe their child
    // tokens and exit cooperatively per ADR-0037.  This is idempotent: if the
    // token is already cancelled (e.g. from the signal handler) this is a no-op.
    rt.shutdown_token.cancel();

    // Drain window: give in-flight jobs time to complete gracefully.
    // We sleep for `shutdown_drain_secs` then exit regardless; any still-running
    // tasks hold only child tokens and will be abandoned when the process exits.
    tokio::time::sleep(std::time::Duration::from_secs(u64::from(
        rt.config.shutdown_drain_secs,
    )))
    .await;

    tracing::info!("drain complete — exiting MCP server");
    Ok(())
}
