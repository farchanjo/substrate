// DDD role: AggregateRoot
//
// CUE schema for the subprocess bounded context.
//
// Cross-references:
//   ADR-0052 — subprocess bounded context decision
//   ADR-0053 — process lifecycle cascade contract (setsid, PR_SET_PDEATHSIG, watchdog pipe)
//   ADR-0054 — subprocess stdout/stderr stream multiplex via notifications/progress
//   ADR-0057 — subprocess output pagination and search
//
// Dependency on shared kernel: #JobId (job.cue) for the job_id field.
package schemas

// #SubprocessState enumerates the lifecycle states of a spawned child process.
// Terminal states (Succeeded, Failed, Cancelled, Killed, TimedOut) never regress.
// Mirrors JobState from job.cue but with subprocess-specific terminal distinctions.
#SubprocessState: "Pending" | "Starting" | "Running" | "Ready" | "Restarting" | "Cancelled" | "Killed" | "Succeeded" | "Failed" | "TimedOut"

// #SubprocessRequest is the value object submitted by an MCP client to launch a
// child process. All fields are validated by the subprocess_invariants Rego policy
// before any OS call is made.
#SubprocessRequest: {
	// binary_path is the absolute path to the executable to spawn.
	// MUST be an absolute path (begins with /). Validated against
	// security.subprocess_binary_allowlist before spawning.
	binary_path: string & !=""

	// args is the argument list passed to the binary; argv[0] is binary_path.
	args: [...string]

	// env_allowlist contains the names (not values) of environment variables
	// from the substrate process environment that may be inherited by the child.
	// Values are always inherited from substrate's own environment; this field
	// controls which names are visible. LD_PRELOAD and related injection vectors
	// are unconditionally banned regardless of this list.
	env_allowlist: [...string]

	// env_override provides explicit key=value overrides in the child environment.
	// Every key in env_override is subject to the same banned-variable list as
	// env_allowlist: LD_PRELOAD, DYLD_INSERT_LIBRARIES, LD_LIBRARY_PATH,
	// and DYLD_LIBRARY_PATH are unconditionally rejected.
	env_override: [string]: string

	// cwd is the working directory for the child process, validated by PathJail.
	// MUST be an absolute path (begins with /).
	cwd: string & !=""

	// stdin_kind describes how the child process receives standard input.
	// "none" closes stdin, "piped" allows the caller to stream bytes in,
	// "file_path" reads from a pre-existing file given in stdin_file_path.
	stdin_kind: "none" | "piped" | "file_path"

	// stdin_file_path is required when stdin_kind is "file_path"; absent otherwise.
	stdin_file_path?: string

	// capture_kind controls how stdout and stderr are captured.
	// "stream" emits chunks via notifications/progress (ADR-0054).
	// "in_memory" buffers all output and returns it in job.result.
	// "tmp_file" spills output to a temporary file (registered in tmp_files).
	capture_kind: "stream" | "in_memory" | "tmp_file"

	// timeout_secs, when present, caps the child process lifetime.
	// If the child has not exited within timeout_secs the signal cascade is
	// triggered and the state transitions to TimedOut. Range: 1..86400.
	timeout_secs?: int & >=1 & <=86400

	// idempotency_key is a client-generated UUIDv7 for deduplication.
	// Reuses the idempotency-key contract from the job control-plane (ADR-0040).
	idempotency_key?: string

	// name is an operator-supplied alias scoped to (client_id, name) per ADR-0056.
	// Enables idempotent re-spawn: if (client_id, name) maps to a non-terminal
	// JobId, subprocess.spawn returns that handle instead of starting a new
	// process. Absent (default) preserves original one-shot semantics.
	// Format: lowercase alphanumeric + hyphens, 1..64 chars.
	name?: string & =~"^[a-z0-9-]{1,64}$"

	// restart_policy controls supervisor re-spawn behavior per ADR-0056.
	// Absent = Never (default, one-shot).
	restart_policy?: #RestartPolicy

	// health_probe gates the Starting -> Ready transition per ADR-0056.
	// Absent = None (Running == Ready immediately).
	health_probe?: #HealthProbe

	// log_rotation rotates capture_kind=tmp_file output per ADR-0056.
	// Absent = None (no rotation; tmp file grows unbounded).
	log_rotation?: #LogRotation
}

// #SubprocessHandle is the aggregate root for an active or completed child process.
// It is stored in the JobRegistry under the job_id and updated on every state
// transition. The handle is the authoritative record for a single spawn invocation.
#SubprocessHandle: {
	// job_id is the UUIDv7 that correlates this handle with the async job entry,
	// the MCP progressToken, and the correlation_id in audit events.
	job_id: string & =~"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"

	// pid is the OS process ID of the spawned child. Always >= 2 to exclude
	// the init process and kernel threads from the allowable range.
	pid: int & >=2

	// pgid is the process group ID assigned by setsid() at spawn time per ADR-0053.
	// killpg(pgid, signal) is used for cascade-kill so that child sub-processes
	// spawned by the child are also reaped.
	pgid: int & >=2

	// state is the current lifecycle position.
	state: #SubprocessState

	// started_at is the RFC 3339 timestamp when the child transitioned to Running.
	started_at: string

	// exit_code is the process exit status, present only when the state is
	// Succeeded or Failed.
	exit_code?: int

	// stream_chunks_dropped counts the number of stdout/stderr chunks discarded
	// due to bounded mpsc channel backpressure. A non-zero value is surfaced in
	// the job.result hints map. Per ADR-0054.
	stream_chunks_dropped: int & >=0

	// tmp_files lists the absolute paths of temporary files registered during
	// this invocation (e.g., capture_kind="tmp_file" spill paths, transactional
	// write intermediates). Cleaned up on cancel, kill, timeout, and normal exit.
	tmp_files: [...string]
}

