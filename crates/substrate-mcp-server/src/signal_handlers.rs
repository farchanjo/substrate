//! Signal handler installation per ADR-0032.
//!
//! - `SIGPIPE`: ignored before runtime starts; broken pipes surface as
//!   `io::ErrorKind::BrokenPipe` (handled by the MCP dispatch layer).
//! - `SIGTERM` / `SIGINT`: trigger cooperative shutdown via `CancellationToken`.
//! - `SIGHUP`: held but never polled; prevents the default terminate action.
//!
//! ADR-0032 amendment (ADR-0040): on shutdown, the `JobRegistry` propagates
//! cancellation to every active job before the drain window begins.
//! ADR-0032 amendment (ADR-0042): capability probe must complete before
//! signal handlers are installed.
//! ADR-0032 amendment (2026-05-24): when the `subprocess` Cargo feature is
//! active, the graceful shutdown sequence includes subprocess cascade
//! termination (SIGTERM → drain window → SIGKILL for survivors) BEFORE
//! the root `CancellationToken` is cancelled. This preserves the SIGPIPE/SIGTERM
//! ordering contract while ensuring no orphan subprocesses are left behind.

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

use tokio_util::sync::CancellationToken;

#[cfg(feature = "subprocess")]
use std::sync::Arc;

// ---- SIGPIPE -----------------------------------------------------------------

/// Sets `SIGPIPE` to `SIG_IGN` so broken-pipe conditions surface as `EPIPE` /
/// `io::ErrorKind::BrokenPipe` rather than terminating the process silently.
///
/// Per ADR-0032: "SIGPIPE `SIG_IGN` converts broken pipe to EPIPE error for
/// surface handling." Must be called in single-threaded context before
/// `tokio::runtime::Builder` spawns worker threads.
///
/// Delegates to `substrate_signal_sys::ignore_sigpipe`, which opts out of the
/// workspace `unsafe_code = "deny"` lint so it may call the `unsafe`
/// `nix::sys::signal::signal` syscall with a SAFETY comment. This crate retains
/// `#![cfg_attr(not(test), forbid(unsafe_code))]` and is never modified.
///
/// # Errors
///
/// Propagates `std::io::Error` if the underlying `signal(2)` syscall fails.
pub(crate) fn ignore_sigpipe() -> std::io::Result<()> {
    substrate_signal_sys::ignore_sigpipe()
}

// ---- SIGTERM / SIGINT / SIGHUP -----------------------------------------------

/// Waits for SIGTERM or SIGINT, then cancels `token` to begin graceful drain.
///
/// SIGHUP is held open to suppress the default terminate action (ADR-0032).
/// This function is designed to run in a dedicated `tokio::spawn`'d task and
/// resolves after cancellation is broadcast.
///
/// The caller (composition root) must join in-flight tool tasks up to
/// `shutdown_drain_secs` after `token` is cancelled.
///
/// When the `subprocess` Cargo feature is active and `subprocess_registry` is
/// `Some`, the cascade termination sequence (ADR-0032 amendment 2026-05-24,
/// ADR-0053) runs AFTER signal receipt but BEFORE `token.cancel()`:
///
/// 1. Call `subprocess_port.list(...)` to enumerate live handles.
/// 2. For each handle: send SIGTERM to the process group (`killpg`).
/// 3. Wait up to `cascade_drain_secs` for all handles to exit naturally.
/// 4. Send SIGKILL to any survivors (forceful cascade kill per ADR-0053).
/// 5. Cancel `token` to propagate shutdown to Bucket B/C job workers.
///
/// This ordering ensures no orphan subprocesses are left behind when the
/// MCP server receives SIGTERM.
#[expect(
    clippy::expect_used,
    reason = "signal handler installation failures at startup are non-recoverable; panic is the correct response"
)]
pub(crate) async fn wait_for_shutdown(
    token: CancellationToken,
    #[cfg(feature = "subprocess")] subprocess_registry: Option<
        Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>,
    >,
    #[cfg(feature = "subprocess")] cascade_drain_secs: u64,
) {
    // Install SIGHUP handler; hold receiver to prevent default termination.
    // Never polled; presence is sufficient.
    #[cfg(unix)]
    let mut _sighup = {
        use tokio::signal::unix::SignalKind;
        tokio::signal::unix::signal(SignalKind::hangup()).expect("SIGHUP handler install failed")
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::SignalKind;
        let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate())
            .expect("SIGTERM handler install failed");
        let mut sigint = tokio::signal::unix::signal(SignalKind::interrupt())
            .expect("SIGINT handler install failed");

        tokio::select! {
            biased;
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received — beginning graceful shutdown");
            }
            _ = sigint.recv() => {
                tracing::info!("SIGINT received — beginning graceful shutdown");
            }
        }
    }

    #[cfg(windows)]
    {
        // On Windows, SIGTERM is not available; use Ctrl-C only.
        tokio::signal::ctrl_c()
            .await
            .expect("Ctrl-C handler install failed");
        tracing::info!("Ctrl-C received — beginning graceful shutdown");
    }

    // ---- Subprocess cascade termination (ADR-0032 amendment 2026-05-24) ----
    //
    // When the `subprocess` feature is active, terminate all live subprocesses
    // BEFORE cancelling the root token. This prevents orphan child processes
    // from surviving the MCP server's lifetime.
    //
    // The cascade sequence per ADR-0053:
    //   1. SIGTERM to every process group.
    //   2. Wait `cascade_drain_secs` for natural exit.
    //   3. SIGKILL any survivors.
    #[cfg(feature = "subprocess")]
    {
        if let Some(ref port) = subprocess_registry {
            terminate_subprocesses_on_shutdown(port, cascade_drain_secs).await;
        }
    }

    token.cancel();
    tracing::info!("shutdown token cancelled; drain window begins");
}

