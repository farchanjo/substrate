// DDD role: AggregateRoot
//
// CUE schema for the launch bounded context (declarative process orchestration).
//
// Cross-references:
//   ADR-0063 — launch orchestration bounded context (Profile / Service / Stack)
//   ADR-0064 — profile trust model (TOFU, #TrustRecord)
//   ADR-0065 — dependency graph and reconciler reload (depends_on, required)
//   ADR-0066 — event stream and notification model (#LaunchEvent)
//   ADR-0067 — concurrency and messaging topology (#LaunchChannelBounds)
//   ADR-0068 — detached supervisor and orphan governance (#SupervisorRegistry, #DisconnectPolicy)
//
// Composition: each #LaunchService materializes to exactly one subprocess.spawn;
// #RestartPolicy, #HealthProbe, #SubprocessState, and #Stream are reused verbatim
// from subprocess.cue (ADR-0056 / ADR-0054) — both files share package schemas.
package schemas

// DDD role: ValueObject
// #ServiceName is the operator-supplied alias for a Service within a Profile.
// Mirrors the subprocess name contract: lowercase alphanumeric + hyphens, 1..64.
#ServiceName: string & =~"^[a-z0-9-]{1,64}$"

// DDD role: ValueObject
// #DisconnectPolicy governs what happens to a Stack when the MCP client (the
// process that issued launch.up) disconnects. Per ADR-0068.
// "shutdown" (default) drains and kills the Stack — zero surviving processes.
// "detach" keeps the Stack alive under a detached supervisor, re-attachable later.
#DisconnectPolicy: "shutdown" | "detach"

// DDD role: ValueObject
// #LaunchService is one entry in a Profile catalog. It materializes to a single
// supervised child process via subprocess.spawn. Per ADR-0063 / ADR-0065.
#LaunchService: {
	// command is the executable plus arguments as an array. A bare string form is
	// rejected at parse time per ADR-0064 to remove the argument-injection surface.
	// command[0] is the binary; it must be in security.subprocess_binary_allowlist.
	// command[0] may be an absolute path, a cwd-relative path (with a separator, e.g.
	// "./gradlew"), or a bare name resolved on $PATH (e.g. "node"); the launch BC
	// resolves it to an absolute path BEFORE building the SubprocessRequest, so
	// subprocess.cue's absolute-path binary_path contract is preserved and the binary
	// allowlist remains the execution gate (ADR-0070).
	command: [string, ...string]

	// args are appended after command[1:]. Present for ergonomic separation of the
	// invocation (command) from per-environment arguments (args).
	args: [...string]

	// env are explicit key=value overrides in the child environment, subject to the
	// same banned-variable list as the subprocess BC (LD_PRELOAD and friends rejected).
	env: [string]: string

	// cwd is the working directory for the child, validated by PathJail. Absolute.
	cwd?: string & !=""

	// depends_on lists the Services that must reach Ready before this Service starts.
	// The union of all depends_on edges must form a DAG (ADR-0065); a cycle is rejected.
	depends_on: [...#ServiceName]

	// required, when false, demotes a missing or failed dependency from a blocker to a
	// warning (optional sidecars not run by every developer). Default true. Per ADR-0065.
	required: bool | *true

	// restart_policy controls supervisor re-spawn on this Service's own exit.
	// Reused verbatim from subprocess.cue (ADR-0056). Absent = Never (one-shot).
	restart_policy?: #RestartPolicy

	// health_probe gates the Starting -> Ready transition and therefore the readiness
	// gate that dependents wait on. Reused from subprocess.cue (ADR-0056).
	health_probe?: #HealthProbe

	// on_dependency_restart selects whether this Service restarts when a dependency is
	// restarted by the reconciler or cascade. Default restart. Per ADR-0065.
	on_dependency_restart: "restart" | "ignore" | *"restart"

	// error_patterns are regex applied to stdout/stderr to distil semantic-plane events
	// (ADR-0066). Matches are coalesced and rate-capped, never streamed raw.
	error_patterns: [...string]

	// redact are per-Service regex applied at the source before any line reaches the
	// event-log or the model context (ADR-0066), merged with the global denylist.
	redact: [...string]

	// streams selects multiplexed (single tagged channel) or separate per-Service
	// output channels. Default multiplexed per ADR-0067. Spawn-time field (ADR-0065).
	streams: "multiplexed" | "separate" | *"multiplexed"
}

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
	services: [#ServiceName]: #LaunchService
}

