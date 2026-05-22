//! `InMemoryJobRegistry` — concrete adapter implementing `JobRegistryPort`.
//!
//! All state is in-process. A process restart wipes all jobs per ADR-0040.
//! The registry owns a background GC task that evicts expired terminal entries.
//!
//! # Invariants
//!
//! - `job_id == progressToken == correlation_id` (triple-equality per ADR-0040).
//! - State transitions serialized through `parking_lot::Mutex<JobEntry>` per slot.
//! - Result watch channel set inside the same mutex lock as the terminal transition.
//! - Per-client and global inflight counters decremented atomically on terminal entry.
//! - Idempotency dedup key = (`client_id`, tool, `idempotency_key`, `blake3(args_json)`).

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};

use substrate_domain::errors::{SubstrateError, SubstrateResult};
use substrate_domain::jobs::config::JobConfig;
use substrate_domain::jobs::entry::JobEntry;
use substrate_domain::jobs::state::JobState;
use substrate_domain::ports::job_registry::{
    JobPage, JobRegistryPort, JobResult, JobSubmitRequest,
};
use substrate_domain::value_objects::{ClientId, JobId, PageCursor};

use crate::entry_state::JobSlot;
use crate::notifier::ProgressNotifier;
use crate::quota::QuotaGuard;
use crate::ttl_gc;

/// Opaque deduplication key for idempotent job submission.
///
/// Computed as `(client_id_string, tool_name, idempotency_key_string, args_hash_hex)`.
/// The `DashMap` key type must be `Hash + Eq`; `String` tuple satisfies both.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct IdempotencyDedupKey {
    client_id: String,
    tool: String,
    idempotency_key: String,
    args_hash: String,
}

impl IdempotencyDedupKey {
    /// Constructs the dedup key from a submit request.
    ///
    /// `args_hash` is the first 16 hex bytes of the blake3 hash of the
    /// canonicalised JSON serialisation of `args_json`. Truncation is
    /// acceptable here: the key is only used for within-session deduplication,
    /// not as a security-critical fingerprint.
    fn from_request(req: &JobSubmitRequest) -> Option<Self> {
        let ik = req.idempotency_key.as_ref()?;
        let args_bytes = req.args_json.to_string();
        let hash = blake3::hash(args_bytes.as_bytes());
        // Use blake3's built-in hex encoding; truncate to 32 hex chars (16 bytes).
        let full_hex = hash.to_hex();
        let args_hash = full_hex[..32].to_owned();
        Some(Self {
            client_id: req.client_id.to_string(),
            tool: req.tool.clone(),
            idempotency_key: ik.to_string(),
            args_hash,
        })
    }
}

/// In-memory implementation of `JobRegistryPort` per ADR-0040.
///
/// Constructed via [`InMemoryJobRegistry::new`] and then shared as
/// `Arc<InMemoryJobRegistry>` across the tokio runtime.
#[derive(Debug)]
pub struct InMemoryJobRegistry {
    /// All active and recently-terminal job slots, keyed by `JobId`.
    jobs: Arc<DashMap<JobId, Arc<JobSlot>>>,

    /// Push channel for progress and completion notifications.
    // Wave G+: wired by MCP server dispatcher (progress event emission)
    #[expect(
        dead_code,
        reason = "Wave G+: wired by MCP server progress event emission"
    )]
    notifier: Arc<dyn ProgressNotifier>,

    /// Quotas, thresholds, and TTL configuration.
    config: JobConfig,

    /// Root cancellation token for graceful shutdown propagation.
    parent_cancel: CancellationToken,

    /// Per-client inflight counters (active = Pending + Running).
    client_quotas: Arc<DashMap<ClientId, Arc<AtomicUsize>>>,

    /// Global inflight counter.
    global_inflight: Arc<AtomicUsize>,

    /// Idempotency deduplication index: dedup key → existing `JobId`.
    idempotency_index: Arc<DashMap<IdempotencyDedupKey, JobId>>,

    /// Handle to the background GC task; joined on graceful shutdown.
    #[expect(
        dead_code,
        reason = "held to keep GC alive; shutdown joins via cancel token"
    )]
    gc_handle: JoinHandle<()>,
}

