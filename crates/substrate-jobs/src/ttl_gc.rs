//! Background garbage-collection task for expired job slots per ADR-0040.
//!
//! Wakes every `gc_interval_secs` and evicts terminal `JobEntry` records whose
//! `terminal_at` timestamp is older than `result_ttl_secs`. After eviction,
//! `job.status` and `job.result` return `SUBSTRATE_JOB_NOT_FOUND` for the
//! evicted ID.
//!
//! The GC loop also purges matching entries from the idempotency deduplication
//! index to prevent stale keys from blocking resubmission of expired jobs.
//!
//! The task is cancel-safe: it checks the [`CancellationToken`] at the start of
//! each sleep phase. On cancellation the loop exits cleanly with no partial state.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument};

use substrate_domain::value_objects::JobId;

use crate::entry_state::JobSlot;
use crate::registry::IdempotencyDedupKey;

/// Monotonically incrementing counter of GC evictions across the process lifetime.
///
/// Exposed for integration tests and observability; never reset during the process.
pub(crate) static EVICTION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Spawns a background GC loop that evicts expired terminal job slots.
///
/// # Parameters
/// - `jobs`: shared reference to the registry's job map.
/// - `idempotency_index`: shared reference to the deduplication index.
/// - `result_ttl_secs`: job result retention in seconds after terminal state entry.
/// - `gc_interval_secs`: sleep duration between GC sweeps in seconds.
/// - `cancel`: root cancellation token; loop exits when cancelled.
#[instrument(skip_all, fields(ttl_secs = result_ttl_secs, interval_secs = gc_interval_secs))]
pub(crate) async fn gc_loop(
    jobs: Arc<DashMap<JobId, Arc<JobSlot>>>,
    idempotency_index: Arc<DashMap<IdempotencyDedupKey, JobId>>,
    result_ttl_secs: u64,
    gc_interval_secs: u64,
    cancel: CancellationToken,
) {
    let interval = tokio::time::Duration::from_secs(gc_interval_secs);
    info!("gc_loop started");

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                info!("gc_loop stopping: cancellation token triggered");
                break;
            }
            () = tokio::time::sleep(interval) => {}
        }

        let evicted = sweep_once(&jobs, &idempotency_index, result_ttl_secs);
        if evicted > 0 {
            EVICTION_COUNTER.fetch_add(evicted, Ordering::Relaxed);
            debug!(evicted, "gc sweep evicted terminal jobs");
        }
    }
}

/// Performs one GC sweep, returning the number of evicted entries.
///
/// Separated from `gc_loop` to allow unit testing without spawning async tasks.
pub(crate) fn sweep_once(
    jobs: &DashMap<JobId, Arc<JobSlot>>,
    idempotency_index: &DashMap<IdempotencyDedupKey, JobId>,
    result_ttl_secs: u64,
) -> u64 {
    let now = OffsetDateTime::now_utc();
    // u64 -> i64: TTL values are small config values well within i64 range.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "result_ttl_secs is a small config value; wrapping at 9.2e18 seconds is impossible"
    )]
    let ttl = time::Duration::seconds(result_ttl_secs as i64);

    let mut expired_ids: Vec<JobId> = Vec::new();

    // Collect IDs to evict without holding DashMap shard locks during the loop.
    for slot_ref in jobs {
        let entry = slot_ref.value().entry.lock();
        if let Some(terminal_at) = entry.terminal_at
            && now - terminal_at > ttl
        {
            expired_ids.push(entry.id.clone());
        }
    }

    let evicted = expired_ids.len() as u64;

    for id in &expired_ids {
        jobs.remove(id);
    }

    // Purge matching idempotency index entries to allow resubmission.
    // We retain entries whose job_id is NOT in the evicted set.
    idempotency_index.retain(|_key, job_id| !expired_ids.contains(job_id));

    evicted
}
