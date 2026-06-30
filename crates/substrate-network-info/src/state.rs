//! Platform-specific TCP state code mappings.
//!
//! Each OS uses different integer encodings for the RFC 793 state machine.
//! This module provides the two platform mappings used by the adapter
//! implementations, keeping the logic centrally testable.

use substrate_domain::network::TcpState;

// ---- macOS ------------------------------------------------------------------

/// Maps macOS `<netinet/tcp_fsm.h>` TCP state codes (0..=10) to [`TcpState`].
///
/// The macOS numbering comes from `tcp_fsm.h` in XNU:
///
/// ```text
/// TCPS_CLOSED = 0, TCPS_LISTEN = 1, TCPS_SYN_SENT = 2,
/// TCPS_SYN_RECEIVED = 3, TCPS_ESTABLISHED = 4, TCPS_CLOSE_WAIT = 5,
/// TCPS_FIN_WAIT_1 = 6, TCPS_CLOSING = 7, TCPS_LAST_ACK = 8,
/// TCPS_FIN_WAIT_2 = 9, TCPS_TIME_WAIT = 10
/// ```
#[cfg(target_os = "macos")]
#[must_use]
pub const fn macos_state_from_u8(raw: u8) -> TcpState {
    match raw {
        0 => TcpState::Closed,
        1 => TcpState::Listen,
        2 => TcpState::SynSent,
        3 => TcpState::SynReceived,
        4 => TcpState::Established,
        5 => TcpState::CloseWait,
        6 => TcpState::FinWait1,
        7 => TcpState::Closing,
        8 => TcpState::LastAck,
        9 => TcpState::FinWait2,
        10 => TcpState::TimeWait,
        _ => TcpState::Unknown,
    }
}

// ---- Linux ------------------------------------------------------------------

/// Maps Linux `/proc/net/tcp` hex state field (0x01..=0x0B) to [`TcpState`].
///
/// Linux numbering comes from `<net/tcp_states.h>` and differs from macOS:
///
/// ```text
/// TCP_ESTABLISHED = 1, TCP_SYN_SENT = 2, TCP_SYN_RECV = 3,
/// TCP_FIN_WAIT1 = 4,   TCP_FIN_WAIT2 = 5, TCP_TIME_WAIT = 6,
/// TCP_CLOSE = 7,       TCP_CLOSE_WAIT = 8, TCP_LAST_ACK = 9,
/// TCP_LISTEN = 10,     TCP_CLOSING = 11
/// ```
#[cfg(target_os = "linux")]
#[must_use]
pub fn linux_state_from_hex(raw: u8) -> TcpState {
    match raw {
        0x01 => TcpState::Established,
        0x02 => TcpState::SynSent,
        0x03 => TcpState::SynReceived,
        0x04 => TcpState::FinWait1,
        0x05 => TcpState::FinWait2,
        0x06 => TcpState::TimeWait,
        0x07 => TcpState::Closed,
        0x08 => TcpState::CloseWait,
        0x09 => TcpState::LastAck,
        0x0A => TcpState::Listen,
        0x0B => TcpState::Closing,
        _ => TcpState::Unknown,
    }
}

// ---- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    mod macos_tests {
        use substrate_domain::network::TcpState;

        use crate::state::macos_state_from_u8;

        #[test]
        fn known_states_round_trip() {
            assert_eq!(macos_state_from_u8(0), TcpState::Closed);
            assert_eq!(macos_state_from_u8(1), TcpState::Listen);
            assert_eq!(macos_state_from_u8(2), TcpState::SynSent);
            assert_eq!(macos_state_from_u8(3), TcpState::SynReceived);
            assert_eq!(macos_state_from_u8(4), TcpState::Established);
            assert_eq!(macos_state_from_u8(5), TcpState::CloseWait);
            assert_eq!(macos_state_from_u8(6), TcpState::FinWait1);
            assert_eq!(macos_state_from_u8(7), TcpState::Closing);
            assert_eq!(macos_state_from_u8(8), TcpState::LastAck);
            assert_eq!(macos_state_from_u8(9), TcpState::FinWait2);
            assert_eq!(macos_state_from_u8(10), TcpState::TimeWait);
        }

        #[test]
        fn unknown_state_returns_unknown() {
            assert_eq!(macos_state_from_u8(11), TcpState::Unknown);
            assert_eq!(macos_state_from_u8(255), TcpState::Unknown);
        }
    }

    #[cfg(target_os = "linux")]
    mod linux_tests {
        use substrate_domain::network::TcpState;

        use crate::state::linux_state_from_hex;

        #[test]
        fn known_states_round_trip() {
            assert_eq!(linux_state_from_hex(0x01), TcpState::Established);
            assert_eq!(linux_state_from_hex(0x02), TcpState::SynSent);
            assert_eq!(linux_state_from_hex(0x03), TcpState::SynReceived);
            assert_eq!(linux_state_from_hex(0x04), TcpState::FinWait1);
            assert_eq!(linux_state_from_hex(0x05), TcpState::FinWait2);
            assert_eq!(linux_state_from_hex(0x06), TcpState::TimeWait);
            assert_eq!(linux_state_from_hex(0x07), TcpState::Closed);
            assert_eq!(linux_state_from_hex(0x08), TcpState::CloseWait);
            assert_eq!(linux_state_from_hex(0x09), TcpState::LastAck);
            assert_eq!(linux_state_from_hex(0x0A), TcpState::Listen);
            assert_eq!(linux_state_from_hex(0x0B), TcpState::Closing);
        }

        #[test]
        fn unknown_state_returns_unknown() {
            assert_eq!(linux_state_from_hex(0x00), TcpState::Unknown);
            assert_eq!(linux_state_from_hex(0x0C), TcpState::Unknown);
            assert_eq!(linux_state_from_hex(0xFF), TcpState::Unknown);
        }
    }
}