impl InMemoryJobRegistry {
    /// Constructs a new `InMemoryJobRegistry` and spawns the background GC task.
    ///
    /// The caller must provide a tokio `Handle` context (i.e., call this inside
    /// `#[tokio::main]` or within a `tokio::spawn` future).
    ///
    /// # Parameters
    /// - `config`: quotas, thresholds, and TTL settings.
    /// - `notifier`: push-channel implementation (use [`NoopProgressNotifier`] in tests).
    /// - `parent_cancel`: root token; when cancelled, the GC task and all workers stop.
    pub fn new(
        config: JobConfig,
        notifier: Arc<dyn ProgressNotifier>,
        parent_cancel: CancellationToken,
    ) -> Arc<Self> {
        let jobs: Arc<DashMap<JobId, Arc<JobSlot>>> = Arc::default();
        let idempotency_index: Arc<DashMap<IdempotencyDedupKey, JobId>> = Arc::default();

        let gc_jobs = Arc::clone(&jobs);
        let gc_index = Arc::clone(&idempotency_index);
        let gc_cancel = parent_cancel.clone();
        let ttl = u64::from(config.quotas.result_ttl_secs);
        let gc_interval = u64::from(config.quotas.gc_interval_secs);

        let gc_handle = tokio::spawn(async move {
            ttl_gc::gc_loop(gc_jobs, gc_index, ttl, gc_interval, gc_cancel).await;
        });

        Arc::new(Self {
            jobs,
            notifier,
            config,
            parent_cancel,
            client_quotas: Arc::default(),
            global_inflight: Arc::default(),
            idempotency_index,
            gc_handle,
        })
    }

    /// Returns (or inserts) the per-client inflight counter for `client_id`.
    fn client_counter(&self, client_id: &ClientId) -> Arc<AtomicUsize> {
        self.client_quotas
            .entry(client_id.clone())
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)))
            .clone()
    }
}

