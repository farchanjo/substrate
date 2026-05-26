//! Subprocess bounded context — domain types for the subprocess BC.
//!
//! This module contains the pure-domain value objects and errors for the
//! subprocess bounded context introduced in ADR-0052. No OS primitives or
//! infra dependencies are permitted here; those live in `substrate-subprocess`.
//!
//! # Module layout
//!
//! - [`errors`] — `SubprocessError` enum with stable `SUBSTRATE_*` codes.
//! - [`handle`] — `SubprocessHandle` aggregate root.
//! - [`request`] — `SubprocessRequest` value object with validation.
//! - [`state`] — `SubprocessState` lifecycle enum.
//! - [`stream`] — `StreamChunk` and `Stream` for stdout/stderr chunks.
//! - [`supervisor`] — `RestartPolicy`, `HealthProbe`, `LogRotation` value objects (ADR-0056).
//! - [`pagination`] — `Pagination`, `Order`, `SubprocessSearchRequest`, `SubprocessSearchResult`,
//!   `SearchMatch` value objects (ADR-0057).
//!
//! References: ADR-0052, ADR-0053, ADR-0054, ADR-0056, ADR-0057.

pub mod errors;
pub mod handle;
pub mod pagination;
pub mod request;
pub mod state;
pub mod stream;
pub mod supervisor;

pub use errors::SubprocessError;
pub use handle::SubprocessHandle;
pub use pagination::{
    Order, Pagination, SearchMatch, SubprocessSearchRequest, SubprocessSearchResult,
};
pub use request::{CaptureKind, StdinKind, SubprocessRequest};
pub use state::SubprocessState;
pub use stream::{Stream, StreamChunk};
pub use supervisor::{HealthProbe, LogRotation, RestartPolicy};
