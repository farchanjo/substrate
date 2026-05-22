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
            .with_env_filter(filter)
            .try_init()
            .map_err(io::Error::other)
    } else {
        tracing_subscriber::fmt()
            .with_writer(io::stderr)
            .with_env_filter(filter)
            .try_init()
            .map_err(io::Error::other)
    }
}
