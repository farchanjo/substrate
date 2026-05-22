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
	"SUBSTRATE_JAIL_DEGRADED_REFUSED"

// #ErrorCategory classifies codes by operational concern.
// "job" added per ADR-0010 amendment for async-job BC per ADR-0040.
#ErrorCategory: "security" | "not_found" | "lifecycle" | "resource" | "input" | "protocol" | "internal" | "startup" | "kernel" | "job"

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
		recovery_hint:     "Upgrade kernel to >= 5.6 (Linux) or macOS >= 11, or set security.refuse_degraded_jail = false."
		category:          "startup"
	}
}
