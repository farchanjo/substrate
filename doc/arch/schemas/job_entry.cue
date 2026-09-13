// DDD role: ValueObject
package schemas

// The async-job registry entry and the two cohesive field groups it embeds.

// #JobOwnership is the provenance half of a #JobEntry: which client submitted the
// job, its correlation handle, and the optional deduplication token. Embedded by
// #JobEntry.
#JobOwnership: {
	// client_id identifies the submitting MCP client.
	client_id: #ClientId

	// correlation_id equals id per ADR-0040 triple-equality invariant.
	correlation_id: #CorrelationId

	// idempotency_key is the client-supplied deduplication token; optional.
	idempotency_key?: #IdempotencyKey
}

// #JobTimestamps is the transition-time half of a #JobEntry. Embedded by #JobEntry.
#JobTimestamps: {
	// started_at is the RFC 3339 timestamp when the job transitioned to running.
	started_at: string & =~"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$"

	// updated_at is the RFC 3339 timestamp of the most recent state transition.
	updated_at: string & =~"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$"

	// terminal_at is the RFC 3339 timestamp when the job entered a terminal state.
	// Absent while the job is in pending or running state.
	terminal_at?: string & =~"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$"
}

// #JobEntry is the in-memory aggregate root snapshot stored in the JobRegistry.
// State transitions are serialized through a parking_lot::Mutex<JobState> per ADR-0040.
// Terminal states never regress; invalid transitions are silently ignored.
// DDD role: AggregateRoot
#JobEntry: {
	// id is the canonical UUIDv7 job identifier, equal to progressToken and correlation_id.
	id: #JobId

	// tool is the fully-qualified MCP tool name including the job_ namespace for
	// control-plane tools, the subprocess_ namespace per ADR-0052, the net_
	// namespace per ADR-0058, and the launch_ namespace per ADR-0069. Wire form
	// uses underscores per ADR-0062.
	tool: string & =~"^(fs|proc|sys|text|archive|job|subprocess|net|launch)_[a-z][a-z0-9_]*$"

	// bucket is the static dispatch bucket assigned to this tool per ADR-0040.
	bucket: #JobBucket

	// state is the current position in the job state machine.
	state: #JobState

	// progress_pct is the last-known completion percentage emitted by the worker.
	// Absent for jobs that have not yet emitted a progress event.
	progress_pct?: int & >=0 & <=100

	// message is the last human-readable status note from the worker; max 120 chars.
	message?: string & =~"^.{0,120}$"

	// progress_events_dropped counts events lost due to bounded mpsc channel
	// backpressure per ADR-0040. An AuditEvent is emitted for each drop.
	progress_events_dropped: int & >=0

	// Submitting client, correlation handle, and deduplication token.
	#JobOwnership

	// Lifecycle transition timestamps.
	#JobTimestamps
}