// DDD role: ValueObject
package schemas

// #AuditEventCode enumerates structured lifecycle codes for audit events that are
// not tied to a tool invocation (startup, capability selection, policy events).
// Per ADR-0038 amendment.
#AuditEventCode:
	// Emitted once at startup when capability tiers are resolved per ADR-0042.
	"SUBSTRATE_CAPABILITY_TIERS_SELECTED" |
	// Emitted once at startup after CPUID/HWCAP detection selects the SIMD tier per ADR-0043.
	"SUBSTRATE_SIMD_TIER_DETECTED" |
	// Emitted when PathJail falls back below tier 1 per ADR-0035 + ADR-0042.
	"SUBSTRATE_JAIL_DEGRADED" |
	// Emitted at startup after build-time subprocess-policy verification per ADR-0044.
	"SUBSTRATE_SUBPROCESS_POLICY_VERIFIED" |
	// Emitted on every async job state transition; subtype is carried in structured_content per ADR-0040.
	"SUBSTRATE_JOB_STATE_TRANSITION"

// #AuditOutcome enumerates the terminal states of a tool invocation.
// "attempted" is emitted as a BEFORE event for mutating tools per ADR-0038.
#AuditOutcome: "attempted" | "success" | "error" | "cancelled" | "timeout" | "dry_run_only"

// #AuditEvent is an immutable record emitted for every tool invocation.
// Events are append-only; never mutate a written event.
// When outcome is "error" or "timeout", error_code MUST be present (enforced via disjunction below).
#AuditEvent: ({
	outcome:    "error" | "timeout"
	error_code: #ErrorCode
} | {
	outcome: "attempted" | "success" | "cancelled" | "dry_run_only"
}) & {
	// correlation_id is a UUIDv7 (time-ordered) that links events across a request chain.
	// Format: 8-4-4-4-12 hex groups; the top 4 bits of the third group must be '7'.
	correlation_id: string & =~"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"

	// timestamp is the invocation start time in ISO 8601 UTC format (Z suffix required).
	timestamp: string & =~"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]+)?Z$"

	// tool_name is the fully-qualified tool identifier (namespace_name).
	// "job" prefix added per ADR-0040 (async job BC).
	tool_name: string & =~"^(fs|proc|sys|text|archive|job)_[a-z][a-z0-9_]*$"

	// args_summary is a redacted, human-readable rendering of the invocation arguments.
	// Sensitive values (tokens, passwords, PII) MUST be replaced with [REDACTED]
	// before this field is populated.
	args_summary: string

	// outcome records the terminal state of the invocation.
	outcome: #AuditOutcome

	// duration_ms is the wall-clock execution time in milliseconds (non-negative integer).
	duration_ms: uint

	// seq is a monotonically increasing counter across the process lifetime.
	// Used to detect dropped or reordered audit events.
	seq: uint

	// active_requests_at_start is the number of concurrent in-flight tool calls
	// at the moment this tool's entry point was reached.
	active_requests_at_start: uint

	// elicitation_used records whether the caller obtained explicit user confirmation
	// via the MCP elicitation flow before this invocation proceeded.
	elicitation_used: bool

	// dry_run records whether this invocation ran in preview mode with no side effects.
	dry_run: bool

	// workflow_id is a UUIDv7 that groups tool calls belonging to the same
	// multi-tool prompt workflow. Absent for single-call invocations.
	workflow_id?: string & =~"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"

	// client_id identifies the MCP client session per ADR-0040 (session attribution).
	// Absent for invocations where no client identity was negotiated.
	client_id?: string & =~"^[A-Za-z0-9._-]{1,64}$"

	// job_id is present for events that belong to an async job lifecycle per ADR-0040.
	// Format is a ULID (Crockford base32, 26 chars).
	job_id?: string & =~"^[0-9A-HJKMNP-TV-Z]{26}$"

	// idempotency_key is present when the originating job submission carried an
	// idempotency key per ADR-0040.
	idempotency_key?: string & =~"^[0-9A-HJKMNP-TV-Z]{26}$"

	// sequence_number is present when this event is part of a job progress stream.
	// Zero-indexed; gaps indicate dropped events.
	sequence_number?: int & >=0

	// progress_events_dropped records how many intermediate progress events were
	// discarded before this terminal-state audit event per ADR-0040.
	progress_events_dropped?: int & >=0

	// simd_tier records which SIMD tier was on the critical path per ADR-0043.
	// Present on startup audit events and per-tool events where SIMD accelerated work.
	simd_tier?: "avx512" | "avx2" | "sse42" | "sse2" | "neon" | "portable"

	// walker_tier records the directory-walker capability tier per ADR-0042.
	walker_tier?: string

	// watcher_tier records the filesystem-watcher capability tier per ADR-0042.
	watcher_tier?: string

	// jail_tier records the path-jail capability tier per ADR-0042.
	jail_tier?: string

	// hash_tier records the cryptographic-hash capability tier per ADR-0042.
	hash_tier?: string

	// stat_tier records the stat/metadata capability tier per ADR-0042.
	stat_tier?: string
}
