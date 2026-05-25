//! TCP statistics and connection-count aggregate value objects for the
//! network-info bounded context.
//!
//! These types mirror the `#TcpStats` and `#ConnectionCounts` definitions in
//! `docs/arch/schemas/network.cue` per ADR-0058 §"Wire Shape".
//!
//! References: ADR-0058.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::socket::TcpState;

// ---- TcpStats ---------------------------------------------------------------

/// Cumulative TCP protocol statistics for the host.
///
/// Counter fields are monotonically increasing since system boot (or the last
/// kernel counter wrap-around for 32-bit counters promoted to 64 bits). The
/// `captured_at` timestamp anchors the snapshot so callers can compute rates.
///
/// On macOS, counters come from `sysctl net.inet.tcp.stats`; on Linux from
/// `/proc/net/snmp` (the `Tcp:` and `TcpExt:` rows).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TcpStats {
    /// Total TCP segments received, including those received in error.
    pub segs_in: u64,

    /// Total TCP segments sent (excluding retransmissions).
    pub segs_out: u64,

    /// Number of TCP segments retransmitted.
    pub segs_retransmitted: u64,

    /// Total TCP packets received at the IP layer destined for TCP.
    pub rcv_packets: u64,

    /// Total TCP packets sent at the IP layer.
    pub snd_packets: u64,

    /// Number of active open connection attempts (SYN sent by local side).
    pub connections_initiated: u64,

    /// Number of passive open connection completions (SYN received, handshake completed).
    pub connections_accepted: u64,

    /// Connections that successfully reached the `Established` state.
    pub connections_established: u64,

    /// Connections that entered the `Closed` state via any path.
    pub connections_closed: u64,

    /// Connections dropped while in the persist timer state (zero-window probing).
    pub persist_timer_drops: u64,

    /// Connections dropped because the keepalive probe received no response.
    pub keepalive_drops: u64,

    /// Incoming segments discarded due to checksum failure.
    pub bad_checksums: u64,

    /// RFC 3339 UTC timestamp of when this snapshot was captured.
    #[serde(with = "time::serde::rfc3339")]
    pub captured_at: OffsetDateTime,
}

// ---- ConnectionCounts -------------------------------------------------------

/// Per-state connection count aggregate for the host.
///
/// `by_state` maps each [`TcpState`] that has at least one active connection to
/// the count of connections in that state. States with zero connections are
/// omitted from the map to reduce payload size. `total` is the sum of all values
/// in `by_state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionCounts {
    /// Map from TCP state to the number of connections currently in that state.
    ///
    /// Uses `BTreeMap` for deterministic serialisation order.
    pub by_state: BTreeMap<TcpState, u32>,

    /// Total number of TCP connections across all states.
    pub total: u32,

    /// RFC 3339 UTC timestamp of when this snapshot was captured.
    #[serde(with = "time::serde::rfc3339")]
    pub captured_at: OffsetDateTime,
}