// DDD role: ValueObject
// #StackState is the lifecycle position of a whole Stack, distinct from the per-Service
// #SubprocessState. Draining and Down are terminal for the Stack instance. Per ADR-0063.
#StackState: "Pending" | "Starting" | "Running" | "Degraded" | "Draining" | "Detached" | "Down"

// DDD role: AggregateRoot
// #Stack is the running instance of a Profile: the dependency graph, the per-Service
// handles, the pinned config, and the lifecycle state. Per ADR-0063.
#Stack: {
	// stack_id is the UUIDv7 (Crockford base32, 26 chars) identifying this Stack instance.
	stack_id: string & =~"^[0-9A-HJKMNP-TV-Z]{26}$"

	// profile_path is the absolute, canonical path of the .substrate.toml this Stack pins.
	profile_path: string & !=""

	// config_hash is the content hash of the Profile pinned at launch.up time (ADR-0064).
	// A running Stack is immutable; an on-disk edit changes this only on reload.
	config_hash: string & =~"^(blake3|sha256):"

	// policy is the resolved disconnect policy in force for this Stack instance.
	policy: #DisconnectPolicy

	// state is the current Stack lifecycle position.
	state: #StackState

	// services maps each Service name to its current per-process lifecycle state.
	services: [#ServiceName]: #SubprocessState

	// supervisor is present only for a detached Stack (policy == "detach"); it records
	// the durable supervisor registry entry. Absent for in-session Stacks. Per ADR-0068.
	supervisor?: #SupervisorRegistry
}

// DDD role: ValueObject
// #StackChild is one supervised child recorded in the durable registry. The pgid is the
// process-group leader id used for cascade reap of the whole subtree. Per ADR-0068.
#StackChild: {
	name: #ServiceName
	pid:  int & >=2
	pgid: int & >=2

	// start_epoch is the child's process start-time (seconds since the Unix epoch:
	// /proc/<pid>/stat field 22 on Linux, kinfo_proc.p_starttime on macOS). The
	// reaper re-reads the live start-time and compares before any adopt/re-attach/
	// killpg; a mismatch means the pid was recycled (ADR-0068), so the entry is
	// cleared and no signal is sent.
	start_epoch: int & >=0
}

// DDD role: Entity
// #SupervisorRegistry is the durable per-Stack state-file written atomically (ADR-0033)
// under the user state directory. It is the rendezvous a fresh MCP server uses to
// re-attach to, adopt, or reap a detached Stack. Per ADR-0068.
#SupervisorRegistry: {
	// supervisor_pid is the OS pid of the detached `substrate --supervise` process.
	supervisor_pid: int & >=2

	// start_epoch is the supervisor start time in seconds since the Unix epoch; used to
	// distinguish a live supervisor from a stale registry entry after pid reuse.
	start_epoch: int & >=0

	// policy is the disconnect policy under which the Stack was detached.
	policy: #DisconnectPolicy

	// config_hash pins the Profile content the supervisor is running.
	config_hash: string & =~"^(blake3|sha256):"

	// children are the supervised processes owned by this supervisor.
	children: [...#StackChild]
}

