//! `substrate-launch` — launch orchestration BC adapter (ADR-0063..0069).
//!
//! Implements [`substrate_domain::ports::launch::LaunchPort`] on
//! [`LaunchRegistry`]. All process spawning is delegated to the injected
//! [`substrate_domain::ports::subprocess::SubprocessPort`]; this crate
//! **never** calls `tokio::process::Command` directly.
//!
//! The `launch` Cargo feature on `substrate-mcp-server` gates this crate.
//! The default server build and the `subprocess`-only build are byte-identical
//! without that feature.
//!
//! # Module layout
//!
//! - [`registry`] — [`LaunchRegistry`]: concrete [`LaunchPort`] adapter.
//! - [`trust_store`] — TOFU trust-store I/O (Phase 3).
//! - [`profile_loader`] — safe-open profile loading with TOFU gate (Phase 3).
//! - [`dag`] — topological order helpers and restart closure (Phase 3).
//! - [`redaction`] — line-level secret redaction applied before event log (Phase 3).
//! - [`supervisor`] — in-process bring-up/teardown orchestration (Phase 4).
//! - [`supervisor_registry`] — durable per-Stack supervisor registry persistence
//!   (Milestone 2, ADR-0068).
//! - [`control_fifo`] — detached supervisor control-FIFO IPC: framing,
//!   permission boundary, reader/writer (Milestone 2, ADR-0068).
//! - [`detached`] — the `substrate --supervise` reactor: bring-up with
//!   parent-death binding, control-FIFO + signal + poll multiplexing, teardown
//!   (Milestone 2, ADR-0068).
//! - [`pid_probe`] — cross-platform single-pid start-time + ppid probe backing
//!   the PID-recycle guard (Milestone 2, ADR-0068).
//! - [`reaper`] — the reaper-on-boot adopt-or-reap reconcile pass over the
//!   durable registry (Milestone 2, ADR-0068).
//!
//! References: ADR-0063, ADR-0064, ADR-0065, ADR-0066, ADR-0067, ADR-0068.

pub mod control_fifo;
pub mod dag;
pub mod detached;
pub mod pid_probe;
pub mod profile_loader;
pub mod reaper;
pub mod redaction;
pub mod registry;
pub mod supervisor;
pub mod supervisor_registry;
pub mod trust_store;

pub use registry::LaunchRegistry;
