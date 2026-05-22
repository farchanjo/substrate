//! `substrate-process` — process BC adapter.
//!
//! Exposes three MCP tools that map to the `process` bounded context:
//! `proc.list`, `proc.tree`, and `proc.signal`.
//!
//! # Async zone classification (ADR-0003)
//!
//! | Tool          | Zone | Mechanism                                  |
//! |---------------|------|--------------------------------------------|
//! | `proc.list`   | B    | `spawn_blocking` + platform scanner        |
//! | `proc.tree`   | B    | `spawn_blocking` + adjacency build         |
//! | `proc.signal` | A    | async-native; `kill(2)` is non-blocking    |
//!
//! # No-subprocess invariant (ADR-0044)
//!
//! This crate observes processes only. It MUST NOT call `std::process::Command`,
//! `tokio::process::Command`, or any equivalent subprocess API. Signal delivery
//! uses `nix::sys::signal::kill` exclusively.

// unsafe_code is deny at workspace level. The sole exception in this crate
// is scanner/macos.rs which calls sysctl(KERN_PROC_ALL) to enumerate
// processes via `kinfo_proc` — a narrow carve-out per ADR-0042 + ADR-0044.
// We use `deny` (not `forbid`) here so that the per-module allow is valid.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod hints_helpers;
pub mod list;
pub mod process_info;
pub mod scanner;
pub mod signal;
pub mod signal_policy;
pub mod tree;

// ---- Re-exports for the composition root -----------------------------------

pub use list::handle_proc_list;
pub use process_info::ProcessInfo;
pub use response::{ProcessDeps, ToolResponse};
pub use scanner::{ProcessScannerPort, default_scanner};
pub use signal::handle_proc_signal;
pub use tree::handle_proc_tree;

mod response;
