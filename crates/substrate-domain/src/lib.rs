//! `substrate-domain` — pure-domain shared kernel.
//!
//! Zero infra dependencies. Ports defined here are implemented by adapter
//! crates and wired by `substrate-mcp-server` (composition root).
//!
//! # Module layout
//!
//! - [`capabilities`] — runtime capability snapshot and tier enumerations (ADR-0042).
//! - [`errors`] — canonical error taxonomy with stable `SUBSTRATE_*` codes (ADR-0010).
//! - [`hints`] — structured response hints map (ADR-0007 + ADR-0040 extension).
//! - [`jobs`] — async job control-plane value objects (ADR-0040).
//! - [`ports`] — inbound port traits implemented by adapter crates.
//! - [`network`] — network-info BC domain types: socket, stats, request/result (ADR-0058).
//! - [`subprocess`] — subprocess BC domain types: request, handle, state, stream, errors (ADR-0052).
//! - [`value_objects`] — shared value objects used across bounded contexts.

#![cfg_attr(not(test), forbid(unsafe_code))]
#![warn(missing_docs)]

pub mod capabilities;
pub mod errors;
pub mod hints;
pub mod jobs;
pub mod network;
pub mod ports;
pub mod subprocess;
pub mod value_objects;

// ---- Flat re-exports from value_objects ------------------------------------

pub use value_objects::client_id::ClientId;
pub use value_objects::correlation_id::CorrelationId;
pub use value_objects::idempotency_key::IdempotencyKey;
pub use value_objects::jailed_path::JailedPath;
pub use value_objects::job_id::JobId;
pub use value_objects::page_cursor::PageCursor;
pub use value_objects::process_group::ProcessGroup;
pub use value_objects::subprocess_id::SubprocessId;

// ---- Flat re-exports from errors -------------------------------------------

pub use errors::{SubstrateError, SubstrateResult};

// ---- Flat re-exports from jobs ---------------------------------------------

pub use jobs::bucket::JobBucket;
pub use jobs::state::JobState;

// ---- Flat re-exports from capabilities -------------------------------------

pub use capabilities::{
    Capabilities, CapabilityOverride, HashTier, JailTier, SimdTier, StatTier, WalkerTier,
    WatcherTier,
};

// ---- Flat re-exports from hints --------------------------------------------

pub use hints::Hints;

// ---- Flat re-exports from subprocess ---------------------------------------

pub use subprocess::errors::SubprocessError;
pub use subprocess::handle::SubprocessHandle;
pub use subprocess::request::{CaptureKind, StdinKind, SubprocessRequest};
pub use subprocess::state::SubprocessState;
pub use subprocess::stream::{Stream, StreamChunk};

// ---- Flat re-exports from ports --------------------------------------------

pub use ports::dir_walker::DirWalkerPort;
pub use ports::factory::PortFactory;
pub use ports::fs_index::FsIndexPort;
pub use ports::fs_watcher::FsWatcherPort;
pub use ports::hash::HashPort;
pub use ports::job_registry::JobRegistryPort;
pub use ports::path_jail::PathJailPort;
pub use ports::stat::StatPort;
pub use ports::subprocess::SubprocessPort;
