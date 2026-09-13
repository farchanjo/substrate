// DDD role: ValueObject
package schemas

// Push-channel payload of the async job control-plane: the progress event and its
// optional subprocess stream extension. Per ADR-0040 and ADR-0054.

// #ProgressStreamExtension carries the optional stdout/stderr chunk a #ProgressEvent
// transports per ADR-0054, plus the terminal sentinel marker. When ANY of these
// fields is set, ALL of stream/chunk_base64/chunk_bytes/chunk_seq/byte_offset MUST
// be set. job_state is optional and present only on the terminal sentinel event
// emitted just before subprocess.result becomes callable. Embedded by #ProgressEvent.
#ProgressStreamExtension: {
	// stream identifies whether the chunk originates from standard output or
	// standard error of the child process.
	stream?: "stdout" | "stderr"

	// chunk_base64 is the raw chunk bytes encoded as base64 (RFC 4648 §4).
	chunk_base64?: #Base64Blob

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

// #ProgressEvent is the push-channel payload emitted via MCP 2025-11-25
// notifications/progress. Events are throttled: suppressed unless 250 ms have
// elapsed since last emission OR progress delta >= 1 percentage point per ADR-0040.
// sequence_number is sourced from a per-job AtomicU64 for dropped-event detection.
// DDD role: ValueObject
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

	// Optional subprocess stdout/stderr chunk and terminal sentinel marker.
	#ProgressStreamExtension
}