//! MCP protocol cancellation dispatch per ADR-0040.
//!
//! The MCP 2025-11-25 specification allows clients to send a
//! `notifications/cancelled` message carrying a `progressToken`. Per ADR-0040,
//! this token equals the `job_id`, so the dispatch is a direct lookup and
//! `CancellationToken::cancel()` call with no additional mapping table.
//!
//! This module provides a thin entry point used by `substrate-mcp-server` when
//! it receives `notifications/cancelled` on the STDIO transport. It does not
//! hold any state itself; all state lives in the [`InMemoryJobRegistry`].

use substrate_domain::errors::SubstrateResult;
use substrate_domain::jobs::state::JobState;
use substrate_domain::value_objects::JobId;

use crate::registry::InMemoryJobRegistry;
use substrate_domain::ports::job_registry::JobRegistryPort;

/// Handles a `notifications/cancelled` message from the MCP transport layer.
///
/// Parses `progress_token` as a [`JobId`] Crockford base32 string and delegates
/// to [`JobRegistryPort::cancel`] on the provided registry.
///
/// # Errors
///
/// - [`SubstrateError::InvalidArgument`] when `progress_token` is not a valid
///   26-character Crockford base32 string.
/// - [`SubstrateError::JobNotFound`] when the referenced job has expired or
///   never existed.
///
/// # Cancel safety
///
/// This function performs a single `await` on `registry.cancel`, which is
/// cancel-safe by contract of `JobRegistryPort`. Dropping the future at any
/// `await` point leaves the registry in a consistent state.
// Wave G+: wired by MCP server dispatcher on notifications/cancelled
#[expect(
    dead_code,
    reason = "Wave G+: wired by MCP server dispatcher on notifications/cancelled"
)]
pub(crate) async fn handle_mcp_cancelled(
    registry: &InMemoryJobRegistry,
    progress_token: &str,
) -> SubstrateResult<JobState> {
    let job_id = JobId::parse_crockford(progress_token)?;
    registry.cancel(&job_id).await
}
