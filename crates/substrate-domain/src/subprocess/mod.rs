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
//!
//! References: ADR-0052, ADR-0053, ADR-0054.

pub mod errors;
pub mod handle;
pub mod request;
pub mod state;
pub mod stream;

pub use errors::SubprocessError;
pub use handle::SubprocessHandle;
pub use request::{CaptureKind, StdinKind, SubprocessRequest};
pub use state::SubprocessState;
pub use stream::{Stream, StreamChunk};