// ---- Subprocess cascade helper (feature = "subprocess") --------------------

/// Terminates all live subprocesses during graceful shutdown.
///
/// Sends SIGTERM to each process group, waits `drain_secs`, then sends SIGKILL
/// to any survivors. Silently ignores all errors (best-effort; server is
/// shutting down anyway).
#[cfg(feature = "subprocess")]
async fn terminate_subprocesses_on_shutdown(
    port: &Arc<dyn substrate_domain::ports::subprocess::SubprocessPort>,
    drain_secs: u64,
) {
    use substrate_domain::{
        ports::subprocess::{SignalTarget, SubprocessSignalName},
        value_objects::ClientId,
    };

    // Enumerate all live handles. Use a synthetic client-id; the port's list
    // implementation shows all handles regardless of client when the filter is None.
    //
    // SAFETY: "shutdown" matches the pattern `^[A-Za-z0-9._-]{1,64}$`; parse cannot fail.
    #[expect(
        clippy::expect_used,
        reason = "static string 'shutdown' always satisfies ClientId pattern [A-Za-z0-9._-]{1,64}"
    )]
    let client_id = ClientId::parse("shutdown").expect("static client_id parse cannot fail");

    let handles = match port.list(&client_id, None, None, 500).await {
        Ok((h, _)) => h,
        Err(e) => {
            tracing::warn!(error = %e, "subprocess list during shutdown failed; skipping cascade");
            return;
        },
    };

    if handles.is_empty() {
        return;
    }

    tracing::info!(
        count = handles.len(),
        "sending SIGTERM to all subprocess process groups (cascade drain)"
    );

    // Send SIGTERM to each process group.
    for handle in &handles {
        if let Err(e) = port
            .signal(
                &handle.job_id,
                SubprocessSignalName::Sigterm,
                SignalTarget::ProcessGroup,
            )
            .await
        {
            tracing::debug!(job_id = %handle.job_id, error = %e, "SIGTERM to subprocess group failed (may have already exited)");
        }
    }

    // Drain window: wait for natural exit.
    if drain_secs > 0 {
        tokio::time::sleep(std::time::Duration::from_secs(drain_secs)).await;
    }

    // SIGKILL survivors.
    let Ok((post_drain, _)) = port.list(&client_id, None, None, 500).await else {
        return;
    };

    for handle in &post_drain {
        tracing::warn!(
            job_id = %handle.job_id,
            "subprocess still alive after drain window; sending SIGKILL"
        );
        let _ = port
            .signal(
                &handle.job_id,
                SubprocessSignalName::Sigkill,
                SignalTarget::ProcessGroup,
            )
            .await;
    }

    tracing::info!("subprocess cascade termination complete");
}
