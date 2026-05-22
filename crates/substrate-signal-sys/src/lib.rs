//! FFI shim crate for signal handlers used by substrate-mcp-server.
//! NOT a public crate. Opts out of workspace lints because nix::sys::signal::signal
//! is unsafe, and substrate-mcp-server enforces #![cfg_attr(not(test),
//! forbid(unsafe_code))] which cannot be overridden in-place.

#![allow(unsafe_code)]

/// Sets `SIGPIPE` to `SIG_IGN` so broken-pipe conditions surface as `EPIPE` /
/// `io::ErrorKind::BrokenPipe` rather than terminating the process silently.
///
/// Per ADR-0032: "SIGPIPE `SIG_IGN` converts broken pipe to EPIPE error for
/// surface handling." Must be called in single-threaded context before the
/// tokio worker pool is started.
///
/// # Errors
///
/// Returns `Err` if the underlying `signal(2)` syscall fails.
#[cfg(unix)]
pub fn ignore_sigpipe() -> std::io::Result<()> {
    use nix::sys::signal::{SigHandler, Signal, signal};
    // SAFETY: signal() is safe to call from the main thread before any other
    // signal handlers are installed and before any I/O is performed.
    // substrate-mcp-server invokes this once during startup before tokio runtime
    // begins. SIG_IGN converts broken pipe into EPIPE error surfaced through
    // normal I/O error handling per ADR-0032.
    unsafe { signal(Signal::SIGPIPE, SigHandler::SigIgn) }
        .map(|_| ())
        .map_err(|e| std::io::Error::other(format!("SIGPIPE SIG_IGN failed: {e}")))
}

/// No-op on non-Unix platforms; broken-pipe handling is the runtime default.
///
/// # Errors
///
/// Always returns `Ok(())`.
#[cfg(not(unix))]
pub fn ignore_sigpipe() -> std::io::Result<()> {
    Ok(())
}