// #SubprocessResult is the terminal output returned by the subprocess.result tool
// call once a job has reached a terminal state. It combines the ring-buffer
// aggregate with optional disk-persistence paths (TmpFile capture branch).
//
// DDD role: ValueObject (returned by SubprocessPort::result; never mutated).
//
// Cross-reference: ADR-0054 amendment 2026-05-24 — TmpFile capture branch.
#SubprocessResult: {
	// exit_code is the process exit status. Present only when terminal_state
	// is Succeeded or Failed. Absent for Cancelled, Killed, and TimedOut
	// (POSIX does not guarantee a meaningful exit code after SIGKILL, and
	// Cancelled jobs may not have exited before cancellation was processed).
	exit_code?: int

	// stdout_aggregate_base64 contains the base64-encoded last 64 KiB of
	// stdout from the ring buffer. Empty string when no bytes were captured.
	stdout_aggregate_base64: string

	// stderr_aggregate_base64 contains the base64-encoded last 64 KiB of
	// stderr from the ring buffer. Empty string when no bytes were captured.
	stderr_aggregate_base64: string

	// stdout_tmp_path is the absolute path to the final (post-rename) stdout
	// capture file. Present only when capture_kind == "tmp_file" AND
	// terminal_state == Succeeded. Absent in all other cases, including when
	// the job is still Running or when the terminal state is not Succeeded.
	stdout_tmp_path?: string

	// stderr_tmp_path is the absolute path to the final (post-rename) stderr
	// capture file. Present only when capture_kind == "tmp_file" AND
	// terminal_state == Succeeded. Absent in all other cases.
	stderr_tmp_path?: string

	// stream_chunks_dropped is the cumulative count of stdout and stderr
	// chunks dropped due to bounded mpsc channel backpressure since the job
	// was created. A non-zero value indicates the aggregate may be incomplete
	// even when stdout_tmp_path / stderr_tmp_path are present (dropped chunks
	// are still written to the tmp file; only the live notification was lost).
	stream_chunks_dropped: int & >=0

	// duration_ms is the elapsed wall-clock time from child process start
	// (Running state entry) to terminal state entry, in milliseconds.
	duration_ms: int & >=0

	// terminal_state is the final lifecycle state of the child process.
	// Always a terminal value; SubprocessResult is never returned for
	// non-terminal jobs.
	terminal_state: #SubprocessState

	// stdout_lines, when pagination was requested, contains the decoded UTF-8 lines
	// for the current page of stdout output. Absent when pagination was not requested.
	// Per ADR-0057.
	stdout_lines?: [...string]

	// stdout_total_lines is the total number of lines in the captured stdout ring buffer.
	// Present only when pagination was requested. Per ADR-0057.
	stdout_total_lines?: int & >=0

	// stdout_next_offset is the pagination offset for the next stdout page.
	// Absent when this is the last (or only) page. Per ADR-0057.
	stdout_next_offset?: int & >=0

	// stderr_lines, when pagination was requested, contains the decoded UTF-8 lines
	// for the current page of stderr output. Absent when pagination was not requested.
	// Per ADR-0057.
	stderr_lines?: [...string]

	// stderr_total_lines is the total number of lines in the captured stderr ring buffer.
	// Present only when pagination was requested. Per ADR-0057.
	stderr_total_lines?: int & >=0

	// stderr_next_offset is the pagination offset for the next stderr page.
	// Absent when this is the last (or only) page. Per ADR-0057.
	stderr_next_offset?: int & >=0
}

// #StreamChunk is the value object carried in each notifications/progress event
// for subprocess stdout and stderr output per ADR-0054. Chunks are numbered
// per-stream (not globally) and include a byte offset for reassembly.
#StreamChunk: {
	// job_id correlates the chunk with its originating SubprocessHandle.
	job_id: string & =~"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"

	// stream identifies whether the chunk originates from standard output or
	// standard error of the child process.
	stream: "stdout" | "stderr"

	// seq is the zero-based monotonic sequence number for this stream.
	// Gaps in seq indicate dropped chunks; seq never resets within a job.
	seq: int & >=0

	// chunk_base64 is the raw bytes of the chunk encoded as base64 standard
	// encoding (RFC 4648 §4). Clients decode before interpretation.
	chunk_base64: string

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
	timestamp: string
}

