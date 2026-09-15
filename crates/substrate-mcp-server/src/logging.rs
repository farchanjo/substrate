//! Logging initializer — routes all diagnostic output to stderr.
//!
//! Per ADR-0005, stdout is the sacred MCP JSON-RPC channel. This module
//! MUST NOT write to stdout under any circumstances.
//!
//! # Format selection
//!
//! - `SUBSTRATE_LOG_FORMAT=json` — structured JSON per line (recommended for production).
//! - Any other value or absent — human-readable "pretty" format.
//!
//! # Level selection
//!
//! Standard `RUST_LOG` env var drives the filter. Defaults to `info` when absent.

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

use std::io;

/// Initializes the global tracing subscriber, writing all output to stderr.
///
/// Must be called once, before any other log-producing code runs.
/// Returns an error only when the subscriber registry is poisoned.
///
/// # Why `log_internal_errors(false)`
///
/// `tracing-subscriber` defaults this to `true`, which reports a failed write
/// by calling `eprintln!`. That is fatal here: when the MCP client dies, the
/// inherited stderr pipe breaks, so the write of the log line fails AND the
/// `eprintln!` that reports the failure targets the same broken pipe. `eprintln!`
/// panics on a failed write, and `panic = "abort"` turns that into a `SIGABRT`
/// that takes the whole server down — losing every supervised child with it.
///
/// The concrete sequence, recovered from a symbolicated crash report:
/// `tracing_subscriber::fmt::Subscriber::event` -> `std::io::stdio::__eprint`
/// -> `panic_with_hook` -> `rust_panic` -> `abort`.
///
/// Disabling internal-error logging keeps a broken stderr from being fatal:
/// the log line is simply dropped and the server keeps serving.
///
/// # Errors
///
/// Returns `io::Error` when the subscriber cannot be installed (rare; occurs
/// only if another subscriber was already set in the same process).
pub(crate) fn init() -> Result<(), io::Error> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let use_json =
        std::env::var("SUBSTRATE_LOG_FORMAT").is_ok_and(|v| v.eq_ignore_ascii_case("json"));

    if use_json {
        tracing_subscriber::fmt()
            .json()
            .with_writer(io::stderr)
            .log_internal_errors(false)
            .with_env_filter(filter)
            .try_init()
            .map_err(io::Error::other)
    } else {
        tracing_subscriber::fmt()
            .with_writer(io::stderr)
            .log_internal_errors(false)
            .with_env_filter(filter)
            .try_init()
            .map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A writer whose every write fails, standing in for a stderr pipe whose
    /// reader has died. Counts attempts so the test can prove the event really
    /// reached the writer instead of being dropped earlier.
    struct DeadPipe {
        attempts: Arc<AtomicUsize>,
    }

    impl io::Write for DeadPipe {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }

    /// Proves: a failed write on the log writer never panics.
    ///
    /// The subscriber is built exactly like [`init`] does, but with a writer
    /// that always fails. Emitting an event through it exercises the path that
    /// aborted in production: `Subscriber::event` -> failed write -> the
    /// internal-error branch. With `log_internal_errors(false)` that branch
    /// stays quiet instead of calling `eprintln!` against the same dead pipe.
    #[test]
    fn a_failed_write_does_not_panic() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&attempts);
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || DeadPipe {
                attempts: Arc::clone(&counter),
            })
            .log_internal_errors(false)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::error!("the writer is dead; emitting must not panic");
        });

        assert!(
            attempts.load(Ordering::Relaxed) > 0,
            "the event must have reached the dead writer"
        );
    }
}
