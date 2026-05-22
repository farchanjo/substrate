//! Async job control-plane domain types per ADR-0040.

pub mod bucket;
pub mod config;
pub mod entry;
pub mod progress;
pub mod state;

pub use bucket::JobBucket;
pub use config::{JobConfig, JobInlineThresholds, JobQuotas, JobTimeouts};
pub use entry::JobEntry;
pub use progress::ProgressEvent;
pub use state::JobState;