// DDD role: ValueObject
// #RestartPolicy controls supervisor re-spawn behavior per ADR-0056.
// Discriminated union: each variant carries its own constraints.
#RestartPolicy: {
	{
		kind: "Never"
	} | {
		kind:        "OnFailure"
		max_retries: int & >=1 & <=100
		backoff_ms:  int & >=100 & <=300000
	} | {
		kind:       "Always"
		backoff_ms: int & >=100 & <=300000
	}
}

// DDD role: ValueObject
// #HealthProbe transitions Starting -> Ready per ADR-0056.
// Three consecutive failures trigger restart_policy.
#HealthProbe: {
	{
		kind: "None"
	} | {
		kind:             "HttpGet"
		url:              string & =~"^https?://"
		expected_status:  int & >=100 & <=599
		interval_ms:      int & >=100 & <=60000
		startup_grace_ms: int & >=0 & <=600000
	} | {
		kind:             "PortOpen"
		host:             string
		port:             int & >=1 & <=65535
		interval_ms:      int & >=100 & <=60000
		startup_grace_ms: int & >=0 & <=600000
	} | {
		kind:       "LogPattern"
		regex:      string
		timeout_ms: int & >=1000 & <=600000
	}
}

// DDD role: ValueObject
// #LogRotation rotates capture_kind=tmp_file output per ADR-0056.
// Cumulative cap = max_bytes_per_file * keep_files.
#LogRotation: {
	{
		kind: "None"
	} | {
		kind:               "BySize"
		max_bytes_per_file: int & >=1048576 & <=1073741824 // 1 MiB .. 1 GiB
		keep_files:         int & >=1 & <=20
	}
}

// DDD role: ValueObject
// #Stream identifies a standard I/O channel of a child process.
// Used by #StreamChunk, #SubprocessSearchRequest, and #SearchMatch.
#Stream: "stdout" | "stderr"

// DDD role: ValueObject
// #Order controls the traversal direction for paginated subprocess output per ADR-0057.
// Tail (default) returns lines from the most-recent end; Head returns from the oldest end.
#Order: "Tail" | "Head"

// DDD role: ValueObject
// #Pagination describes a single page of line-oriented subprocess output per ADR-0057.
// Pagination is optional on subprocess.result and subprocess.search; absent means
// the caller receives the full ring-buffer aggregate without line decomposition.
#Pagination: {
	// offset is the 0-based line offset from which to start the page.
	// For order=Tail offset 0 = most-recent line; for order=Head offset 0 = oldest line.
	offset: int & >=0

	// page_size is the maximum number of lines to return in this page.
	// Default 100; hard ceiling 10000.
	page_size: int & >=1 & <=10000 | *100

	// order controls traversal direction. Default Tail (most-recent-first).
	order: #Order | *"Tail"
}

// DDD role: ValueObject
// #SubprocessResultRequest is the value object submitted to subprocess.result
// to retrieve the terminal output of a completed job per ADR-0057.
// When pagination is absent the full ring-buffer aggregate (base64 blobs) is returned.
// When pagination is set the line-decomposed fields (stdout_lines, stderr_lines, etc.) are
// populated in #SubprocessResult and the aggregate blobs are omitted.
#SubprocessResultRequest: {
	// job_id identifies the target job.
	job_id: string & =~"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"

	// pagination, when present, enables line-based paged retrieval of captured output.
	// Absent preserves original full-aggregate behavior.
	pagination?: #Pagination
}

// DDD role: ValueObject
// #SubprocessSearchRequest submits a regex search across captured subprocess output
// per ADR-0057. Results are line-oriented and optionally paginated.
#SubprocessSearchRequest: {
	// job_id identifies the target job.
	job_id: string & =~"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"

	// pattern is the regex applied to each captured output line.
	// Length: 1..1024 characters.
	pattern: string & =~"^.{1,1024}$"

	// streams limits search to the specified output channels.
	// Default: both stdout and stderr.
	streams: [...#Stream] | *["stdout", "stderr"]

	// case_insensitive, when true, applies the regex in case-insensitive mode.
	case_insensitive: bool | *false

	// pagination, when present, enables paged retrieval of matching lines.
	pagination?: #Pagination
}

// DDD role: ValueObject
// #SearchMatch is a single line that matched the search pattern per ADR-0057.
// line_number is 1-based and scoped per stream (stdout and stderr each start at 1).
#SearchMatch: {
	// stream identifies the output channel that produced the matching line.
	stream: #Stream

	// line_number is the 1-based line index within the identified stream.
	line_number: int & >=1

	// line_text is the raw text content of the matching line (newline excluded).
	line_text: string
}

// DDD role: ValueObject
// #SubprocessSearchResult is the response returned by subprocess.search per ADR-0057.
// matches contains the page of #SearchMatch entries for this request; total_matches
// reflects the full match count across all pages.
#SubprocessSearchResult: {
	// matches is the current page of matching lines.
	matches: [...#SearchMatch]

	// total_matches is the total number of lines matching the pattern across all pages.
	total_matches: int & >=0

	// next_offset, when present, is the pagination offset to pass in the next request
	// to retrieve the subsequent page. Absent indicates the last page.
	next_offset?: int & >=0
}
