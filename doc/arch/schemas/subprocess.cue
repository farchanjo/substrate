// DDD role: AggregateRoot
//
// CUE schema for the subprocess bounded context: the spawn request, the lifecycle
// handle, and the supervisor policy value objects.
//
// Cross-references:
//   ADR-0052 — subprocess bounded context decision
//   ADR-0053 — process lifecycle cascade contract (setsid, PR_SET_PDEATHSIG, watchdog pipe)
//   ADR-0054 — subprocess stdout/stderr stream multiplex via notifications/progress
//   ADR-0056 — subprocess supervisor semantics (#RestartPolicy, #HealthProbe, #LogRotation)
//   ADR-0057 — subprocess output pagination and search
//
// Sibling files, same `schemas` package:
//   subprocess_identity.cue    — #ProcessId, #ProcessGroupId, #ProcessIdentity
//   subprocess_collections.cue — #ArgumentList, #EnvNameList, #EnvOverrideMap, #AbsolutePathList
//   subprocess_output.cue      — result, stream chunk, and pagination shapes
//   subprocess_search.cue      — search request, match, and response
//
// Dependency on shared kernel: #JobId and #PageSize (job.cue / shared_kernel.cue).
package schemas

// DDD role: ValueObject
// #SubprocessState enumerates the lifecycle states of a spawned child process.
// Terminal states (Succeeded, Failed, Cancelled, Killed, TimedOut) never regress.
// Mirrors JobState from job.cue but with subprocess-specific terminal distinctions.
#SubprocessState: "Pending" | "Starting" | "Running" | "Ready" | "Restarting" | "Cancelled" | "Killed" | "Succeeded" | "Failed" | "TimedOut"

// DDD role: ValueObject
// #ProcessInvocation is the execution half of a #SubprocessRequest: which binary
// runs, with which arguments, in which directory, and with which environment.
// Embedded by #SubprocessRequest.
#ProcessInvocation: {
	// binary_path is the absolute path to the executable to spawn.
	// MUST be an absolute path (begins with /). Validated against
	// security.subprocess_binary_allowlist before spawning.
	binary_path: string & !=""

	// args is the argument list passed to the binary; argv[0] is binary_path.
	args: #ArgumentList

	// env_allowlist contains the names (not values) of environment variables
	// from the substrate process environment that may be inherited by the child.
	// Values are always inherited from substrate's own environment; this field
	// controls which names are visible. LD_PRELOAD and related injection vectors
	// are unconditionally banned regardless of this list.
	env_allowlist: #EnvNameList

	// env_override provides explicit key=value overrides in the child environment.
	// Every key in env_override is subject to the same banned-variable list as
	// env_allowlist: LD_PRELOAD, DYLD_INSERT_LIBRARIES, LD_LIBRARY_PATH,
	// and DYLD_LIBRARY_PATH are unconditionally rejected.
	env_override: #EnvOverrideMap

	// cwd is the working directory for the child process, validated by PathJail.
	// MUST be an absolute path (begins with /).
	cwd: string & !=""
}

// DDD role: ValueObject
// #StreamCaptureSpec is the stdio wiring of a #SubprocessRequest: how the child
// receives stdin and how its stdout/stderr are captured. Embedded by
// #SubprocessRequest.
#StreamCaptureSpec: {
	// stdin_kind describes how the child process receives standard input.
	// "none" closes stdin, "piped" allows the caller to stream bytes in,
	// "file_path" reads from a pre-existing file given in stdin_file_path.
	stdin_kind: "none" | "piped" | "file_path"

	// stdin_file_path is required when stdin_kind is "file_path"; absent otherwise.
	stdin_file_path?: #AbsolutePath

	// capture_kind controls how stdout and stderr are captured.
	// "stream" emits chunks via notifications/progress (ADR-0054).
	// "in_memory" buffers all output and returns it in job.result.
	// "tmp_file" spills output to a temporary file (registered in tmp_files).
	capture_kind: "stream" | "in_memory" | "tmp_file"
}

// DDD role: ValueObject
// #SubprocessSupervision is the lifecycle-policing half of a #SubprocessRequest:
// the lifetime cap and the ADR-0056 supervisor policies. Embedded by
// #SubprocessRequest.
#SubprocessSupervision: {
	// timeout_secs, when present, caps the child process lifetime.
	// If the child has not exited within timeout_secs the signal cascade is
	// triggered and the state transitions to TimedOut. Range: 1..86400.
	timeout_secs?: int & >=1 & <=86400

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

// #SubprocessRequest is the value object submitted by an MCP client to launch a
// child process. All fields are validated by the subprocess_invariants Rego policy
// before any OS call is made.
// DDD role: ValueObject
#SubprocessRequest: {
	// How the child is executed and with which environment.
	#ProcessInvocation

	// How the child receives stdin and how its output is captured.
	#StreamCaptureSpec

	// Lifetime cap, restart policy, readiness probe, and log rotation.
	#SubprocessSupervision

	// idempotency_key is a client-generated UUIDv7 for deduplication.
	// Reuses the idempotency-key contract from the job control-plane (ADR-0040).
	idempotency_key?: #IdempotencyKey

	// name is an operator-supplied alias scoped to (client_id, name) per ADR-0056.
	// Enables idempotent re-spawn: if (client_id, name) maps to a non-terminal
	// JobId, subprocess.spawn returns that handle instead of starting a new
	// process. Absent (default) preserves original one-shot semantics.
	// Format: lowercase alphanumeric + hyphens, 1..64 chars.
	name?: string & =~"^[a-z0-9-]{1,64}$"
}

// #SubprocessHandle is the aggregate root for an active or completed child process.
// It is stored in the JobRegistry under the job_id and updated on every state
// transition. The handle is the authoritative record for a single spawn invocation.
#SubprocessHandle: {
	// job_id is the UUIDv7 (Crockford base32, 26 chars) that correlates this handle
	// with the async job entry, the MCP progressToken, and the correlation_id in
	// audit events. Aliases #JobId so the ADR-0040 triple-equality holds at the
	// schema level (job_id == progressToken == correlation_id).
	job_id: #JobId

	// pid and pgid of the spawned child, used for cascade-kill.
	#ProcessIdentity

	// state is the current lifecycle position.
	state: #SubprocessState

	// started_at is the RFC 3339 timestamp when the child transitioned to Running.
	started_at: #Timestamp

	// exit_code is the process exit status, present only when the state is
	// Succeeded or Failed.
	exit_code?: #ExitCode

	// stream_chunks_dropped counts the number of stdout/stderr chunks discarded
	// due to bounded mpsc channel backpressure. A non-zero value is surfaced in
	// the job.result hints map. Per ADR-0054.
	stream_chunks_dropped: int & >=0

	// tmp_files lists the absolute paths of temporary files registered during
	// this invocation (e.g., capture_kind="tmp_file" spill paths, transactional
	// write intermediates). Cleaned up on cancel, kill, timeout, and normal exit.
	tmp_files: #AbsolutePathList
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