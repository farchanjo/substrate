//! Domain value objects shared across bounded contexts.

pub mod client_id;
pub mod correlation_id;
pub mod idempotency_key;
pub mod jailed_path;
pub mod job_id;
pub mod page_cursor;
pub mod process_group;
pub mod subprocess_id;

pub use client_id::ClientId;
pub use correlation_id::CorrelationId;
pub use idempotency_key::IdempotencyKey;
pub use jailed_path::JailedPath;
pub use job_id::JobId;
pub use page_cursor::PageCursor;
pub use process_group::ProcessGroup;
pub use subprocess_id::SubprocessId;
