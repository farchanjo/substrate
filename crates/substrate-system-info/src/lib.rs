//! `substrate-system-info` — system-info BC adapter.
//!
//! Exposes six MCP tools mapping to the `system-info` bounded context:
//! `sys.uname`, `sys.hostname`, `sys.uptime`, `sys.df`,
//! `sys.load_average`, and `sys.info`.
//!
//! # Async zone classification (ADR-0003)
//!
//! All six tools are Zone A (sync inline). None of the underlying syscalls
//! block for meaningful time: `uname(2)`, `gethostname(2)`, `statvfs(2)`,
//! and `/proc/uptime` are single-page reads. `spawn_blocking` is NOT used.
//!
//! # No-subprocess invariant (ADR-0044)
//!
//! This crate MUST NOT call `std::process::Command`, `tokio::process::Command`,
//! or any equivalent. `uname`, `hostname`, `uptime`, `df` capabilities are
//! provided exclusively by `nix`, `procfs` (Linux), and `libc` sysctl
//! bindings (macOS).

// unsafe_code is deny at workspace level. The sole exceptions in this crate
// are the macOS sysctl / FFI wrappers in df.rs, load_average.rs, and
// uptime.rs — each of which carries a module-level `#![allow(unsafe_code)]`
// citing ADR-0042 + ADR-0044. We use `deny` (not `forbid`) here so that
// per-module allows are valid; `forbid` cannot be downgraded by sub-items.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod df;
pub mod hints_helpers;
pub mod hostname;
pub mod info;
pub mod load_average;
pub mod response;
pub mod uname;
pub mod uptime;

// ---- Re-exports for the composition root ------------------------------------

pub use df::handle_sys_df;
pub use hostname::handle_sys_hostname;
pub use info::handle_sys_info;
pub use load_average::handle_sys_load_average;
pub use response::{SystemInfoDeps, ToolResponse};
pub use uname::handle_sys_uname;
pub use uptime::handle_sys_uptime;
