// DDD role: AggregateRoot
package schemas

// #JobId is a UUIDv7 encoded in base32 Crockford form (26 uppercase chars).
// It doubles as the MCP progressToken and the correlation_id per ADR-0040.
#JobId: string & =~"^[0-9A-HJKMNP-TV-Z]{26}$"

// #CorrelationId is an alias of #JobId.
// The triple equality (job_id == progressToken == correlation_id) eliminates
// any mapping table between MCP protocol tokens and internal identifiers per ADR-0040.
#CorrelationId: #JobId

// #JobProgressToken is the MCP progressToken value for a job submission.
// It equals the #JobId per ADR-0040 triple-equality invariant.
// Named #JobProgressToken to avoid collision with #ProgressToken in shared_kernel.cue,
// which models the incremental-progress tracking token used by streaming tools.
#JobProgressToken: #JobId

// #IdempotencyKey is a client-generated UUIDv7 (base32 Crockford, 26 chars).
// Deduplication key: (client_id, tool_name, idempotency_key, blake3_hash_of_args_json)
// per ADR-0040. Bounded to result_ttl_secs and evicted by the same GC.
#IdempotencyKey: string & =~"^[0-9A-HJKMNP-TV-Z]{26}$"

// #ClientId identifies the MCP client submitting a job.
// Cross-client visibility is forbidden; each client sees only its own jobs per ADR-0040.
#ClientId: string & =~"^[A-Za-z0-9._-]{1,64}$"

// #JobState enumerates all valid states of the async job state machine per ADR-0040.
// Terminal states (succeeded, failed, cancelled, timed_out) never regress.
#JobState: "pending" | "running" | "succeeded" | "failed" | "cancelled" | "timed_out"

// #PollingEndpoint names the control-plane tools used to poll a job per ADR-0040.
#PollingEndpoint: "job.status" | "job.result"

// #JobBucket classifies every MCP tool into a dispatch bucket per ADR-0040.
// A: sync inline (snapshot-instant). B: auto-mode (inline if small, job if large).
// C: always async (job mandatory). D: sync side-effect (commit fast, audit async).
#JobBucket: "A_sync_inline" | "B_auto_mode" | "C_always_async" | "D_sync_side_effect"

// #ProgressEvent is the push-channel payload emitted via MCP 2025-11-25
// notifications/progress. Events are throttled: suppressed unless 250 ms have
// elapsed since last emission OR progress delta >= 1 percentage point per ADR-0040.
// sequence_number is sourced from a per-job AtomicU64 for dropped-event detection.
#ProgressEvent: {
	// progress_token equals the job_id and the MCP progressToken per ADR-0040.
	progress_token: #JobProgressToken

	// progress is the completion percentage (0 to 100 inclusive).
	progress: int & >=0 & <=100

	// total is the denominator for the progress percentage; defaults to 100.
	total: int & >=0 | *100

	// message is an optional human-readable status note; max 120 chars.
	message?: string & =~"^.{0,120}$"

	// sequence_number is a monotonically increasing per-job counter.
	// Clients MUST use this field to detect dropped or reordered events.
	sequence_number: int & >=0

	// emitted_at is the RFC 3339 timestamp at which this event was constructed.
	emitted_at: string & =~"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$"

	// Stream-extension fields (optional, present only when this ProgressEvent
	// carries a subprocess stdout/stderr chunk per ADR-0054). When ANY of these
	// fields is set, ALL of stream/chunk_base64/chunk_bytes/chunk_seq/byte_offset
	// MUST be set. job_state is optional and present only on the terminal
	// sentinel event emitted just before subprocess.result becomes callable.

	// stream identifies whether the chunk originates from standard output or
	// standard error of the child process.
	stream?: "stdout" | "stderr"

	// chunk_base64 is the raw chunk bytes encoded as base64 (RFC 4648 §4).
	chunk_base64?: string

	// chunk_bytes is the decoded byte count of chunk_base64. Cap: 4096 (4 KiB).
	chunk_bytes?: int & >=0 & <=4096

	// chunk_seq is the per-stream zero-based monotonic sequence number.
	// Distinct from sequence_number which is per-job. Gaps indicate dropped chunks.
	chunk_seq?: int & >=0

	// byte_offset is the cumulative byte offset of the first byte in this chunk
	// relative to the beginning of the stream.
	byte_offset?: int & >=0

	// job_state is set ONLY on the terminal sentinel event (job_state = Succeeded
	// | Failed | TimedOut | Cancelled). Signals that the dispatcher task has
	// flushed all pending chunks and subprocess.result is now callable.
	job_state?: #SubprocessState
}