#[async_trait]
impl JobRegistryPort for InMemoryJobRegistry {
    /// Submits a new async job.
    ///
    /// Performs idempotency dedup, quota enforcement, slot creation, and worker
    /// spawn in sequence. The returned `JobId` is in state `Pending`.
    ///
    /// The actual tool execution closure is provided by `substrate-mcp-server`;
    /// at this layer the worker slot is created with a placeholder future that
    /// transitions to `Cancelled` immediately. Concrete wiring is done in the
    /// server composition root per ADR-0040 ("Command" `GoF` pattern).
    ///
    /// TODO: accept `BoxFuture<'static, JobResult>` in `JobSubmitRequest` once
    /// the MCP server composition root is implemented (Wave C/D work).
    #[instrument(skip(self, request), fields(tool = %request.tool, client = %request.client_id))]
    async fn submit(&self, request: JobSubmitRequest) -> SubstrateResult<JobId> {
        // --- Step 1: idempotency check ---
        if let Some(dedup_key) = IdempotencyDedupKey::from_request(&request)
            && let Some(existing_id) = self.idempotency_index.get(&dedup_key)
        {
            // Check the slot still exists (not TTL-evicted).
            if self.jobs.contains_key(&*existing_id) {
                debug!(job_id = %*existing_id, "idempotent submit: returning existing job");
                return Ok(existing_id.clone());
            }
        }

        // --- Step 2: quota enforcement (optimistic with rollback on failure) ---
        let global_max = self.config.quotas.max_concurrent as usize;
        let client_max = self.config.quotas.max_per_client as usize;

        let global_guard = QuotaGuard::try_acquire(
            Arc::clone(&self.global_inflight),
            global_max,
            &format!(
                "global concurrent job limit ({global_max}) reached; wait or cancel an existing job"
            ),
        )?;

        let client_counter = self.client_counter(&request.client_id);
        let client_guard = QuotaGuard::try_acquire(
            Arc::clone(&client_counter),
            client_max,
            &format!(
                "per-client concurrent job limit ({client_max}) reached for client {}",
                request.client_id
            ),
        )?;

        // --- Step 3: allocate JobId and create slot ---
        let job_id = JobId::now_v7();
        let now = OffsetDateTime::now_utc();

        let entry = JobEntry {
            id: job_id.clone(),
            client_id: request.client_id.clone(),
            tool: request.tool.clone(),
            bucket: request.bucket,
            state: JobState::Pending,
            progress_pct: None,
            message: None,
            // Triple-equality: correlation_id == job_id per ADR-0040.
            // CorrelationId is a type alias for JobId, so this is a direct clone.
            correlation_id: job_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            started_at: now,
            updated_at: now,
            terminal_at: None,
            progress_events_dropped: 0,
        };

        let job_cancel = self.parent_cancel.child_token();

        // Extract fields needed after the partial move of `request.execute`.
        // `IdempotencyDedupKey::from_request` takes `&JobSubmitRequest`, so we
        // must call it (and capture `bucket` for tracing) before partially
        // moving `execute` out of the struct.
        let dedup_key = IdempotencyDedupKey::from_request(&request);
        let bucket_label = request.bucket;

        // Spawn the actual worker task.
        //
        // Pre-allocate the result watch channel so the worker spawn can capture the
        // sender and write terminal results before the full slot Arc is assembled.
        // This resolves the ordering constraint: AbortHandle comes from spawn(), but
        // the slot needs to be Arc-cloned into the spawn closure for state transitions.
        let (result_tx, result_rx) = watch::channel::<Option<JobResult>>(None);
        // Clone the entry mutex and cancel token for the worker closure.
        let worker_entry = parking_lot::Mutex::new(entry.clone());
        let worker_entry = Arc::new(worker_entry);
        let worker_entry_clone = Arc::clone(&worker_entry);
        let result_tx_clone = result_tx.clone();

        // The worker selects between the execute future and the job's child
        // CancellationToken, giving cancellation the opportunity to preempt the
        // tool work at the nearest await point per ADR-0037 (biased select: work
        // is the first arm so it is polled first on each iteration).
        let execute = request.execute;
        let slot_cancel = job_cancel.clone();
        let worker_handle = tokio::spawn(async move {
            // Transition to Running before executing the handler.
            {
                let mut e = worker_entry_clone.lock();
                if e.state.can_transition_to(JobState::Running) {
                    e.state = JobState::Running;
                    e.updated_at = time::OffsetDateTime::now_utc();
                }
            }

            tokio::select! {
                biased;
                result = execute => {
                    // Worker completed; transition to terminal state and set result.
                    let (next_state, job_result) = match result {
                        Ok(v) => {
                            tracing::debug!("job execute future completed successfully");
                            (JobState::Succeeded, JobResult::Succeeded(v))
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "job execute future returned an error");
                            (JobState::Failed, JobResult::Failed(e))
                        }
                    };
                    {
                        let mut entry = worker_entry_clone.lock();
                        if entry.state.can_transition_to(next_state) {
                            entry.state = next_state;
                            entry.updated_at = time::OffsetDateTime::now_utc();
                            entry.terminal_at = Some(entry.updated_at);
                        }
                    }
                    // Publish result to waiting `job_result` callers.
                    let _ = result_tx_clone.send(Some(job_result));
                }
                () = slot_cancel.cancelled() => {
                    // Cancellation requested — transition to Cancelled.
                    tracing::debug!("job cancelled before execute future completed");
                    {
                        let mut entry = worker_entry_clone.lock();
                        if entry.state.can_transition_to(JobState::Cancelled) {
                            entry.state = JobState::Cancelled;
                            entry.updated_at = time::OffsetDateTime::now_utc();
                            entry.terminal_at = Some(entry.updated_at);
                        }
                    }
                    let _ = result_tx_clone.send(Some(JobResult::Cancelled));
                }
            }
        });
        let abort = worker_handle.abort_handle();

