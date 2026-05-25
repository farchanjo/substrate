//! Socket-level value objects for the network-info bounded context.
//!
//! These types mirror the `#SocketEntry`, `#Protocol`, `#AddrFamily`, and
//! `#TcpState` definitions in `docs/arch/schemas/network.cue` and are stable
//! across the wire format per ADR-0058 §"Wire Shape".
//!
//! References: ADR-0058.

use serde::{Deserialize, Serialize};

// ---- Protocol ---------------------------------------------------------------

/// IP transport-layer protocol for a socket entry.
///
/// Serialized as `"Tcp"` / `"Udp"` (PascalCase) to match the CUE wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Protocol {
    /// Transmission Control Protocol.
    Tcp,
    /// User Datagram Protocol.
    Udp,
}

// ---- AddrFamily -------------------------------------------------------------

/// IP address family for a socket entry.
///
/// Serialized as `"Inet"` / `"Inet6"` (PascalCase) to match the CUE wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum AddrFamily {
    /// IPv4 (`AF_INET`).
    Inet,
    /// IPv6 (`AF_INET6`).
    Inet6,
}

// ---- TcpState ---------------------------------------------------------------

/// TCP connection state as reported by the OS.
///
/// Serialized as PascalCase (e.g., `"Established"`, `"TimeWait"`) to match the
/// CUE wire format. `Unknown` covers any OS-reported state that falls outside the
/// RFC 793 state machine (e.g., Linux internal states on SYN cookies).
///
/// Implements `Eq + Hash` so it can be used as a key in the `BTreeMap` inside
/// [`ConnectionCounts`](super::stats::ConnectionCounts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum TcpState {
    /// Socket is closed.
    Closed,
    /// Socket is listening for incoming connections.
    Listen,
    /// Active connection initiation: SYN sent.
    SynSent,
    /// Passive connection initiation: SYN received, SYN-ACK sent.
    SynReceived,
    /// Connection is established and data can flow.
    Established,
    /// Active close initiated: FIN sent, waiting for ACK.
    FinWait1,
    /// FIN acknowledged; waiting for remote FIN.
    FinWait2,
    /// Passive close: remote sent FIN, waiting for local application to close.
    CloseWait,
    /// Both sides initiated close simultaneously; waiting for ACK of FIN.
    Closing,
    /// Last ACK of FIN sent by local side; waiting for its acknowledgement.
    LastAck,
    /// Waiting for all duplicate packets to expire after connection close.
    TimeWait,
    /// OS-reported state does not map to any known RFC 793 state.
    Unknown,
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::TcpState;

    /// Regression: `state_filter = ["Listen"]` from an MCP client must deserialize
    /// to `TcpState::Listen`. All variants use `rename_all = "PascalCase"`, so the
    /// JSON wire value `"Listen"` must round-trip through serde correctly.
    #[test]
    fn tcp_state_listen_serde_round_trip() {
        let encoded = serde_json::to_string(&TcpState::Listen)
            .expect("serialization must succeed");
        assert_eq!(encoded, r#""Listen""#, "TcpState::Listen must serialize as \"Listen\"");

        let decoded: TcpState = serde_json::from_str(r#""Listen""#)
            .expect("deserialization of \"Listen\" must succeed");
        assert_eq!(decoded, TcpState::Listen, "\"Listen\" must deserialize to TcpState::Listen");
    }

    /// Verify that every named variant serializes to and from PascalCase.
    #[test]
    fn tcp_state_all_variants_round_trip() {
        let cases: &[(TcpState, &str)] = &[
            (TcpState::Closed, "\"Closed\""),
            (TcpState::Listen, "\"Listen\""),
            (TcpState::SynSent, "\"SynSent\""),
            (TcpState::SynReceived, "\"SynReceived\""),
            (TcpState::Established, "\"Established\""),
            (TcpState::FinWait1, "\"FinWait1\""),
            (TcpState::FinWait2, "\"FinWait2\""),
            (TcpState::CloseWait, "\"CloseWait\""),
            (TcpState::Closing, "\"Closing\""),
            (TcpState::LastAck, "\"LastAck\""),
            (TcpState::TimeWait, "\"TimeWait\""),
            (TcpState::Unknown, "\"Unknown\""),
        ];
        for (state, expected_json) in cases {
            let got = serde_json::to_string(state).expect("serialization must succeed");
            assert_eq!(&got, expected_json, "wrong serialization for {state:?}");
            let back: TcpState = serde_json::from_str(&got).expect("deserialization must succeed");
            assert_eq!(&back, state, "round-trip failed for {state:?}");
        }
    }
}

// ---- SocketEntry ------------------------------------------------------------

/// A single socket entry as reported by the OS network stack.
///
/// Fields follow the CUE `#SocketEntry` schema from ADR-0058. Text addresses
/// (`local_addr`, `remote_addr`) are textual IPv4 or IPv6 representations, never
/// raw bytes, so they can be embedded directly in JSON responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocketEntry {
    /// Transport-layer protocol.
    pub protocol: Protocol,

    /// IP address family.
    pub family: AddrFamily,

    /// Textual local IPv4 or IPv6 address (e.g., `"127.0.0.1"`, `"::1"`).
    pub local_addr: String,

    /// Local port number.
    pub local_port: u16,

    /// Textual remote IPv4 or IPv6 address.
    ///
    /// `None` for listening sockets or unconnected UDP sockets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_addr: Option<String>,

    /// Remote port number.
    ///
    /// `None` for listening sockets or unconnected UDP sockets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_port: Option<u16>,

    /// Current TCP connection state.
    ///
    /// For UDP sockets this is always `TcpState::Unknown`.
    pub state: TcpState,

    /// PID of the process that owns this socket.
    ///
    /// `None` when PID resolution fails (insufficient privileges, kernel race,
    /// or `resolve_pid = false` in the request).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,

    /// Kernel inode number for this socket.
    ///
    /// `None` when inode information is unavailable (e.g., macOS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inode: Option<u64>,
}
