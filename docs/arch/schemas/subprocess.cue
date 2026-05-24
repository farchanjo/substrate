// DDD role: AggregateRoot
//
// CUE schema for the subprocess bounded context.
//
// Cross-references:
//   ADR-0052 — subprocess bounded context decision
//   ADR-0053 — process lifecycle cascade contract (setsid, PR_SET_PDEATHSIG, watchdog pipe)
//   ADR-0054 — subprocess stdout/stderr stream multiplex via notifications/progress
//
// Dependency on shared kernel: #JobId (job.cue) for the job_id field.
package schemas

// #SubprocessState enumerates the lifecycle states of a spawned child process.
// Terminal states (Succeeded, Failed, Cancelled, Killed, TimedOut) never regress.
// Mirrors JobState from job.cue but with subprocess-specific terminal distinctions.
#SubprocessState: "Pending" | "Running" | "Cancelled" | "Killed" | "Succeeded" | "Failed" | "TimedOut"

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

	// byte_offset is the cumulative byte offset of the first byte in this chunk
	// relative to the beginning of the stream, allowing ordered reassembly even
	// when events arrive out of order.
	byte_offset: int & >=0

	// timestamp is the RFC 3339 timestamp at which the chunk was read from the
	// OS pipe into the substrate capture buffer.
	timestamp: string
}
