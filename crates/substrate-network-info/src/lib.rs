//! Network socket introspection adapter per ADR-0058.
//!
//! Two concrete platform implementations:
//!
//! - [`MacosSysctlAdapter`](macos::MacosSysctlAdapter) — macOS via
//!   `sysctlbyname("net.inet.tcp.pcblist_n", ...)`.
//! - [`LinuxProcNetAdapter`](linux::LinuxProcNetAdapter) — Linux via
//!   `/proc/net/{tcp,tcp6,udp,udp6}` + `/proc/net/snmp`.
//!
//! The composition root selects the right adapter via [`NetworkInfoFactory::build`].
//! Linux `NETLINK_INET_DIAG` is a planned v1.1 upgrade — the procnet parser
//! provides v1 coverage of all four [`NetworkInfoPort`] methods.
//!
//! [`NetworkInfoPort`]: substrate_domain::ports::network_info::NetworkInfoPort

#![warn(missing_docs)]
// This crate wraps raw OS syscalls on macOS; unsafe blocks are required by
// design for sysctl / struct-cast FFI. Each unsafe block carries a SAFETY
// comment justifying the invariants upheld (ADR-0042, ADR-0044, ADR-0058).
#![allow(unsafe_code, reason = "macOS sysctl + struct-cast FFI per ADR-0058/ADR-0042/ADR-0044")]

pub mod factory;
pub mod state;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

pub use factory::{NetworkInfoFactory, NetworkInfoTier};
