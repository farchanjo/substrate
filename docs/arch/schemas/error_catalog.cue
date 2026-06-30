// DDD role: ReadModel
package schemas

// #ErrorCode enumerates the stable substrate error codes.
// Codes are stable identifiers; never rename or remove a code once published.
// Codes -32027 through -32033 added per ADR-0010 amendment (job BC + capability startup).
#ErrorCode:
	"SUBSTRATE_PATH_OUTSIDE_ALLOWLIST" |
	"SUBSTRATE_PATH_TRAVERSAL_BLOCKED" |
	"SUBSTRATE_SYMLINK_ESCAPE" |
	"SUBSTRATE_PERMISSION_DENIED" |
	"SUBSTRATE_NOT_FOUND" |
	"SUBSTRATE_TIMEOUT" |
	"SUBSTRATE_CANCELLED" |
	"SUBSTRATE_RESOURCE_LIMIT" |
	"SUBSTRATE_INVALID_ARGUMENT" |
	"SUBSTRATE_DRY_RUN_REQUIRED" |
	"SUBSTRATE_CONFIRMATION_REQUIRED" |
	"SUBSTRATE_PROTOCOL_VERSION_UNSUPPORTED" |
	"SUBSTRATE_INTERNAL_ERROR" |
	// Kernel-induced I/O errors (-32014 through -32019)
	"SUBSTRATE_SYMLINK_LOOP" |
	"SUBSTRATE_IO_ERROR" |
	"SUBSTRATE_STORAGE_FULL" |
	"SUBSTRATE_READ_ONLY_FS" |
	"SUBSTRATE_ENCODING_ERROR" |
	"SUBSTRATE_TRANSIENT_IO" |
	// Startup contract errors per ADR-0036 (-32020 through -32026)
	"SUBSTRATE_CONFIG_INVALID" |
	"SUBSTRATE_CONFIG_NOT_FOUND" |
	"SUBSTRATE_ALLOWLIST_ROOT_MISSING" |
	"SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE" |
	"SUBSTRATE_RUNTIME_INIT_FAILED" |
	"SUBSTRATE_FD_LIMIT_TOO_LOW" |
	"SUBSTRATE_UNSUPPORTED_PLATFORM" |
	// Async-job BC errors per ADR-0040 (-32027 through -32031)
	"SUBSTRATE_JOB_NOT_FOUND" |
	"SUBSTRATE_QUOTA_EXCEEDED" |
	"SUBSTRATE_JOB_CANCELLED" |
	"SUBSTRATE_JOB_TIMED_OUT" |
	"SUBSTRATE_RESULT_WAIT_EXCEEDED" |
	// Capability-startup errors per ADR-0042 + ADR-0043 (-32032 through -32033)
	"SUBSTRATE_TIER_OVERRIDE_INVALID" |
	"SUBSTRATE_JAIL_DEGRADED_REFUSED" |
	// Subprocess BC security and lifecycle errors per ADR-0052 + ADR-0053 + ADR-0054 + ADR-0055 (-32034 through -32043)
	"SUBSTRATE_SUBPROCESS_BINARY_NOT_ALLOWED" |
	"SUBSTRATE_SUBPROCESS_ENV_BANNED" |
	"SUBSTRATE_SUBPROCESS_CWD_OUTSIDE_ALLOWLIST" |
	"SUBSTRATE_SUBPROCESS_QUOTA_EXCEEDED" |
	"SUBSTRATE_SUBPROCESS_SPAWN_FAILED" |
	"SUBSTRATE_SUBPROCESS_TIMEOUT" |
	"SUBSTRATE_SUBPROCESS_KILLED" |
	"SUBSTRATE_ELICITATION_REQUIRED" |
	"SUBSTRATE_STREAM_CHUNK_DROPPED" |
	"SUBSTRATE_INVALID_STATE_TRANSITION" |
	// Launch BC errors per ADR-0064 + ADR-0065 + ADR-0068 (-32044 through -32053)
	"SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED" |
	"SUBSTRATE_LAUNCH_CONFIG_SYMLINK_REJECTED" |
	"SUBSTRATE_LAUNCH_CONFIG_UNTRUSTED_DIR" |
	"SUBSTRATE_LAUNCH_TRUST_STORE_INSECURE" |
	"SUBSTRATE_LAUNCH_CYCLE_DETECTED" |
	"SUBSTRATE_LAUNCH_DEPENDENCY_FAILED" |
	"SUBSTRATE_LAUNCH_ORPHAN_REAPED" |
	"SUBSTRATE_LAUNCH_ORPHAN_ADOPTED" |
	"SUBSTRATE_LAUNCH_STACK_TTL_EXPIRED" |
	"SUBSTRATE_LAUNCH_SUPERVISOR_UNREACHABLE" |
	// Launch supervisor-hardening errors per ADR-0068 (-32054 through -32056)
	"SUBSTRATE_LAUNCH_REGISTRY_INSECURE" |
	"SUBSTRATE_LAUNCH_FRAME_TOO_LARGE" |
	"SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED"

