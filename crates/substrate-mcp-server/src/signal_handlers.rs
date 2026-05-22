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

/// Sets SIGPIPE to `SIG_IGN` so broken-pipe conditions surface as `EPIPE` /
/// `io::ErrorKind::BrokenPipe` rather than terminating the process silently.
///
/// Per ADR-0032: "SIGPIPE `SIG_IGN` converts broken pipe to EPIPE error for
/// surface handling." This must be called in single-threaded context before
/// `tokio::runtime::Builder` spawns worker threads.
///
/// # Implementation constraint (ADR-0032 + crate lint policy)
///
/// `main.rs` declares `#![cfg_attr(not(test), forbid(unsafe_code))]`, which
/// prevents any `unsafe` block anywhere in this crate — including the
/// `unsafe { nix::sys::signal::signal(...) }` call that installs `SIG_IGN`.
/// Both `nix::sys::signal::signal` and `nix::sys::signal::sigaction` are
/// `unsafe fn`, so no safe-Rust path exists without a separate crate or a
/// `forbid` → `deny` relaxation in `main.rs`.
///
/// The correct fix (deferred to Wave D) is to extract the signal-setup logic
/// into a dedicated `substrate-signal` crate with its own
/// `[lints.rust] unsafe_code = "deny"` (workspace `forbid` overridden at
/// crate level per Cargo lint precedence), include a SAFETY comment, and add
/// a `miri` coverage test. Until that crate exists this function is a safe
/// no-op: tokio converts `EPIPE` to `io::ErrorKind::BrokenPipe` on most
/// platforms, which the MCP dispatch layer already handles gracefully.
///
/// Wave D action: move this into `substrate-signal` crate, relax `unsafe_code`
/// to `"deny"` at crate level, then call:
/// ```text
///   // SAFETY: signal(SIGPIPE, SIG_IGN) is async-signal-safe and safe to call
///   // in single-threaded context before the tokio worker pool is started.
///   // SigHandler::SigIgn does not invoke a user-space handler; no re-entrancy
///   // risk. ADR-0032: "SIGPIPE SIG_IGN converts broken pipe to EPIPE error
///   // for surface handling."
///   unsafe { nix::sys::signal::signal(Signal::SIGPIPE, SigHandler::SigIgn)?; }
/// ```
///
/// # Errors
///
/// Always returns `Ok(())` (no-op; see constraint note above).
#[expect(
    clippy::unnecessary_wraps,
    reason = "Result signature preserved so call-site is unchanged when Wave D adds the real SIG_IGN install in substrate-signal"
)]
pub(crate) const fn ignore_sigpipe() -> Result<(), nix::Error> {
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
