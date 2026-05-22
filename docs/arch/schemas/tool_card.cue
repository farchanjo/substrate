// DDD role: ValueObject
package schemas

// #ToolCard is the canonical human-readable narrative arc for a tool.
// All six sections follow the USE/DOES/ARGS/RETURNS/NEXT/AVOID grammar.
// The total token budget across all text fields MUST NOT exceed 180 tokens.
#ToolCard: {
	// use: one sentence on when a caller should reach for this tool.
	use: string

	// does: one sentence on what the tool actually performs.
	does: string

	// args: ordered list of argument descriptors.
	args: [...#ToolArg]

	// returns: one sentence describing the successful output shape.
	returns: string

	// next: suggested follow-up tool names (0-3 items preferred).
	next: [...string]

	// avoid: anti-patterns or misuse scenarios to warn callers about.
	avoid: string

	// total_token_budget_max is a spec constant; never exceed 180 tokens across all fields.
	total_token_budget_max: 180
}

// #ArgType enumerates the primitive argument types recognised by the substrate runtime.
#ArgType: "string" | "number" | "boolean" | "array" | "object" | "path"

// #Hints carries the structuredContent hints map included in every tool response.
// Added 2026-05-21 per ADR-0007 amendment. Six job orchestration keys for push/pull
// dual channel (per ADR-0040); two diagnostic tier annotations (per ADR-0042/0043).
#Hints: {
	// next_action_suggested is a free-form string advising the caller of a follow-up step.
	next_action_suggested?: string

	// alternative_tool names an alternative tool if this call is inappropriate.
	alternative_tool?: string

	// confirm_destructive is set when the operation requires explicit elicitation.
	confirm_destructive?: bool

	// quota_status is a human-readable quota summary (e.g. "14/16 jobs used").
	quota_status?: string

	// error_recovery contains a short recovery hint when the call results in an error.
	error_recovery?: string

	// job_id is the UUIDv7 (base32 26-char ULID form) of the created or reused job (per ADR-0040).
	job_id?: =~"^[0-9A-HJKMNP-TV-Z]{26}$"

	// job_state is the current JobState value for this job (per ADR-0040).
	job_state?: "pending" | "running" | "succeeded" | "failed" | "cancelled" | "timed_out"

	// job_progress_pct is the completion percentage 0-100; absent for terminal or not-yet-started jobs (per ADR-0040).
	job_progress_pct?: int & >=0 & <=100

	// polling_endpoint names the control-plane tool to call for status or result retrieval (per ADR-0040).
	polling_endpoint?: "job.status" | "job.result"

	// estimated_completion_ms is a best-effort completion estimate in milliseconds; absent when unknown (per ADR-0040).
	estimated_completion_ms?: int & >=0

	// sequence_number is the last known monotonic sequence counter for the associated job (per ADR-0040).
	sequence_number?: int & >=0

	// simd_tier_used is a diagnostic annotation identifying the SIMD tier chosen at runtime (per ADR-0042/ADR-0043).
	simd_tier_used?: "avx512" | "avx2" | "sse42" | "sse2" | "neon" | "portable"

	// walker_tier_used is a diagnostic annotation identifying the DirWalker tier chosen at runtime (per ADR-0042).
	walker_tier_used?: string
}

// #ToolArg describes a single named argument accepted by a tool.
#ToolArg: {
	// name must be a valid identifier (lowercase snake_case).
	name: string & =~"^[a-z][a-z0-9_]*$"

	// type is the JSON-compatible primitive type of this argument.
	type: #ArgType

	// default is the value used when the argument is absent; omit if required.
	default?: string | number | bool

	// purpose is a brief (≤25 token) description of the argument's role.
	purpose: string
}
