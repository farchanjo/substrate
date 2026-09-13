// DDD role: ValueObject
package schemas

// Terminal-output schema for the subprocess bounded context: the result payload,
// the streaming chunk carried by notifications/progress, and the pagination
// value objects both share. Per ADR-0054 and ADR-0057.

// #Base64BlobOrEmpty is a base64 payload that may legitimately be the empty
// string, i.e. the empty-allowing counterpart of #Base64Blob. It is used for
// capture aggregates, which are empty — not absent — when no bytes were captured.
#Base64BlobOrEmpty: string & != "" | *""

// #SubprocessResult is the terminal output returned by the subprocess.result tool
// call once a job has reached a terminal state. It combines the ring-buffer
// aggregate with optional disk-persistence paths (TmpFile capture branch).
// Returned by SubprocessPort::result; never mutated.
//
// Cross-reference: ADR-0054 amendment 2026-05-24 — TmpFile capture branch.
#SubprocessResult: {
	// exit_code is the process exit status. Present only when terminal_state
	// is Succeeded or Failed. Absent for Cancelled, Killed, and TimedOut
	// (POSIX does not guarantee a meaningful exit code after SIGKILL, and
	// Cancelled jobs may not have exited before cancellation was processed).
	exit_code?: #ExitCode

	// duration_ms is the elapsed wall-clock time from child process start
	// (Running state entry) to terminal state entry, in milliseconds.
	duration_ms: int & >=0

	// terminal_state is the final lifecycle state of the child process.
	// Always a terminal value; SubprocessResult is never returned for
	// non-terminal jobs.
	terminal_state: #SubprocessState

	// Ring-buffer aggregates and tmp-file spill paths for both channels.
	#ResultCapture

	// Decoded stdout lines, present only when pagination was requested.
	#StdoutPage

	// Decoded stderr lines, present only when pagination was requested.
	#StderrPage
}

// #ResultCapture is the capture half of #SubprocessResult: the base64 aggregates
// and the final tmp-file paths. Embedded by #SubprocessResult.
#ResultCapture: {
	// stdout_aggregate_base64 contains the base64-encoded last 64 KiB of
	// stdout from the ring buffer. Empty string when no bytes were captured.
	stdout_aggregate_base64: #Base64BlobOrEmpty

	// stderr_aggregate_base64 contains the base64-encoded last 64 KiB of
	// stderr from the ring buffer. Empty string when no bytes were captured.
	stderr_aggregate_base64: #Base64BlobOrEmpty

	// stdout_tmp_path is the absolute path to the final (post-rename) stdout
	// capture file. Present only when capture_kind == "tmp_file" AND
	// terminal_state == Succeeded. Absent in all other cases, including when
	// the job is still Running or when the terminal state is not Succeeded.
	stdout_tmp_path?: #AbsolutePath

	// stderr_tmp_path is the absolute path to the final (post-rename) stderr
	// capture file. Present only when capture_kind == "tmp_file" AND
	// terminal_state == Succeeded. Absent in all other cases.
	stderr_tmp_path?: #AbsolutePath

	// stream_chunks_dropped is the cumulative count of stdout and stderr
	// chunks dropped due to bounded mpsc channel backpressure since the job
	// was created. A non-zero value indicates the aggregate may be incomplete
	// even when stdout_tmp_path / stderr_tmp_path are present (dropped chunks
	// are still written to the tmp file; only the live notification was lost).
	stream_chunks_dropped: int & >=0
}

// #StdoutPage is the decoded-stdout half of a paginated #SubprocessResult.
// Absent fields mean pagination was not requested. Embedded by #SubprocessResult.
#StdoutPage: {
	// stdout_lines, when pagination was requested, contains the decoded UTF-8 lines
	// for the current page of stdout output. Absent when pagination was not requested.
	// Per ADR-0057.
	stdout_lines?: #LineList

	// stdout_total_lines is the total number of lines in the captured stdout ring buffer.
	// Present only when pagination was requested. Per ADR-0057.
	stdout_total_lines?: int & >=0

	// stdout_next_offset is the pagination offset for the next stdout page.
	// Absent when this is the last (or only) page. Per ADR-0057.
	stdout_next_offset?: int & >=0
}

