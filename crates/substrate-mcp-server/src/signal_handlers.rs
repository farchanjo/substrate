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

#![allow(clippy::redundant_pub_crate, reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates")]

use tokio_util::sync::CancellationToken;

// ---- SIGPIPE -----------------------------------------------------------------

/// Sets SIGPIPE to `SIG_IGN` in the calling process.
///
/// Must be called in single-threaded context before `tokio::runtime::Builder`
/// spawns worker threads. Returns `Ok(())` on success.
///
/// # Errors
///
/// Returns a `nix::Error` when the `signal(2)` call fails (extremely rare;
/// would indicate a kernel-level constraint on signal disposition).
/// Sets SIGPIPE to `SIG_IGN` in the calling process (ADR-0032).
///
/// Must be called in single-threaded context before `tokio::runtime::Builder`
/// spawns worker threads. Returns `Ok(())` on success.
///
/// # Implementation note (Wave B scaffold)
///
/// `nix::sys::signal::signal` is `unsafe fn`. The workspace lint table sets
/// `unsafe_code = "forbid"`, which cannot be overridden per-function without
/// removing `workspace = true` from `[lints]`. The Wave D implementation will
/// resolve this by either:
///   (a) Moving `ignore_sigpipe` to a dedicated crate that opts out of the
///       workspace forbid via its own `[lints.rust] unsafe_code = "deny"`, or
///   (b) Using the `rlimit` / `signal-hook` crate's safe `SigAction` wrapper.
///
/// For the Wave B scaffold, SIGPIPE handling is intentionally a no-op. The
/// tokio runtime converts `BrokenPipe` to `io::ErrorKind::BrokenPipe` on many
/// platforms anyway; the scaffold does not perform long-running stdout writes.
///
/// TODO Wave D: implement real `SIG_IGN` via safe wrapper crate.
///
/// # Errors
///
/// Always returns `Ok(())` in the Wave B scaffold.
#[expect(
    clippy::unnecessary_wraps,
    reason = "Wave B scaffold — Wave D will perform a real SIGPIPE install that can fail; preserving Result signature avoids call-site churn"
)]
pub(crate) const fn ignore_sigpipe() -> Result<(), nix::Error> {
    // TODO Wave D: call `nix::sys::signal::signal(Signal::SIGPIPE, SigHandler::SigIgn)`
    // inside an `unsafe` block with a SAFETY comment, from a crate module that
    // uses `#![allow(unsafe_code)]` with `[lints.rust] unsafe_code = "deny"`
    // (workspace forbid overridden at crate level per Cargo lint precedence).
    Ok(())
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