// #JobEntry is the in-memory aggregate root snapshot stored in the JobRegistry.
// State transitions are serialized through a parking_lot::Mutex<JobState> per ADR-0040.
// Terminal states never regress; invalid transitions are silently ignored.
#JobEntry: {
	// id is the canonical UUIDv7 job identifier, equal to progressToken and correlation_id.
	id: #JobId

	// client_id identifies the submitting MCP client.
	client_id: #ClientId

	// tool is the fully-qualified MCP tool name including the job_ namespace for
	// control-plane tools and the subprocess_ namespace per ADR-0052.
	tool: string & =~"^(fs|proc|sys|text|archive|job|subprocess)_[a-z][a-z0-9_]*$"

	// bucket is the static dispatch bucket assigned to this tool per ADR-0040.
	bucket: #JobBucket

	// state is the current position in the job state machine.
	state: #JobState

	// progress_pct is the last-known completion percentage emitted by the worker.
	// Absent for jobs that have not yet emitted a progress event.
	progress_pct?: int & >=0 & <=100

	// message is the last human-readable status note from the worker; max 120 chars.
	message?: string & =~"^.{0,120}$"

	// correlation_id equals id per ADR-0040 triple-equality invariant.
	correlation_id: #CorrelationId

	// idempotency_key is the client-supplied deduplication token; optional.
	idempotency_key?: #IdempotencyKey

	// started_at is the RFC 3339 timestamp when the job transitioned to running.
	started_at: string & =~"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$"

	// updated_at is the RFC 3339 timestamp of the most recent state transition.
	updated_at: string & =~"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$"

	// terminal_at is the RFC 3339 timestamp when the job entered a terminal state.
	// Absent while the job is in pending or running state.
	terminal_at?: string & =~"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$"

	// progress_events_dropped counts events lost due to bounded mpsc channel
	// backpressure per ADR-0040. An AuditEvent is emitted for each drop.
	progress_events_dropped: int & >=0
}

// #JobQuotas configures the resource limits for the async job control-plane per ADR-0040.
// All fields have safe defaults; operators may override via TOML [jobs] section.
#JobQuotas: {
	// max_concurrent is the global limit on active (pending + running) jobs.
	max_concurrent: int & >=1 | *16

	// max_per_client is the per-client active job limit.
	max_per_client: int & >=1 | *4

	// result_ttl_secs is the retention period after terminal state entry.
	// After eviction, job.result and job.status return SUBSTRATE_JOB_NOT_FOUND.
	result_ttl_secs: int & >=1 | *300

	// result_max_wait_ms caps the wait_ms parameter of job.result (long-poll ceiling).
	result_max_wait_ms: int & >=0 | *30000

	// result_default_wait_ms is the wait_ms substituted by the handler when the
	// caller omits the field per ADR-0059. Must satisfy 0 < default <= result_max_wait_ms.
	// An explicit wait_ms=0 in the request payload is honored as before; only the
	// "field absent" case is substituted by this default.
	result_default_wait_ms: int & >0 & <=result_max_wait_ms | *5000

	// progress_interval_ms is the minimum emission interval between progress events.
	// Events are also suppressed unless progress delta >= 1 percentage point.
	progress_interval_ms: int & >=10 | *250

	// progress_channel_size is the bounded mpsc channel capacity per job.
	// Events submitted via try_send when full are dropped and counted.
	progress_channel_size: int & >=1 | *64

	// gc_interval_secs is the background GC wake interval for evicting expired jobs.
	gc_interval_secs: int & >=1 | *60
}

// #JobInlineThresholds declares per-tool size thresholds for Bucket B auto-mode.
// A tool invocation below its threshold returns an inline result; at or above the
// threshold the tool is promoted to an async job per ADR-0040.
// Open struct: additional tool thresholds may be added without a schema amendment.
#JobInlineThresholds: {
	// fs_find_inline_entries: inline if the candidate count is below this value.
	fs_find_inline_entries: int & >=0 | *1000

	// fs_read_inline_bytes: inline if the file byte size is below this value.
	fs_read_inline_bytes: int & >=0 | *1048576

	// fs_hash_inline_bytes: inline if the input byte size is below this value.
	fs_hash_inline_bytes: int & >=0 | *4194304

	// fs_copy_inline_bytes: inline if the source file size is below this value.
	fs_copy_inline_bytes: int & >=0 | *1048576

	// text_search_inline_bytes: inline if the file byte size is below this value.
	text_search_inline_bytes: int & >=0 | *524288

	// text_count_lines_inline_bytes: inline if the file byte size is below this value.
	text_count_lines_inline_bytes: int & >=0 | *524288

	// archive_gzip_inline_bytes: inline if the uncompressed byte size is below this value.
	archive_gzip_inline_bytes: int & >=0 | *131072

	// archive_hash_inline_bytes: inline if the archive byte size is below this value.
	archive_hash_inline_bytes: int & >=0 | *4194304

	// Open: additional per-tool thresholds may be declared here without breaking existing configs.
	...
}

// #JobTimeouts configures per-tool execution time limits for async jobs per ADR-0040.
// Per-tool entries override the default. All values are in seconds.
#JobTimeouts: {
	// default_secs applies when no per-tool override is present.
	default_secs: int & >=1 | *600

	// archive_create_secs caps archive.tar.create and archive.zip.create jobs.
	archive_create_secs: int & >=1 | *1800

	// archive_extract_secs caps archive.tar.extract and archive.zip.extract jobs.
	archive_extract_secs: int & >=1 | *1800

	// fs_find_secs caps fs.find jobs promoted to Bucket C.
	fs_find_secs: int & >=1 | *60

	// fs_hash_secs caps fs.hash jobs in Bucket B or C.
	fs_hash_secs: int & >=1 | *600
}

// #JobConfig is the top-level configuration aggregate for the async job control-plane.
// It is embedded in the main RuntimeConfig under the [jobs] TOML section per ADR-0040.
#JobConfig: {
	// quotas configures resource limits (concurrency, TTL, channel sizes).
	quotas: #JobQuotas

	// inline_thresholds declares per-tool size thresholds for Bucket B auto-mode.
	inline_thresholds: #JobInlineThresholds

	// timeouts configures per-tool execution time limits.
	timeouts: #JobTimeouts
}
