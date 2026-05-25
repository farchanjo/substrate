//! `substrate-subprocess` — Subprocess BC adapter per ADR-0052.
//!
//! This crate is the **single permitted host** of `tokio::process::Command`
//! in the `substrate` workspace. The no-subprocess Rego policy
//! (`policies/subprocess_invariants.rego`) whitelists only `crates/substrate-subprocess/`.
//!
//! # Hexagonal layering
//!
//! This crate depends on:
//! - `substrate-domain` (port traits + value objects)
//! - `substrate-policy` (allowlist enforcement + `PathJail`)
//! - `substrate-jobs` (`InMemoryJobRegistry` API)
//!
//! It MUST NOT depend on `substrate-mcp-server` or any other adapter crate.
//! Only `substrate-mcp-server` depends on this crate (via the `subprocess` Cargo feature).
//!
//! # Architecture
//!
//! ADR-0052 supersedes ADR-0044. ADR-0044 imposed a blanket prohibition on
//! subprocess invocation; ADR-0052 narrows that rule to all crates outside this one.
//! ADR-0053 specifies the cascade kill contract. ADR-0054 specifies the
//! stdout/stderr stream multiplex. ADR-0055 specifies the orphan reaper on startup.
//!
//! # Module layout
//!
//! - [`pre_exec`] — async-signal-safe pre-exec hook (setsid + prctl/watchdog).
//! - [`watchdog`] — macOS watchdog pipe pattern per ADR-0053.
//! - [`spawn`] — supervised child spawn producing a [`spawn::ChildHandle`].
//! - [`stream_capture`] — reader tasks for stdout/stderr mpsc multiplex.
//! - [`cascade`] — cascade kill chain per ADR-0053.
//! - [`cleanup`] — explicit tmp-file cleanup per ADR-0033/ADR-0014.
//! - [`registry`] — [`SubprocessRegistry`]: the [`SubprocessPort`] implementation.
//!
//! # References
//!
//! ADR-0052, ADR-0053, ADR-0054, ADR-0055, ADR-0033, ADR-0037, ADR-0032.

#![warn(missing_docs)]

pub mod cascade;
pub mod cleanup;
pub mod health_probe;
pub mod orphan_reaper;
pub mod pre_exec;
pub mod registry;
pub mod spawn;
pub mod stream_capture;
pub mod tmp_file;
pub mod watchdog;

pub use health_probe::{ProbeOutcome, run_probe};
pub use orphan_reaper::{ReaperStats, run_once as run_orphan_reaper_once};
pub use registry::{SubprocessRegistry, paginate_lines};

use substrate_domain::ports::subprocess::SubprocessPort;
/// Alias confirming that [`SubprocessRegistry`] implements the inbound port.
///
/// The composition root in `substrate-mcp-server` stores an `Arc<dyn SubprocessPort>`
/// when the `subprocess` Cargo feature is active.
pub type DynSubprocessPort = dyn SubprocessPort;