// DDD role: ValueObject
// #TrustRecord is one bless entry in the user-scope trust store (trust.toml). It binds a
// canonical Profile path to its full inode-and-content identity tuple, re-verified on
// every load to defeat permission-flip and rewrite attacks. Per ADR-0064.
#TrustRecord: {
	// path is the absolute canonical path of the trusted .substrate.toml.
	path: string & !=""

	// dev / ino / uid / mode are the inode identity captured by fstat at bless time and
	// re-checked on every load. mode is masked to the permission bits (0o7777 = 4095).
	dev:  int & >=0
	ino:  int & >=0
	uid:  int & >=0
	mode: int & >=0 & <=4095

	// content is the prefixed content hash of the file at bless time (blake3 or sha256).
	content: string & =~"^(blake3|sha256):"

	// blessed_at is the RFC 3339 timestamp the record was created.
	blessed_at: string
}

// DDD role: ValueObject
// #LaunchOperatorConfig is the user-scope launch operator policy, loaded at startup
// from ${XDG_CONFIG_HOME:-~/.config}/substrate/launch.toml (mode 0600, owner-checked).
// It lives OUTSIDE any repository so a cloned Profile cannot authorize its own
// blessing (trust-order confusion). Per ADR-0064.
#LaunchOperatorConfig: {
	// auto_bless_paths lists absolute canonical path prefixes for which launch.up may
	// bless a new content/identity tuple inline instead of requiring launch.trust.
	// Empty (default) means every new Profile needs an explicit launch.trust ceremony.
	// A repository cannot add itself here; only the operator edits user-scope config.
	auto_bless_paths: [...string] | *[]
}

// DDD role: ValueObject
// #LaunchEventKind enumerates the typed lifecycle plane plus the SEMANTIC marker for the
// heuristic plane. Lifecycle events are authoritative; SEMANTIC events are advisory.
// Per ADR-0066.
#LaunchEventKind: "STARTED" | "READY" | "EXITED" | "CRASHED" | "RESTARTING" | "ORPHAN_REAPED" | "ORPHAN_ADOPTED" | "STACK_TTL_EXPIRED" | "SEMANTIC"

// DDD role: ValueObject
// #LaunchEvent is one entry in the durable per-Stack event-log (events.ndjson) and the
// unit delivered over the events resource and replay. The cursor is the opaque ?since
// value (ADR-0008) a client passes to read the delta. Per ADR-0066.
#LaunchEvent: {
	// stack_id correlates the event with its Stack.
	stack_id: string & =~"^[0-9A-HJKMNP-TV-Z]{26}$"

	// service is the originating Service; absent for Stack-level events.
	service?: #ServiceName

	// kind is the event classification.
	kind: #LaunchEventKind

	// seq is the zero-based monotonic sequence number within the Stack event-log.
	seq: int & >=0

	// cursor is the opaque pagination cursor addressing this position in the log.
	cursor: string & !=""

	// stream is present only for SEMANTIC events distilled from a child output channel.
	stream?: #Stream

	// message is the redacted, human-oriented event text (already passed the denylist).
	message: string

	// exit_code is present only for EXITED / CRASHED events.
	exit_code?: int

	// timestamp is the RFC 3339 time the event was recorded.
	timestamp: string
}

// DDD role: ValueObject
// #LaunchChannelBounds carries the configurable bounds for the lock-free messaging
// fabric, all with defaults. Per ADR-0067 (channel capacities) and ADR-0066 (rate caps)
// and ADR-0065 (orchestrated-restart rate limit).
#LaunchChannelBounds: {
	// stdout_mpsc_capacity bounds the per-Service stdout/stderr reader channel; overflow
	// is dropped with a count, never awaited (the pipe is never blocked).
	stdout_mpsc_capacity: int & >=1 | *1024

	// event_broadcast_capacity bounds the per-Stack broadcast bus; a lagging consumer
	// receives Lagged(n) drop-with-count backpressure.
	event_broadcast_capacity: int & >=1 | *256

	// notify_rate_per_sec caps semantic-event emission per Service per second.
	notify_rate_per_sec: int & >=1 | *5

	// orchestrated_restart_per_min caps reconciler/cascade restarts per Service per minute.
	orchestrated_restart_per_min: int & >=1 | *60
}
