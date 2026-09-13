// DDD role: AggregateRoot
//
// CUE schema for the launch bounded context (declarative process orchestration):
// the Profile catalog, its per-Service launch spec, and the trust store.
//
// Cross-references:
//   ADR-0063 — launch orchestration bounded context (Profile / Service / Stack)
//   ADR-0064 — profile trust model (TOFU, #TrustRecord)
//   ADR-0065 — dependency graph and reconciler reload (depends_on, required)
//   ADR-0070 — PATH binary resolution (command[0] resolution before spawn)
//   ADR-0071 — .env file support (env_file)
//
// Composition: each #LaunchService materializes to exactly one subprocess.spawn;
// #RestartPolicy, #HealthProbe, #ArgumentList, #EnvOverrideMap, #ProcessId,
// #ProcessGroupId, #SubprocessState, and #Stream are reused verbatim from
// subprocess.cue (ADR-0056 / ADR-0054) — every schema file shares package schemas.
//
// Sibling files, same `schemas` package:
//   launch_collections.cue — #CommandLine, #EnvFilePathList, #ServiceNameList, #PatternList
//   launch_stack.cue       — Stack aggregate, supervisor registry, control frames
//   launch_event.cue       — event kinds, event log entry, channel bounds
//   launch_trust.cue       — trust record and operator policy
package schemas

// DDD role: ValueObject
// #ServiceName is the operator-supplied alias for a Service within a Profile.
// Mirrors the subprocess name contract: lowercase alphanumeric + hyphens, 1..64.
#ServiceName: string & =~"^[a-z0-9-]{1,64}$"

// DDD role: ValueObject
// #StackId is the UUIDv7 (Crockford base32, 26 chars) identifying a Stack instance.
#StackId: string & =~"^[0-9A-HJKMNP-TV-Z]{26}$"

// DDD role: ValueObject
// #DisconnectPolicy governs what happens to a Stack when the MCP client (the
// process that issued launch.up) disconnects. Per ADR-0068.
// "shutdown" (default) drains and kills the Stack — zero surviving processes.
// "detach" keeps the Stack alive under a detached supervisor, re-attachable later.
#DisconnectPolicy: "shutdown" | "detach"

// DDD role: ValueObject
// #ServiceCommand is the process-invocation half of a #LaunchService: what to
// run, with which arguments, in which directory, and with which environment.
// Embedded by #LaunchService.
#ServiceCommand: {
	// command is the executable plus arguments as an array. A bare string form is
	// rejected at parse time per ADR-0064 to remove the argument-injection surface.
	// command[0] is the binary; it must be in security.subprocess_binary_allowlist.
	// command[0] may be an absolute path, a cwd-relative path (with a separator, e.g.
	// "./gradlew"), or a bare name resolved on $PATH (e.g. "node"); the launch BC
	// resolves it to an absolute path BEFORE building the SubprocessRequest, so
	// subprocess.cue's absolute-path binary_path contract is preserved and the binary
	// allowlist remains the execution gate (ADR-0070).
	command: #CommandLine

	// args are appended after command[1:]. Present for ergonomic separation of the
	// invocation (command) from per-environment arguments (args).
	args: #ArgumentList

	// env are explicit key=value overrides in the child environment, subject to the
	// same banned-variable list as the subprocess BC (LD_PRELOAD and friends rejected).
	env: #EnvOverrideMap

	// env_file lists .env files loaded into the child environment (ADR-0071). Each
	// path is relative to the profile directory and must not escape it (no absolute
	// paths, no ".."). Files apply in order (a later file overrides an earlier one)
	// and the inline env map overrides all of them. Values feed the same banned-key
	// validation as env.
	env_file?: #EnvFilePathList

	// cwd is the working directory for the child, validated by PathJail. Absolute.
	cwd?: string & !=""
}

// DDD role: ValueObject
// #ServiceSupervision is the readiness-and-restart half of a #LaunchService:
// which Services gate this one, and how it is re-spawned. Embedded by
// #LaunchService.
#ServiceSupervision: {
	// restart_policy controls supervisor re-spawn on this Service's own exit.
	// Reused verbatim from subprocess.cue (ADR-0056). Absent = Never (one-shot).
	restart_policy?: #RestartPolicy

	// health_probe gates the Starting -> Ready transition and therefore the readiness
	// gate that dependents wait on. Reused from subprocess.cue (ADR-0056).
	health_probe?: #HealthProbe

	// required, when false, demotes a missing or failed dependency from a blocker to a
	// warning (optional sidecars not run by every developer). Default true. Per ADR-0065.
	required: bool | *true

	// on_dependency_restart selects whether this Service restarts when a dependency is
	// restarted by the reconciler or cascade. Default restart. Per ADR-0065.
	on_dependency_restart: "restart" | "ignore" | *"restart"

	// depends_on lists the Services that must reach Ready before this Service starts.
	// The union of all depends_on edges must form a DAG (ADR-0065); a cycle is rejected.
	depends_on: #ServiceNameList
}

// DDD role: ValueObject
// #ServiceOutput is the output-distillation half of a #LaunchService: which
// output channels exist and which patterns shape the semantic event plane.
// Embedded by #LaunchService.
#ServiceOutput: {
	// error_patterns are regex applied to stdout/stderr to distil semantic-plane events
	// (ADR-0066). Matches are coalesced and rate-capped, never streamed raw.
	error_patterns: #PatternList

	// redact are per-Service regex applied at the source before any line reaches the
	// event-log or the model context (ADR-0066), merged with the global denylist.
	redact: #PatternList

	// streams selects multiplexed (single tagged channel) or separate per-Service
	// output channels. Default multiplexed per ADR-0067. Spawn-time field (ADR-0065).
	streams: "multiplexed" | "separate" | *"multiplexed"
}

// DDD role: ValueObject
// #LaunchService is one entry in a Profile catalog. It materializes to a single
// supervised child process via subprocess.spawn. Per ADR-0063 / ADR-0065.
#LaunchService: {
	// Invocation, environment, and working directory.
	#ServiceCommand

	// Readiness gate and restart policy.
	#ServiceSupervision

	// Output channels and event-distillation patterns.
	#ServiceOutput
}

// DDD role: ValueObject
// #ServiceCatalog is the Profile's Service map, keyed by #ServiceName. Each entry
// is one supervised child process.
#ServiceCatalog: [#ServiceName]: #LaunchService

// DDD role: ValueObject
// #LaunchProfile is the value object parsed from .substrate.toml: the catalog of
// Services plus Stack-level defaults. Immutable once loaded and trusted. Per ADR-0063.
#LaunchProfile: {
	// version is the Profile schema version; reserved for forward migration.
	version: int & >=1 | *1

	// on_client_disconnect is the Stack-level default disconnect policy (ADR-0068).
	on_client_disconnect: #DisconnectPolicy | *"shutdown"

	// orphan_ttl_secs bounds how long a detached Stack may run with no client attached
	// before it is automatically brought down (ADR-0068). Default 1 hour; 0 disables
	// detached survival entirely (treated as shutdown). Range 0..86400.
	orphan_ttl_secs: int & >=0 & <=86400 | *3600

	// services is the catalog keyed by Service name. Each entry is one supervised child.
	// NOTE: inline auto-blessing is NOT a Profile field — it lives in user-scope
	// #LaunchOperatorConfig (~/.config/substrate/launch.toml) so a cloned repo cannot
	// authorize its own blessing (ADR-0064 trust-order-confusion defense).
	services: #ServiceCatalog
}