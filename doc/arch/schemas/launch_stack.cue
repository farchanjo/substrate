// DDD role: ValueObject
package schemas

// Stack aggregate, durable supervisor registry, and control frames of the launch
// bounded context. Per ADR-0063 and ADR-0068.

// #StackState is the lifecycle position of a whole Stack, distinct from the per-Service
// #SubprocessState. Draining and Down are terminal for the Stack instance. Per ADR-0063.
#StackState: "Pending" | "Starting" | "Running" | "Degraded" | "Draining" | "Detached" | "Down"

// #ServiceStateMap maps each Service name to its current per-process lifecycle state.
#ServiceStateMap: [#ServiceName]: #SubprocessState

// DDD role: AggregateRoot
// #Stack is the running instance of a Profile: the dependency graph, the per-Service
// handles, the pinned config, and the lifecycle state. Per ADR-0063.
#Stack: {
	// stack_id is the UUIDv7 (Crockford base32, 26 chars) identifying this Stack instance.
	stack_id: #StackId

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
	services: #ServiceStateMap

	// supervisor is present only for a detached Stack (policy == "detach"); it records
	// the durable supervisor registry entry. Absent for in-session Stacks. Per ADR-0068.
	supervisor?: #SupervisorRegistry
}

// #StackChild is one supervised child recorded in the durable registry. The pgid is the
// process-group leader id used for cascade reap of the whole subtree. Per ADR-0068.
#StackChild: {
	// name is the Service alias this child materializes.
	name: #ServiceName

	// pid and pgid are the OS process identity used for the adopt/reap decision.
	#ProcessIdentity

	// start_epoch is the child's process start-time (seconds since the Unix epoch:
	// /proc/<pid>/stat field 22 on Linux, kinfo_proc.p_starttime on macOS). The
	// reaper re-reads the live start-time and compares before any adopt/re-attach/
	// killpg; a mismatch means the pid was recycled (ADR-0068), so the entry is
	// cleared and no signal is sent.
	start_epoch: int & >=0
}

// #StackChildList is the ordered list of supervised children a registry records.
#StackChildList: [...#StackChild]

// DDD role: Entity
// #SupervisorRegistry is the durable per-Stack state-file written atomically (ADR-0033)
// under the user state directory. It is the rendezvous a fresh MCP server uses to
// re-attach to, adopt, or reap a detached Stack. Per ADR-0068. Its identity is the
// (supervisor_pid, start_epoch) pair: the pid names the detached supervisor process
// and the start-time distinguishes a live supervisor from a stale entry after pid reuse.
#SupervisorRegistry: {
	// supervisor_pid is the OS pid of the detached `substrate --supervise` process.
	supervisor_pid: #ProcessId

	// start_epoch is the supervisor start time in seconds since the Unix epoch; used to
	// distinguish a live supervisor from a stale registry entry after pid reuse.
	start_epoch: int & >=0

	// policy is the disconnect policy under which the Stack was detached.
	policy: #DisconnectPolicy

	// config_hash pins the Profile content the supervisor is running.
	config_hash: string & =~"^(blake3|sha256):"

	// children are the supervised processes owned by this supervisor. Empty is
	// a valid, expected value in the narrow window between the supervisor's
	// initial publish (supervisor_pid + config_hash, before any Service is
	// spawned) and its first post-spawn flush: a fresh MCP server treats "the
	// registry exists with a matching config_hash and a live supervisor_pid"
	// as the complete up(detach) readiness contract, never waiting for
	// children to be populated (ADR-0068/ADR-0056 2026-07-01 amendment).
	children: #StackChildList
}

// #ControlFrame is one newline-delimited JSON command written to a detached
// Stack's control.fifo (ADR-0068 "Lock-free multiplexed IPC"). Serialized with
// an internally tagged `type` discriminator; each write(2) call is exactly one
// frame, bounded to MAX_COMMAND_FRAME_SIZE (PIPE_BUF - 1) bytes before the
// trailing newline delimiter that a reader splits on. Added the "restart"
// variant in the 2026-07-01 amendment, alongside wiring `down`/`restart`/
// `reload` on a detached Stack to actually write these frames in production.
// Each variant is its own type so a frame stays a small entity.
#ControlFrame: #ControlDownFrame | #ControlReloadFrame | #ControlRestartFrame

// #ControlDownFrame requests a graceful teardown of the named Stack.
#ControlDownFrame: {
	type:     "down"
	stack_id: #StackId
}

// #ControlReloadFrame requests the supervisor reload the Stack from (optionally) a new
// Profile path, restarting the dependency-closure of changed Services (ADR-0065).
// profile_path absent re-reads the pinned path on file.
#ControlReloadFrame: {
	type:          "reload"
	stack_id:      #StackId
	profile_path?: #AbsolutePath
}

// #ControlRestartFrame requests the supervisor restart exactly one named Service: a
// fresh spawn, not counted against the subprocess crash-loop budget, mirroring the
// in-session launch.restart semantics for a detached Stack.
#ControlRestartFrame: {
	type:         "restart"
	stack_id:     #StackId
	service_name: #ServiceName
}