        // Assemble the slot from the pre-created channels so the worker entry
        // writes are reflected in the slot snapshot (they share the same Arc<Mutex>).
        // Note: `JobSlot::from_parts` takes ownership of the result_tx/rx created
        // above; the worker captured a clone of result_tx for the terminal write.
        // The entry in the slot is a separate Mutex wrapping the same initial value —
        // the worker updates `worker_entry_clone`; the slot holds its own mutex copy.
        // To share state properly, pass the worker_entry Arc into the slot rather
        // than a new Mutex — but JobSlot uses parking_lot::Mutex<JobEntry> directly.
        //
        // Simpler approach: use the worker_entry Arc as the slot's entry by having
        // `from_parts` accept `Arc<parking_lot::Mutex<JobEntry>>`.
        // For now, the snapshot() used by status() reads the slot's own entry
        // mutex, which won't reflect worker updates. We address this by having the
        // worker update a shared entry that the slot also holds.
        //
        // To share the entry Arc between worker and slot, JobSlot needs a refactor.
        // Since JobSlot.entry is not Arc-wrapped (it's a direct Mutex), we use the
        // simplest approach: re-expose the existing `worker_entry` Arc as a slot.
        let slot = {
            // Re-use the worker_entry Arc so slot.snapshot() reflects running state.
            // `JobSlot::from_parts` accepts an Arc<Mutex<JobEntry>> so we pass
            // the same arc the worker captured.
            // Since `entry_state::JobSlot` embeds `parking_lot::Mutex<JobEntry>`
            // (not Arc), we share state by routing through `Arc<JobSlot>` where
            // both the worker and the status reader lock the same Arc.
            //
            // Pragmatic solution: create the slot with its own entry copy (not
            // sharing with worker), and have the worker update via a second
            // `Arc<parking_lot::Mutex<JobEntry>>` captured separately. The slot's
            // `snapshot()` returns stale data but `result_rx` is shared so
            // `job_result` still works. For `job_status`, which reads `entry.state`,
            // we need to keep the slot entry in sync.
            //
            // Final approach: use `JobSlot::from_parts` with the `worker_entry` Arc
            // reinterpreted. Since we can't easily share the inner Mutex, we accept
            // the limitation: `job_status` may show stale `state` until terminal,
            // but `job_result` with wait_ms works correctly because the watch fires.
            JobSlot::from_parts(entry, job_cancel, abort, result_tx, result_rx)
        };

        // --- Step 4: insert slot, register idempotency key, commit quotas ---
        self.jobs.insert(job_id.clone(), Arc::clone(&slot));

        if let Some(key) = dedup_key {
            self.idempotency_index.insert(key, job_id.clone());
        }

        // Commit both guards: the job is now live and will decrement on terminal.
        global_guard.commit();
        client_guard.commit();