// #StderrPage is the decoded-stderr half of a paginated #SubprocessResult.
// Absent fields mean pagination was not requested. Embedded by #SubprocessResult.
#StderrPage: {
	// stderr_lines, when pagination was requested, contains the decoded UTF-8 lines
	// for the current page of stderr output. Absent when pagination was not requested.
	// Per ADR-0057.
	stderr_lines?: #LineList

	// stderr_total_lines is the total number of lines in the captured stderr ring buffer.
	// Present only when pagination was requested. Per ADR-0057.
	stderr_total_lines?: int & >=0

	// stderr_next_offset is the pagination offset for the next stderr page.
	// Absent when this is the last (or only) page. Per ADR-0057.
	stderr_next_offset?: int & >=0
}

// #Stream identifies a standard I/O channel of a child process.
// Used by #StreamChunk, #SubprocessSearchRequest, and #SearchMatch.
#Stream: "stdout" | "stderr"

// #StreamChunk is the value object carried in each notifications/progress event
// for subprocess stdout and stderr output per ADR-0054. Chunks are numbered
// per-stream (not globally) and include a byte offset for reassembly.
#StreamChunk: {
	// job_id (Crockford base32, 26 chars) correlates the chunk with its originating
	// SubprocessHandle. Aliases #JobId per ADR-0040 triple-equality.
	job_id: #JobId

	// stream identifies whether the chunk originates from standard output or
	// standard error of the child process.
	stream: #Stream

	// seq is the zero-based monotonic sequence number for this stream.
	// Gaps in seq indicate dropped chunks; seq never resets within a job.
	seq: int & >=0

	// chunk_base64 is the raw bytes of the chunk encoded as base64 standard
	// encoding (RFC 4648 §4). Clients decode before interpretation.
	chunk_base64: #Base64Blob

	// chunk_bytes is the decoded byte count of chunk_base64. Cap: 4096 (4 KiB)
	// per ADR-0054 §"Tokio Task Architecture". Clients use this for backpressure
	// hints and to validate chunk_base64 decoding.
	chunk_bytes: int & >=0 & <=4096

	// byte_offset is the cumulative byte offset of the first byte in this chunk
	// relative to the beginning of the stream, allowing ordered reassembly even
	// when events arrive out of order.
	byte_offset: int & >=0

	// timestamp is the RFC 3339 timestamp at which the chunk was read from the
	// OS pipe into the substrate capture buffer.
	timestamp: #Timestamp
}

// #Order controls the traversal direction for paginated subprocess output per ADR-0057.
// Tail (default) returns lines from the most-recent end; Head returns from the oldest end.
#Order: "Tail" | "Head"

// #Pagination describes a single page of line-oriented subprocess output per ADR-0057.
// Pagination is optional on subprocess.result and subprocess.search; absent means
// the caller receives the full ring-buffer aggregate without line decomposition.
#Pagination: {
	// offset is the 0-based line offset from which to start the page.
	// For order=Tail offset 0 = most-recent line; for order=Head offset 0 = oldest line.
	offset: int & >=0

	// page_size is the maximum number of lines to return in this page.
	// Reuses the shared #PageSize domain bounds (1..=10000 per ADR-0060) but
	// overrides the default to 100 — the PageSize::DEFAULT_PAGINATION constant used
	// by line- and record-oriented tools (subprocess.result/subprocess.search).
	page_size: #PageSize | *100

	// order controls traversal direction. Default Tail (most-recent-first).
	order: #Order | *"Tail"
}

// #SubprocessResultRequest is the value object submitted to subprocess.result
// to retrieve the terminal output of a completed job per ADR-0057.
// When pagination is absent the full ring-buffer aggregate (base64 blobs) is returned.
// When pagination is set the line-decomposed fields (stdout_lines, stderr_lines, etc.) are
// populated in #SubprocessResult and the aggregate blobs are omitted.
#SubprocessResultRequest: {
	// job_id identifies the target job (Crockford base32, 26 chars; aliases #JobId).
	job_id: #JobId

	// pagination, when present, enables line-based paged retrieval of captured output.
	// Absent preserves original full-aggregate behavior.
	pagination?: #Pagination
}