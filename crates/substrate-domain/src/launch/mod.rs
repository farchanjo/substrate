//! Launch bounded context — domain types for declarative process orchestration.
//!
//! This module contains the pure-domain value objects and errors for the launch
//! bounded context introduced in ADR-0063..0069. No OS primitives or infra
//! dependencies are permitted here; those live in `substrate-launch`. All process
//! spawning is delegated to the subprocess BC via its port, so this module never
//! references `tokio::process` or any OS fork primitive.
//!
//! # Module layout
//!
//! - [`errors`] — `LaunchError` enum with stable `SUBSTRATE_LAUNCH_*` codes.
//! - [`event`] — `LaunchEvent` and `LaunchEventKind` for the per-Stack event-log.
//! - [`profile`] — `LaunchProfile`, `LaunchService`, DAG ordering, and config value objects.
//! - [`stack`] — `StackHandle` aggregate, `StackChild`, `SupervisorRegistry`.
//! - [`state`] — `StackState` lifecycle enum and `DisconnectPolicy`.
//! - [`trust`] — `TrustRecord` TOFU trust-store entry.
//!
//! References: ADR-0063, ADR-0064, ADR-0065, ADR-0066, ADR-0067, ADR-0068.

pub mod errors;
pub mod event;
pub mod profile;
pub mod stack;
pub mod state;
pub mod trust;

pub use errors::LaunchError;
pub use event::{LaunchEvent, LaunchEventKind};
pub use profile::{
    CommandSpec, DependencyRestartMode, LaunchChannelBounds, LaunchOperatorConfig, LaunchProfile,
    LaunchService, ServiceName, StreamMux,
};
pub use stack::{StackChild, StackHandle, SupervisorRegistry};
pub use state::{DisconnectPolicy, StackState};
pub use trust::TrustRecord;