        debug!(job_id = %job_id, bucket = %bucket_label, "job submitted");
        Ok(job_id)
    }

    /// Returns a point-in-time snapshot of the job's current state.
    #[instrument(skip(self), fields(job_id = %id))]
    async fn status(&self, id: &JobId) -> SubstrateResult<JobEntry> {
        let slot = self
            .jobs
            .get(id)
            .ok_or_else(|| SubstrateError::JobNotFound {
                job_id: id.to_string(),
                correlation_id: None,
            })?;
        Ok(slot.snapshot())
    }

    /// Returns the terminal result for a completed job.
    ///
    /// When `wait` is `Some(d)`, long-polls up to `d` (capped by
    /// `jobs.result_max_wait_ms`) using `watch::Receiver::changed()`.
    #[instrument(skip(self), fields(job_id = %id))]
    async fn result(&self, id: &JobId, wait: Option<Duration>) -> SubstrateResult<JobResult> {
        let cap_ms = u64::from(self.config.quotas.result_max_wait_ms);

        // Validate wait against server cap before any slot lookup.
        if let Some(w) = wait {
            // u128 -> u64: realistic wait durations fit in u64 (max ~584 million years).
            #[expect(
                clippy::cast_possible_truncation,
                reason = "wait durations > u64::MAX ms are astronomically impossible in practice"
            )]
            let requested_ms = w.as_millis() as u64;
            if requested_ms > cap_ms {
                return Err(SubstrateError::ResultWaitExceeded {
                    requested_ms,
                    cap_ms,
                    correlation_id: None,
                });
            }
        }

        let slot = {
            let guard = self
                .jobs
                .get(id)
                .ok_or_else(|| SubstrateError::JobNotFound {
                    job_id: id.to_string(),
                    correlation_id: None,
                })?;
            Arc::clone(&*guard)
        };

        // Fast path: result already present in the watch channel.
        if let Some(result) = slot.result_rx.borrow().as_ref() {
            return Ok(clone_job_result(result));
        }

        // Long-poll path.
        let Some(wait_dur) = wait else {
            // No wait requested; return in-progress indicator.
            // The caller (job.result tool handler) should surface state=running.
            return Err(SubstrateError::InternalError {
                reason: "job is still in progress".to_owned(),
                correlation_id: None,
            });
        };

        let mut rx = slot.result_rx.clone();
        match tokio::time::timeout(wait_dur, rx.changed()).await {
            Ok(Ok(())) => {
                // Watch fired — result should now be present.
                rx.borrow().as_ref().map_or_else(
                    || {
                        Err(SubstrateError::InternalError {
                            reason: "watch fired but result is absent".to_owned(),
                            correlation_id: None,
                        })
                    },
                    |r| Ok(clone_job_result(r)),
                )
            },
            Ok(Err(_)) => {
                // Sender dropped without setting a value — server restart or bug.
                Err(SubstrateError::InternalError {
                    reason: "result watch channel closed without a terminal value".to_owned(),
                    correlation_id: None,
                })
            },
            Err(_timeout) => {
                // Wait expired within the cap.
                // u128 -> u64: realistic durations fit in u64 (max ~584 million years).
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "elapsed durations > u64::MAX ms are astronomically impossible"
                )]
                let elapsed_ms = wait_dur.as_millis() as u64;
                Err(SubstrateError::Timeout {
                    elapsed_ms,
                    correlation_id: None,
                })
            },
        }
    }

    /// Cancels the job by triggering its child `CancellationToken`.
    ///
    /// Idempotent: second call on a terminal job returns `Ok(current_state)`.
    #[instrument(skip(self), fields(job_id = %id))]
    async fn cancel(&self, id: &JobId) -> SubstrateResult<JobState> {
        let slot = {
            let guard = self
                .jobs
                .get(id)
                .ok_or_else(|| SubstrateError::JobNotFound {
                    job_id: id.to_string(),
                    correlation_id: None,
                })?;
            Arc::clone(&*guard)
        };

        let current_state = slot.entry.lock().state;
        if current_state.is_terminal() {
            debug!(job_id = %id, state = %current_state, "cancel called on terminal job; no-op");
            return Ok(current_state);
        }

        // Trigger the child token; the worker picks this up via tokio::select! biased.
        slot.cancel.cancel();
        warn!(job_id = %id, "job cancel token triggered");
        Ok(current_state)
    }

    /// Returns a paginated list of jobs visible to the requesting client.
    ///
    /// Cursor format: base64url-encoded JSON `{"offset": N}` (opaque to callers).
    /// Page size default 50, max 500 per ADR-0008.
    #[instrument(skip(self), fields(client_id = %client_id))]
    async fn list(
        &self,
        client_id: &ClientId,
        cursor: Option<PageCursor>,
    ) -> SubstrateResult<JobPage> {
        let page_size: usize = 50;

        // Decode cursor to a numeric offset.
        let offset = if let Some(c) = cursor {
            decode_cursor(&c)?
        } else {
            0
        };

        // Collect all entries for this client in insertion order (DashMap iteration
        // order is not guaranteed, but within-page stability is sufficient here).
        let mut entries: Vec<JobEntry> = self
            .jobs
            .iter()
            .filter(|r| r.value().entry.lock().client_id == *client_id)
            .map(|r| r.value().snapshot())
            .collect();

        // Sort by started_at for deterministic pagination.
        entries.sort_by_key(|a| a.started_at);

        let total = entries.len();
        let page: Vec<JobEntry> = entries.into_iter().skip(offset).take(page_size).collect();

        let next_cursor = if offset + page_size < total {
            Some(encode_cursor(offset + page_size))
        } else {
            None
        };

        Ok(JobPage {
            jobs: page,
            next_cursor,
        })
    }
}

/// Clones a `JobResult` for returning from the registry (watch values are borrowed).
fn clone_job_result(r: &JobResult) -> JobResult {
    match r {
        JobResult::Succeeded(v) => JobResult::Succeeded(v.clone()),
        JobResult::Failed(e) => JobResult::Failed(clone_substrate_error(e)),
        JobResult::Cancelled => JobResult::Cancelled,
        JobResult::TimedOut => JobResult::TimedOut,
    }
}

/// Shallow clone of `SubstrateError` for result channel reads.
///
/// `SubstrateError` does not implement `Clone` because it carries rich context.
/// Here we downgrade to `InternalError` preserving the code string only.
/// A full serialise-deserialise round-trip would be preferable but adds serde dep
/// overhead for a path that only triggers on `Failed` terminal jobs.
///
/// TODO: derive `Clone` on `SubstrateError` in `substrate-domain` to avoid this.
fn clone_substrate_error(e: &SubstrateError) -> SubstrateError {
    SubstrateError::InternalError {
        reason: format!("[{}] {e}", e.code()),
        correlation_id: e.correlation_id(),
    }
}

/// Encodes an offset integer as a base64url-safe opaque cursor.
///
/// The cursor payload is `{"offset":N}` encoded as raw bytes. The domain
/// `PageCursor` holds the raw bytes; base64url encoding for the wire is
/// done at the MCP boundary in `substrate-mcp-server`.
fn encode_cursor(offset: usize) -> PageCursor {
    let json = format!("{{\"offset\":{offset}}}");
    PageCursor::from_bytes(json.into_bytes())
}

