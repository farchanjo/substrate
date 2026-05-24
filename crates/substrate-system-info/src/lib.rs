//! `substrate-system-info` — system-info BC adapter.
//!
//! Exposes eight MCP tools mapping to the `system-info` bounded context:
//! `sys.uname`, `sys.hostname`, `sys.uptime`, `sys.df`,
//! `sys.load_average`, `sys.info`, `sys.mem`, and `sys.cpu`.
//!
//! # Async zone classification (ADR-0003)
//!
//! | Tool              | Zone | Notes                                             |
//! |-------------------|------|---------------------------------------------------|
//! | `sys.uname`       | A    | `uname(2)` — single syscall                       |
//! | `sys.hostname`    | A    | `gethostname(2)` — single syscall                 |
//! | `sys.uptime`      | A    | `/proc/uptime` or `sysctl KERN_BOOTTIME`          |
//! | `sys.df`          | A    | `statvfs(2)` — single syscall                     |
//! | `sys.load_average`| A    | `getloadavg(3)` or `sysinfo(2)`                   |
//! | `sys.info`        | A    | composite of the above                            |
//! | `sys.mem`         | B    | `spawn_blocking`; `/proc/meminfo` or sysctl/mach  |
//! | `sys.cpu`         | B    | `spawn_blocking`; `/proc/stat` or mach `host_processor_info`  |
//!
//! # No-subprocess invariant (ADR-0044)
//!
//! This crate MUST NOT call `std::process::Command`, `tokio::process::Command`,
//! or any equivalent. `uname`, `hostname`, `uptime`, `df`, `mem`, and `cpu`
//! capabilities are provided exclusively by `nix`, `procfs` (Linux), and `libc`
//! sysctl / mach bindings (macOS).

// unsafe_code is deny at workspace level. The sole exceptions in this crate
// are the macOS sysctl / FFI wrappers in df.rs, load_average.rs, and
// uptime.rs — each of which carries a module-level `#![allow(unsafe_code)]`
// citing ADR-0042 + ADR-0044. We use `deny` (not `forbid`) here so that
// per-module allows are valid; `forbid` cannot be downgraded by sub-items.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod cpu;
pub mod df;
pub mod hints_helpers;
pub mod hostname;
pub mod info;
pub mod load_average;
pub mod mem;
pub mod response;
pub mod uname;
pub mod uptime;

// ---- Re-exports for the composition root ------------------------------------

pub use cpu::{CpuStats, SharedCpuState, handle_sys_cpu, new_cpu_state};
pub use df::handle_sys_df;
pub use hostname::handle_sys_hostname;
pub use info::handle_sys_info;
pub use load_average::handle_sys_load_average;
pub use mem::{MemorySnapshot, handle_sys_mem};
pub use response::{SystemInfoDeps, ToolResponse};
pub use uname::handle_sys_uname;
pub use uptime::handle_sys_uptime;
