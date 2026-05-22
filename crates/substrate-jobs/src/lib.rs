//! `substrate-jobs` — `InMemoryJobRegistry` adapter per ADR-0040.
//!
//! This crate provides the in-process async job control-plane. It implements
//! `substrate_domain::ports::job_registry::JobRegistryPort` using a
//! [`DashMap`](dashmap::DashMap) of [`JobSlot`](crate::entry_state::JobSlot)
//! values guarded by per-slot `parking_lot::Mutex` for state transitions.
//!
//! # Architecture
//!
//! ```text
//! InMemoryJobRegistry
//!   ├── jobs: Arc<DashMap<JobId, Arc<JobSlot>>>
//!   ├── notifier: Arc<dyn ProgressNotifier>     (push channel)
//!   ├── config: JobConfig                        (quotas + TTL)
//!   ├── parent_cancel: CancellationToken         (root shutdown token)
//!   ├── client_quotas: Arc<DashMap<ClientId, AtomicUsize>>
//!   ├── global_inflight: Arc<AtomicUsize>
//!   ├── idempotency_index: Arc<DashMap<IdempotencyDedupKey, JobId>>
//!   └── gc_handle: JoinHandle<()>
//! ```
//!
//! Progress events travel through a bounded `mpsc` channel (capacity 64 by default)
//! to a [`ProgressNotifier`] implementation. Slow consumers lose events; dropped counts
//! are recorded per ADR-0040. The result pull-channel uses
//! `tokio::sync::watch::Receiver<Option<JobResult>>` for zero-poll last-value reads.

#![cfg_attr(not(test), forbid(unsafe_code))]
#![warn(missing_docs)]
// Private modules expose `pub(crate)` items; clippy considers this redundant
// because the enclosing module is already private. The annotation is kept for
// clarity: it signals intent (crate-internal) rather than being a no-op.
#![expect(
    clippy::redundant_pub_crate,
    reason = "private modules use pub(crate) to signal crate-internal intent explicitly"
)]

mod cancel;
mod entry_state;
mod notifier;
mod quota;
mod registry;
mod throttle;
mod ttl_gc;

pub use notifier::{NoopProgressNotifier, ProgressNotifier};
pub use registry::InMemoryJobRegistry;
