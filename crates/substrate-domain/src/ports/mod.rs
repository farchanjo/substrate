//! Inbound port traits for the substrate domain per ADR-0022 hexagonal architecture.
//!
//! Adapter crates (`substrate-jobs`, `substrate-policy`, `substrate-fs-query`, etc.)
//! implement these traits. Only `substrate-mcp-server` (composition root) may
//! depend on adapter crates; domain code depends exclusively on these traits.

pub mod dir_walker;
pub mod factory;
pub mod fs_index;
pub mod fs_watcher;
pub mod hash;
pub mod job_registry;
pub mod path_jail;
pub mod stat;

pub use dir_walker::{DirEntry, DirWalkerPort, WalkOpts};
pub use factory::PortFactory;
pub use fs_index::{CancelSignal, FsIndexPort, IndexQuery};
pub use fs_watcher::{FsWatcherPort, WatchEvent, WatchGuard};
pub use hash::{Blake3Digest, HashPort};
pub use job_registry::{JobPage, JobRegistryPort, JobResult, JobSubmitRequest};
pub use path_jail::PathJailPort;
pub use stat::{FileStat, StatPort};