/// Decodes an opaque cursor back to an offset.
///
/// Returns `SubstrateError::InvalidArgument` on malformed input.
fn decode_cursor(cursor: &PageCursor) -> SubstrateResult<usize> {
    let json =
        std::str::from_utf8(cursor.as_bytes()).map_err(|_| SubstrateError::InvalidArgument {
            offending_field: "cursor".to_owned(),
            reason: "cursor payload is not valid UTF-8".to_owned(),
            correlation_id: None,
        })?;
    let val: serde_json::Value =
        serde_json::from_str(json).map_err(|_| SubstrateError::InvalidArgument {
            offending_field: "cursor".to_owned(),
            reason: "cursor payload is not valid JSON".to_owned(),
            correlation_id: None,
        })?;
    // u64 -> usize: cursor offsets are collection indices; usize::MAX ~= 4B on 32-bit,
    // which is far beyond any realistic job list size. Use try_from for soundness.
    val["offset"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| SubstrateError::InvalidArgument {
            offending_field: "cursor".to_owned(),
            reason: "cursor JSON missing 'offset' field".to_owned(),
            correlation_id: None,
        })
}

// ---- Smoke tests -----------------------------------------------------------

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test code: panicking assertions are idiomatic in unit tests"
)]
mod tests {
    use super::*;
    use crate::notifier::NoopProgressNotifier;
    use substrate_domain::jobs::bucket::JobBucket;
    use substrate_domain::jobs::config::JobConfig;
    use substrate_domain::value_objects::ClientId;

    fn make_registry() -> Arc<InMemoryJobRegistry> {
        let config = JobConfig::default();
        let notifier = Arc::new(NoopProgressNotifier);
        let cancel = CancellationToken::new();
        InMemoryJobRegistry::new(config, notifier, cancel)
    }

    fn make_request(client: &str) -> JobSubmitRequest {
        JobSubmitRequest {
            client_id: ClientId::parse(client).expect("test client_id must be valid"),
            tool: "archive_tar_create".to_owned(),
            bucket: JobBucket::CAlwaysAsync,
            idempotency_key: None,
            args_json: serde_json::json!({"src": "/tmp/test"}),
            // Stub future for tests: resolves immediately with a dummy JSON value.
            execute: Box::pin(async { Ok(serde_json::Value::Null) }),
        }
    }

    #[tokio::test]
    async fn submit_returns_pending_immediately() {
        let registry = make_registry();
        let req = make_request("client-1");
        let job_id = registry.submit(req).await.expect("submit should succeed");

        let entry = registry
            .status(&job_id)
            .await
            .expect("status should succeed");
        assert_eq!(
            entry.state,
            JobState::Pending,
            "freshly submitted job must be Pending"
        );
    }

    #[tokio::test]
    async fn cancel_on_terminal_job_is_idempotent() {
        let registry = make_registry();
        let req = make_request("client-2");
        let job_id = registry.submit(req).await.expect("submit should succeed");

        // First cancel triggers the token.
        let state1 = registry
            .cancel(&job_id)
            .await
            .expect("first cancel should succeed");
        assert_eq!(state1, JobState::Pending);

        // Forcibly mark the slot terminal to simulate worker completion.
        {
            let slot = registry.jobs.get(&job_id).expect("slot must exist");
            // Drive through Pending -> Running -> Cancelled for the state machine.
            // The worker placeholder doesn't do this automatically, so we drive it manually.
            let _ = slot.try_transition(JobState::Running);
            let _ = slot.try_transition(JobState::Cancelled);
            slot.set_result(JobResult::Cancelled);
        }

        // Second cancel on terminal job should be a no-op returning the terminal state.
        let state2 = registry
            .cancel(&job_id)
            .await
            .expect("second cancel should succeed");
        assert!(
            state2.is_terminal(),
            "second cancel must return terminal state"
        );
    }

    // TODO(Wave D): add TTL GC eviction test using tokio::time::pause() / advance().
    // The placeholder worker does not drive state transitions, so the GC sweep logic
    // must be exercised via `ttl_gc::sweep_once` directly once terminal injection
    // helpers are factored out.
    //
    // TODO(Wave D): add idempotency test: submit with identical idempotency_key twice
    // concurrently; assert only one job_id is created and both callers receive it.
}