// #ErrorCategory classifies codes by operational concern.
// "job" added per ADR-0010 amendment for async-job BC per ADR-0040.
// "security_violation", "resource_limit", "io_error", "timeout", "cancellation", "user_consent", "backpressure" added per ADR-0052 subprocess BC.
#ErrorCategory: "security" | "not_found" | "lifecycle" | "resource" | "input" | "protocol" | "internal" | "startup" | "kernel" | "job" | "security_violation" | "resource_limit" | "io_error" | "timeout" | "cancellation" | "user_consent" | "backpressure"

// #ErrorEntry documents a single error code with its wire mapping and remediation hint.
#ErrorEntry: {
	// code is the stable substrate error identifier.
	code: #ErrorCode

	// http_jsonrpc_code is the JSON-RPC 2.0 error code in the application-defined range.
	// Substrate reserves -32099 through -32000 (server-defined).
	http_jsonrpc_code: int & >=-32099 & <=-32000

	// recovery_hint is an operator-facing remediation suggestion (max 150 characters).
	recovery_hint: string & =~"^.{1,150}$"

	// category groups the error for observability dashboards and alerting rules.
	category: #ErrorCategory

	// offending_field is present when the error originates from input validation
	// and identifies the specific input field that caused the failure.
	offending_field?: string
}

