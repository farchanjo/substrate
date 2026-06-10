// DDD role: AggregateRoot
package schemas

// #Signal enumerates POSIX signals that the proc namespace may deliver.
// SIGKILL and SIGSTOP are marked destructive; they cannot be caught or ignored.
#Signal:
	"SIGTERM" |   // graceful termination (default-allowed, catchable)
	"SIGHUP" |    // hangup / config reload (default-allowed, catchable)
	"SIGINT" |    // keyboard interrupt (default-allowed, catchable)
	"SIGUSR1" |   // user-defined signal 1 (default-allowed, catchable)
	"SIGUSR2" |   // user-defined signal 2 (default-allowed, catchable)
	"SIGKILL" |   // DESTRUCTIVE: force kill, cannot be caught or blocked
	"SIGSTOP"     // DESTRUCTIVE: force stop, cannot be caught or blocked

// #Allowlist defines path roots and optional per-tool path overrides.
#Allowlist: {
	// roots is the ordered list of absolute directory prefixes that tools may access.
	// Paths outside every root are rejected with SUBSTRATE_PATH_OUTSIDE_ALLOWLIST.
	roots: [...string & =~"^/"]

	// per_tool_overrides maps tool names to their own root lists,
	// narrowing (never widening) the global roots for that specific tool.
	per_tool_overrides: {[string]: [...string & =~"^/"]}
}

// #SecurityPolicy is the aggregate root governing runtime access control.
// A missing or empty policy causes the runtime to deny all filesystem and
// process operations by default (fail-closed posture).
#SecurityPolicy: {
	// allowlist controls which filesystem paths tools may read or modify.
	allowlist: #Allowlist

	// dry_run_required_for lists tool names that MUST be called with dry_run=true
	// on their first invocation; a live call without prior dry run is rejected
	// with SUBSTRATE_DRY_RUN_REQUIRED.
	dry_run_required_for: [...string]

	// elicitation_required_for lists tool names that require an explicit
	// confirmation elicitation before execution; rejected otherwise with
	// SUBSTRATE_CONFIRMATION_REQUIRED.
	elicitation_required_for: [...string]

	// signal_allowlist is the set of signals that proc tools may send.
	// Defaults exclude SIGKILL and SIGSTOP; include them only with explicit justification.
	signal_allowlist: [...#Signal] | *["SIGTERM", "SIGHUP", "SIGINT", "SIGUSR1", "SIGUSR2"]

	// outbound_net_enabled controls whether sys_* tools may open outbound TCP/UDP sockets.
	// Defaults to false (no outbound network) to prevent exfiltration.
	outbound_net_enabled: bool | *false

	// extra_redaction_patterns is a list of organization-specific regex patterns
	// whose matches are replaced with [REDACTED] in all log output per ADR-0018.
	// Patterns are Go-compatible regular expressions.
	extra_redaction_patterns: [...string] | *[]

	// reject_hardlinks causes the runtime to refuse hard links to files outside the
	// allowlist. Mirrors the runtime_config field; policy-time declaration.
	reject_hardlinks: bool | *false

	// archive_allow_symlinks permits symlinks inside extracted archive contents.
	// Disabled by default; mirrors the runtime_config field; policy-time declaration.
	archive_allow_symlinks: bool | *false

	// proc_signal_pid_allowlist_filter controls how the PID allowlist is computed
	// when proc tools deliver signals.
	// current_user_only: restrict to PIDs owned by the process's effective UID (default).
	// explicit_list: require an explicit PID list in the tool call arguments.
	proc_signal_pid_allowlist_filter: "current_user_only" | "explicit_list" | *"current_user_only"

	// refuse_degraded_jail causes startup to abort if the PathJail cannot reach tier 1
	// (openat2 on Linux, O_NOFOLLOW_ANY on macOS). Default true (fail-closed) per ADR-0035 + ADR-0042.
	// Set to false to accept the userspace-fallback jail tier in restricted environments.
	refuse_degraded_jail: bool | *true

	// refuse_polling_watcher causes startup to abort if the filesystem watcher falls back
	// to polling per ADR-0042. Default false; operators in production may set true.
	refuse_polling_watcher: bool | *false

	// refuse_degraded_simd causes startup to abort when the detected SIMD tier is "portable"
	// (no hardware acceleration available) per ADR-0043.
	refuse_degraded_simd: bool | *false

	// allow_avx512 opts in to AVX-512 code paths per ADR-0043.
	// AVX-512 is capability-detected but not activated without this flag because of
	// power-license and clock-frequency side-effects on some microarchitectures.
	allow_avx512: bool | *false

	// subprocess_policy_enforced reflects the build-time verification result that
	// no subprocess-spawning syscalls exist in the linked binary per ADR-0044.
	// The runtime sets this field at startup; operators MUST NOT override it.
	// Superseded as a hard ban by ADR-0052: subprocess is now an opt-in BC behind a
	// Cargo feature gated by the subprocess_binary_allowlist below.
	subprocess_policy_enforced: bool | *true

	// subprocess_binary_allowlist is the set of absolute binary paths that
	// subprocess.spawn may execute per ADR-0052. Default deny-all (empty list): a
	// binary absent from this list is rejected with
	// SUBSTRATE_SUBPROCESS_BINARY_NOT_ALLOWED. The recovery hint and ADR prose name
	// this key security.subprocess_binary_allowlist, while the loaded TOML path is
	// the [subprocess] section field binary_allowlist; both denote the same gate.
	subprocess_binary_allowlist: [...string & =~"^/"] | *[]

	// subprocess_env_allowlist names the environment variables (names only) that a
	// child process may inherit from substrate per ADR-0052. Banned injection
	// vectors (LD_PRELOAD, DYLD_INSERT_LIBRARIES, LD_LIBRARY_PATH,
	// DYLD_LIBRARY_PATH) are rejected unconditionally regardless of this list.
	subprocess_env_allowlist: [...string] | *[]

	// subprocess_cwd_within_allowlist requires every child working directory to
	// resolve inside security_policy.allowlist.roots per ADR-0052; a cwd outside is
	// rejected with SUBSTRATE_SUBPROCESS_CWD_OUTSIDE_ALLOWLIST. Default true.
	subprocess_cwd_within_allowlist: bool | *true

	// subprocess_max_concurrent is the global cap on active spawned children
	// (loaded TOML path: [subprocess] max_concurrent). Default 8 per ADR-0052.
	subprocess_max_concurrent: int & >=1 | *8

	// subprocess_max_per_client is the per-client cap on active spawned children
	// (loaded TOML path: [subprocess] max_per_client). Default 4 per ADR-0052.
	// Exceeding either quota yields SUBSTRATE_SUBPROCESS_QUOTA_EXCEEDED.
	subprocess_max_per_client: int & >=1 | *4
}
