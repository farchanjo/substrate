//! Network-info bounded context domain types per ADR-0058.
//!
//! Provides value objects for TCP/UDP socket enumeration and TCP protocol
//! statistics. All types are platform-agnostic; adapter crates supply OS-specific
//! implementations (macOS sysctl / Linux netlink+procnet).
//!
//! # Module layout
//!
//! - [`socket`] — `Protocol`, `AddrFamily`, `TcpState`, `SocketEntry`.
//! - [`stats`] — `TcpStats`, `ConnectionCounts`.
//! - [`request`] — request and result envelopes (`NetworkTcpListRequest`, etc.).

pub mod request;
pub mod socket;
pub mod stats;

pub use request::{
    NetworkTcpListRequest, NetworkTcpListResult, NetworkUdpListRequest, NetworkUdpListResult,
};
pub use socket::{AddrFamily, Protocol, SocketEntry, TcpState};
pub use stats::{ConnectionCounts, TcpStats};

// Re-export Pagination for ergonomic imports from this module.
pub use crate::subprocess::pagination::Pagination;