// #ErrorCatalog is a closed map of all error codes to their entries.
// 7 codes added per ADR-0010 amendment (job BC + capability startup codes).
// 10 codes added per ADR-0052/0053/0054/0055 (subprocess BC).
// This read model is generated; do not edit entries; open a spec ADR to evolve codes.
#ErrorCatalog: {
	SUBSTRATE_PATH_OUTSIDE_ALLOWLIST: #ErrorEntry & {
		code:              "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"
		http_jsonrpc_code: -32001
		recovery_hint:     "Add the requested path root to security_policy.allowlist.roots."
		category:          "security"
	}
	SUBSTRATE_PATH_TRAVERSAL_BLOCKED: #ErrorEntry & {
		code:              "SUBSTRATE_PATH_TRAVERSAL_BLOCKED"
		http_jsonrpc_code: -32002
		recovery_hint:     "Remove '..' segments or absolute escape sequences from the path argument."
		category:          "security"
	}
	SUBSTRATE_SYMLINK_ESCAPE: #ErrorEntry & {
		code:              "SUBSTRATE_SYMLINK_ESCAPE"
		http_jsonrpc_code: -32003
		recovery_hint:     "Resolve the symlink target and verify it stays within an allowed root."
		category:          "security"
	}
	SUBSTRATE_PERMISSION_DENIED: #ErrorEntry & {
		code:              "SUBSTRATE_PERMISSION_DENIED"
		http_jsonrpc_code: -32004
		recovery_hint:     "Check OS file permissions or adjust security_policy for this tool."
		category:          "security"
	}
	SUBSTRATE_NOT_FOUND: #ErrorEntry & {
		code:              "SUBSTRATE_NOT_FOUND"
		http_jsonrpc_code: -32005
		recovery_hint:     "Verify the path or resource identifier exists before calling."
		category:          "not_found"
	}
	SUBSTRATE_TIMEOUT: #ErrorEntry & {
		code:              "SUBSTRATE_TIMEOUT"
		http_jsonrpc_code: -32006
		recovery_hint:     "Increase timeouts.per_tool for this tool or break the operation into smaller chunks."
		category:          "lifecycle"
	}
	SUBSTRATE_CANCELLED: #ErrorEntry & {
		code:              "SUBSTRATE_CANCELLED"
		http_jsonrpc_code: -32007
		recovery_hint:     "Retry the call; cancellation originated from the client or a signal."
		category:          "lifecycle"
	}
	SUBSTRATE_RESOURCE_LIMIT: #ErrorEntry & {
		code:              "SUBSTRATE_RESOURCE_LIMIT"
		http_jsonrpc_code: -32008
		recovery_hint:     "Reduce payload size or increase semaphore_caps / buffer limits in runtime_config."
		category:          "resource"
	}
	SUBSTRATE_INVALID_ARGUMENT: #ErrorEntry & {
		code:              "SUBSTRATE_INVALID_ARGUMENT"
		http_jsonrpc_code: -32009
		recovery_hint:     "Consult the tool input_schema and correct the offending argument."
		category:          "input"
	}
	SUBSTRATE_DRY_RUN_REQUIRED: #ErrorEntry & {
		code:              "SUBSTRATE_DRY_RUN_REQUIRED"
		http_jsonrpc_code: -32010
		recovery_hint:     "Call the tool first with dry_run=true to preview changes, then re-submit."
		category:          "lifecycle"
	}
	SUBSTRATE_CONFIRMATION_REQUIRED: #ErrorEntry & {
		code:              "SUBSTRATE_CONFIRMATION_REQUIRED"
		http_jsonrpc_code: -32011
		recovery_hint:     "Obtain explicit user confirmation via the elicitation flow, then retry."
		category:          "lifecycle"
	}
	SUBSTRATE_PROTOCOL_VERSION_UNSUPPORTED: #ErrorEntry & {
		code:              "SUBSTRATE_PROTOCOL_VERSION_UNSUPPORTED"
		http_jsonrpc_code: -32012
		recovery_hint:     "Negotiate a supported protocol version during the MCP initialize handshake."
		category:          "protocol"
	}
	SUBSTRATE_INTERNAL_ERROR: #ErrorEntry & {
		code:              "SUBSTRATE_INTERNAL_ERROR"
		http_jsonrpc_code: -32099
		recovery_hint:     "Report the correlation_id from the audit log to the substrate maintainers."
		category:          "internal"
	}
	// Kernel-induced I/O errors
	SUBSTRATE_SYMLINK_LOOP: #ErrorEntry & {
		code:              "SUBSTRATE_SYMLINK_LOOP"
		http_jsonrpc_code: -32014
		recovery_hint:     "Remove circular symlink chains in the target directory tree before retrying."
		category:          "kernel"
	}
	SUBSTRATE_IO_ERROR: #ErrorEntry & {
		code:              "SUBSTRATE_IO_ERROR"
		http_jsonrpc_code: -32015
		recovery_hint:     "Check kernel dmesg and filesystem health; retry after resolving hardware issues."
		category:          "kernel"
	}
	SUBSTRATE_STORAGE_FULL: #ErrorEntry & {
		code:              "SUBSTRATE_STORAGE_FULL"
		http_jsonrpc_code: -32016
		recovery_hint:     "Free disk space on the target volume and retry the operation."
		category:          "resource"
	}
	SUBSTRATE_READ_ONLY_FS: #ErrorEntry & {
		code:              "SUBSTRATE_READ_ONLY_FS"
		http_jsonrpc_code: -32017
		recovery_hint:     "Remount the filesystem read-write or redirect writes to a writable path."
		category:          "kernel"
	}
	SUBSTRATE_ENCODING_ERROR: #ErrorEntry & {
		code:              "SUBSTRATE_ENCODING_ERROR"
		http_jsonrpc_code: -32018
		recovery_hint:     "Ensure the file is valid UTF-8 or specify the correct encoding in the request."
		category:          "input"
	}
	SUBSTRATE_TRANSIENT_IO: #ErrorEntry & {
		code:              "SUBSTRATE_TRANSIENT_IO"
		http_jsonrpc_code: -32019
		recovery_hint:     "Retry the operation; transient I/O errors typically resolve on subsequent attempts."
		category:          "kernel"
	}
	// Startup contract errors per ADR-0036
	SUBSTRATE_CONFIG_INVALID: #ErrorEntry & {
		code:              "SUBSTRATE_CONFIG_INVALID"
		http_jsonrpc_code: -32020
		recovery_hint:     "Fix the runtime_config field reported in offending_field and restart the server."
		category:          "startup"
	}
	SUBSTRATE_CONFIG_NOT_FOUND: #ErrorEntry & {
		code:              "SUBSTRATE_CONFIG_NOT_FOUND"
		http_jsonrpc_code: -32021
		recovery_hint:     "Create or mount the runtime configuration file at the expected path and restart."
		category:          "startup"
	}
	SUBSTRATE_ALLOWLIST_ROOT_MISSING: #ErrorEntry & {
		code:              "SUBSTRATE_ALLOWLIST_ROOT_MISSING"
		http_jsonrpc_code: -32022
		recovery_hint:     "Create the allowlist root directory or remove the missing entry from the policy."
		category:          "startup"
	}
	SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE: #ErrorEntry & {
		code:              "SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE"
		http_jsonrpc_code: -32023
		recovery_hint:     "Grant read permission to the substrate process on the configured allowlist root."
		category:          "startup"
	}
	SUBSTRATE_RUNTIME_INIT_FAILED: #ErrorEntry & {
		code:              "SUBSTRATE_RUNTIME_INIT_FAILED"
		http_jsonrpc_code: -32024
		recovery_hint:     "Check server logs for the root cause; verify system resources and restart."
		category:          "startup"
	}
	SUBSTRATE_FD_LIMIT_TOO_LOW: #ErrorEntry & {
		code:              "SUBSTRATE_FD_LIMIT_TOO_LOW"
		http_jsonrpc_code: -32025
		recovery_hint:     "Increase the process file-descriptor limit (ulimit -n) to at least 1024 and restart."
		category:          "startup"
	}
	SUBSTRATE_UNSUPPORTED_PLATFORM: #ErrorEntry & {
		code:              "SUBSTRATE_UNSUPPORTED_PLATFORM"
		http_jsonrpc_code: -32026
		recovery_hint:     "Run substrate on a supported OS/architecture; consult the compatibility matrix."
		category:          "startup"
	}
	// Async-job BC errors per ADR-0040
	SUBSTRATE_JOB_NOT_FOUND: #ErrorEntry & {
		code:              "SUBSTRATE_JOB_NOT_FOUND"
		http_jsonrpc_code: -32027
		recovery_hint:     "Verify job_id; expired jobs cannot be recovered."
		category:          "job"
	}
	SUBSTRATE_QUOTA_EXCEEDED: #ErrorEntry & {
		code:              "SUBSTRATE_QUOTA_EXCEEDED"
		http_jsonrpc_code: -32028
		recovery_hint:     "Wait for active jobs to complete or cancel an existing job."
		category:          "job"
	}
	SUBSTRATE_JOB_CANCELLED: #ErrorEntry & {
		code:              "SUBSTRATE_JOB_CANCELLED"
		http_jsonrpc_code: -32029
		recovery_hint:     "Retry the operation if cancellation was unintended."
		category:          "job"
	}
	SUBSTRATE_JOB_TIMED_OUT: #ErrorEntry & {
		code:              "SUBSTRATE_JOB_TIMED_OUT"
		http_jsonrpc_code: -32030
		recovery_hint:     "Increase timeout or split the work into smaller units."
		category:          "job"
	}
	SUBSTRATE_RESULT_WAIT_EXCEEDED: #ErrorEntry & {
		code:              "SUBSTRATE_RESULT_WAIT_EXCEEDED"
		http_jsonrpc_code: -32031
		recovery_hint:     "Retry with a smaller wait_ms."
		category:          "job"
	}
	// Capability-startup errors per ADR-0042 + ADR-0043
	SUBSTRATE_TIER_OVERRIDE_INVALID: #ErrorEntry & {
		code:              "SUBSTRATE_TIER_OVERRIDE_INVALID"
		http_jsonrpc_code: -32032
		recovery_hint:     "Review capabilities.override config and use a valid tier name for this port."
		category:          "startup"
	}
	SUBSTRATE_JAIL_DEGRADED_REFUSED: #ErrorEntry & {
		code:              "SUBSTRATE_JAIL_DEGRADED_REFUSED"
		http_jsonrpc_code: -32033
		recovery_hint:     "Upgrade kernel to >= 5.6 (Linux) or macOS >= 12, or set security.refuse_degraded_jail = false."
		category:          "startup"
	}
	// Subprocess BC errors per ADR-0052 + ADR-0053 + ADR-0054 + ADR-0055
	SUBSTRATE_SUBPROCESS_BINARY_NOT_ALLOWED: #ErrorEntry & {
		code:              "SUBSTRATE_SUBPROCESS_BINARY_NOT_ALLOWED"
		http_jsonrpc_code: -32034
		recovery_hint:     "Add the binary path to security.subprocess_binary_allowlist in substrate.toml and restart."
		category:          "security_violation"
	}
	SUBSTRATE_SUBPROCESS_ENV_BANNED: #ErrorEntry & {
		code:              "SUBSTRATE_SUBPROCESS_ENV_BANNED"
		http_jsonrpc_code: -32035
		recovery_hint:     "Remove the banned env var from env_override and env_allowlist; substrate enforces this independent of allowlist."
		category:          "security_violation"
	}
	SUBSTRATE_SUBPROCESS_CWD_OUTSIDE_ALLOWLIST: #ErrorEntry & {
		code:              "SUBSTRATE_SUBPROCESS_CWD_OUTSIDE_ALLOWLIST"
		http_jsonrpc_code: -32036
		recovery_hint:     "Set cwd to a path inside security.allowed_paths."
		category:          "security_violation"
	}
	SUBSTRATE_SUBPROCESS_QUOTA_EXCEEDED: #ErrorEntry & {
		code:              "SUBSTRATE_SUBPROCESS_QUOTA_EXCEEDED"
		http_jsonrpc_code: -32037
		recovery_hint:     "Wait for an active subprocess to terminate or cancel an existing job via subprocess.cancel."
		category:          "resource_limit"
	}
	SUBSTRATE_SUBPROCESS_SPAWN_FAILED: #ErrorEntry & {
		code:              "SUBSTRATE_SUBPROCESS_SPAWN_FAILED"
		http_jsonrpc_code: -32038
		recovery_hint:     "Verify binary exists at the configured path and has execute permission; inspect substrate stderr audit log."
		category:          "io_error"
	}
	SUBSTRATE_SUBPROCESS_TIMEOUT: #ErrorEntry & {
		code:              "SUBSTRATE_SUBPROCESS_TIMEOUT"
		http_jsonrpc_code: -32039
		recovery_hint:     "Increase timeout_secs on the SubprocessRequest, or split the workload into smaller invocations."
		category:          "timeout"
	}
	SUBSTRATE_SUBPROCESS_KILLED: #ErrorEntry & {
		code:              "SUBSTRATE_SUBPROCESS_KILLED"
		http_jsonrpc_code: -32040
		recovery_hint:     "Inspect audit events for the kill cascade; if triggered by substrate shutdown, restart and resubmit."
		category:          "cancellation"
	}
	SUBSTRATE_ELICITATION_REQUIRED: #ErrorEntry & {
		code:              "SUBSTRATE_ELICITATION_REQUIRED"
		http_jsonrpc_code: -32041
		recovery_hint:     "Re-invoke the tool with elicitation_confirmed: true after operator approves the form."
		category:          "user_consent"
	}
	SUBSTRATE_STREAM_CHUNK_DROPPED: #ErrorEntry & {
		code:              "SUBSTRATE_STREAM_CHUNK_DROPPED"
		http_jsonrpc_code: -32042
		recovery_hint:     "Client should drain notifications/progress faster; or use subprocess.result aggregate after job completion."
		category:          "backpressure"
	}
	SUBSTRATE_INVALID_STATE_TRANSITION: #ErrorEntry & {
		code:              "SUBSTRATE_INVALID_STATE_TRANSITION"
		http_jsonrpc_code: -32043
		recovery_hint:     "Report the correlation_id; this is an internal state machine violation."
		category:          "internal"
	}
	// Launch BC errors per ADR-0064 (trust) + ADR-0065 (deps) + ADR-0068 (orphan).
	SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_PROFILE_NOT_TRUSTED"
		http_jsonrpc_code: -32044
		recovery_hint:     "Run launch.trust to bless this .substrate.toml after reviewing it; its inode/content tuple is not in the trust store."
		category:          "security"
	}
	SUBSTRATE_LAUNCH_CONFIG_SYMLINK_REJECTED: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_CONFIG_SYMLINK_REJECTED"
		http_jsonrpc_code: -32045
		recovery_hint:     "The .substrate.toml path is a symlink; replace it with a regular file. Symlinked config is rejected per ADR-0064."
		category:          "security"
	}
	SUBSTRATE_LAUNCH_CONFIG_UNTRUSTED_DIR: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_CONFIG_UNTRUSTED_DIR"
		http_jsonrpc_code: -32046
		recovery_hint:     "The config's parent directory is world-writable or not owned by you; fix its ownership and permissions before launch.up."
		category:          "security"
	}
	SUBSTRATE_LAUNCH_TRUST_STORE_INSECURE: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_TRUST_STORE_INSECURE"
		http_jsonrpc_code: -32047
		recovery_hint:     "Set ~/.config/substrate to mode 0700 and trust.toml to 0600 owned by you; the trust store permissions are too loose."
		category:          "security"
	}
	SUBSTRATE_LAUNCH_CYCLE_DETECTED: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_CYCLE_DETECTED"
		http_jsonrpc_code: -32048
		recovery_hint:     "Remove the dependency cycle from depends_on in .substrate.toml; run launch.list to inspect the graph before retrying."
		category:          "input"
	}
	SUBSTRATE_LAUNCH_DEPENDENCY_FAILED: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_DEPENDENCY_FAILED"
		http_jsonrpc_code: -32049
		recovery_hint:     "Check launch.status for the failed dependency; fix its readiness probe or set required=false to make it optional."
		category:          "lifecycle"
	}
	SUBSTRATE_LAUNCH_ORPHAN_REAPED: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_ORPHAN_REAPED"
		http_jsonrpc_code: -32050
		recovery_hint:     "A previously detached process was reaped on startup; re-run launch.up to restart the stack."
		category:          "lifecycle"
	}
	SUBSTRATE_LAUNCH_ORPHAN_ADOPTED: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_ORPHAN_ADOPTED"
		http_jsonrpc_code: -32051
		recovery_hint:     "A detached process was re-adopted on startup; use launch.status to inspect it."
		category:          "lifecycle"
	}
	SUBSTRATE_LAUNCH_STACK_TTL_EXPIRED: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_STACK_TTL_EXPIRED"
		http_jsonrpc_code: -32052
		recovery_hint:     "The detached stack exceeded launch.orphan_ttl_secs without a client; re-run launch.up to restart it."
		category:          "lifecycle"
	}
	SUBSTRATE_LAUNCH_SUPERVISOR_UNREACHABLE: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_SUPERVISOR_UNREACHABLE"
		http_jsonrpc_code: -32053
		recovery_hint:     "The detached supervisor is not responding; run launch.status to trigger reaper-on-boot."
		category:          "lifecycle"
	}
	// Launch supervisor-hardening errors per ADR-0068 (registry/IPC + reaper).
	SUBSTRATE_LAUNCH_REGISTRY_INSECURE: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_REGISTRY_INSECURE"
		http_jsonrpc_code: -32054
		recovery_hint:     "Set the launch stacks dir to 0700 and control.fifo to 0600 owned by you, with no world-writable ancestor; then retry."
		category:          "security"
	}
	SUBSTRATE_LAUNCH_FRAME_TOO_LARGE: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_FRAME_TOO_LARGE"
		http_jsonrpc_code: -32055
		recovery_hint:     "The control-FIFO command frame exceeds PIPE_BUF-1 and was rejected to preserve atomic framing; send a smaller command."
		category:          "input"
	}
	SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED: #ErrorEntry & {
		code:              "SUBSTRATE_LAUNCH_CHILD_PID_RECYCLED"
		http_jsonrpc_code: -32056
		recovery_hint:     "A recorded child's pid was recycled to another process; the stale entry was cleared with no signal sent. Re-run launch.up."
		category:          "lifecycle"
	}
}
