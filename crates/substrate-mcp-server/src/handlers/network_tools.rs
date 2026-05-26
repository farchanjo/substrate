//! MCP tool handlers for network.* tools per ADR-0058.
//!
//! All four handlers delegate to `Arc<dyn NetworkInfoPort>` wired by the
//! composition root and convert domain results to `DispatchedResponse` envelopes.
//!
//! Tool descriptions are thin one-liners per the ADR-0007 2026-05-22 amendment;
//! full narrative arc lives in the companion `substrate` skill.

#![allow(
    clippy::redundant_pub_crate,
    reason = "binary crate: pub(crate) is conventional for cross-module access in binary crates"
)]

use std::sync::Arc;

use serde_json::{Value, json};
use tracing::instrument;

use substrate_domain::network::{NetworkTcpListRequest, NetworkUdpListRequest};
use substrate_domain::{SubstrateError, SubstrateResult, ports::network_info::NetworkInfoPort};

use crate::handlers::dispatcher::DispatchedResponse;

// ---- net_tcp_list -----------------------------------------------------------

/// Dispatches `net_tcp_list` — paginated TCP socket enumeration.
///
/// Deserializes args as [`NetworkTcpListRequest`], validates, delegates to the
/// wired [`NetworkInfoPort::list_tcp`], and returns a model-oriented text
/// summary plus the full JSON result as `structured_content`.
#[instrument(skip(port, args))]
pub(crate) async fn handle_net_tcp_list(
    args: Value,
    port: Arc<dyn NetworkInfoPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: NetworkTcpListRequest =
        serde_json::from_value(args).map_err(|e| SubstrateError::InvalidArgument {
            offending_field: "arguments".to_owned(),
            reason: e.to_string(),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        })?;

    req.validate()?;

    let result = port.list_tcp(req).await?;

    let total = result.total;
    let entry_count = result.entries.len();
    let next_offset = result.next_offset;

    let content =
        format!("net.tcp_list: total={total} entries={entry_count} next_offset={next_offset:?}.");

    let structured = json!({
        "entries": result.entries,
        "total": total,
        "next_offset": next_offset,
    });

    let hints = substrate_domain::Hints {
        next_action_suggested: if next_offset.is_some() {
            Some("net_tcp_list".to_owned())
        } else {
            None
        },
        ..substrate_domain::Hints::default()
    };

    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- net_udp_list -----------------------------------------------------------

/// Dispatches `net_udp_list` — paginated UDP socket enumeration.
///
/// Deserializes args as [`NetworkUdpListRequest`], validates, delegates to the
/// wired [`NetworkInfoPort::list_udp`], and returns a model-oriented text
/// summary plus the full JSON result as `structured_content`.
#[instrument(skip(port, args))]
pub(crate) async fn handle_net_udp_list(
    args: Value,
    port: Arc<dyn NetworkInfoPort>,
) -> SubstrateResult<DispatchedResponse> {
    let req: NetworkUdpListRequest =
        serde_json::from_value(args).map_err(|e| SubstrateError::InvalidArgument {
            offending_field: "arguments".to_owned(),
            reason: e.to_string(),
            correlation_id: Some(uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext))),
        })?;

    req.validate()?;

    let result = port.list_udp(req).await?;

    let total = result.total;
    let entry_count = result.entries.len();
    let next_offset = result.next_offset;

    let content =
        format!("net.udp_list: total={total} entries={entry_count} next_offset={next_offset:?}.");

    let structured = json!({
        "entries": result.entries,
        "total": total,
        "next_offset": next_offset,
    });

    let hints = substrate_domain::Hints {
        next_action_suggested: if next_offset.is_some() {
            Some("net_udp_list".to_owned())
        } else {
            None
        },
        ..substrate_domain::Hints::default()
    };

    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints,
    })
}

// ---- net_tcp_stats ----------------------------------------------------------

/// Dispatches `net_tcp_stats` — global TCP protocol counters.
///
/// Delegates to [`NetworkInfoPort::tcp_stats`] and returns the full
/// [`TcpStats`] value as `structured_content`.
#[instrument(skip(port, _args))]
pub(crate) async fn handle_net_tcp_stats(
    _args: Value,
    port: Arc<dyn NetworkInfoPort>,
) -> SubstrateResult<DispatchedResponse> {
    let stats = port.tcp_stats().await?;

    let content = format!(
        "net.tcp_stats: initiated={} accepted={} established={}.",
        stats.connections_initiated, stats.connections_accepted, stats.connections_established,
    );

    let structured = serde_json::to_value(&stats).unwrap_or(serde_json::Value::Null);

    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints: substrate_domain::Hints::default(),
    })
}

// ---- net_connection_count ---------------------------------------------------

/// Dispatches `net_connection_count` — TCP connection-state histogram.
///
/// Delegates to [`NetworkInfoPort::connection_count`] and returns the per-state
/// counts as `structured_content`. Cheaper than `net_tcp_list` when only
/// aggregate counts are needed.
#[instrument(skip(port, _args))]
pub(crate) async fn handle_net_connection_count(
    _args: Value,
    port: Arc<dyn NetworkInfoPort>,
) -> SubstrateResult<DispatchedResponse> {
    let counts = port.connection_count().await?;

    let content = format!(
        "net.connection_count: total={} states={}.",
        counts.total,
        counts.by_state.len(),
    );

    let structured = serde_json::to_value(&counts).unwrap_or(serde_json::Value::Null);

    Ok(DispatchedResponse {
        content,
        structured_content: structured,
        hints: substrate_domain::Hints::default(),
    })
}
