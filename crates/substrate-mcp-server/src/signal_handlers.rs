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

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

use tokio_util::sync::CancellationToken;

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
#[expect(
    clippy::expect_used,
    reason = "signal handler installation failures at startup are non-recoverable; panic is the correct response"
)]
pub(crate) async fn wait_for_shutdown(token: CancellationToken) {
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

    token.cancel();
    tracing::info!("shutdown token cancelled; drain window begins");
}
