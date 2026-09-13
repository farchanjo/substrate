// DDD role: ValueObject
package schemas

import "strings"

// #ToolCard is the canonical wire description for a registered MCP tool.
// Per the ADR-0007 2026-05-22 amendment (MCP + skill synergy) the description is a
// thin one-liner capped at 100 characters; the former 180-token six-field
// USE/DOES/ARGS/RETURNS/NEXT/AVOID grammar was retired and those labels are
// forbidden on the wire. The full lookup reference (argument tables, follow-up
// suggestions, anti-patterns) lives in the companion substrate skill, not in the
// tool listing.
#ToolCard: {
	// description is the thin one-liner shown verbatim in client tool listings.
	// Hard cap: 100 characters (tool_description_cap). No multi-field template.
	description: string & strings.MaxRunes(100)

	// args: ordered list of argument descriptors. Retained for skill-side reference
	// generation; not emitted in the on-the-wire tool description.
	args: [...#ToolArg]
}

// #ArgType enumerates the primitive argument types recognised by the substrate runtime.
#ArgType: "string" | "number" | "boolean" | "array" | "object" | "path"

// #Hints carries the structuredContent hints map included in every tool response.
// Added 2026-05-21 per ADR-0007 amendment. Six job orchestration keys for push/pull
// dual channel (per ADR-0040); two diagnostic tier annotations (per ADR-0042/0043).
// The keys are grouped into the four #Hints* fragments below; each fragment is
// embedded, so the hints map stays a single flat map.
#Hints: {
	#HintsCallerGuidance
	#HintsJobChannel
	#HintsLaunchStack
	#HintsTierDiagnostics
}

// #HintsCallerGuidance is the caller-facing fragment of #Hints: what the caller
// should do next, and which safety gate applies to this response.
#HintsCallerGuidance: {
	// next_action_suggested is a free-form string advising the caller of a follow-up step.
	next_action_suggested?: #ShortText

	// alternative_tool names an alternative tool if this call is inappropriate.
	alternative_tool?: #ToolName

	// confirm_destructive is set when the operation requires explicit elicitation.
	confirm_destructive?: #Flag

	// quota_status is a human-readable quota summary (e.g. "14/16 jobs used").
	quota_status?: #ShortText

	// error_recovery contains a short recovery hint when the call results in an error.
	error_recovery?: #ShortText
}

// #HintsJobChannel is the async-job fragment of #Hints: the push/pull channel
// state for the job backing this response per ADR-0040.
#HintsJobChannel: {
	// job_id is the UUIDv7 (base32 26-char ULID form) of the created or reused job (per ADR-0040).
	job_id?: #JobId

	// job_state is the current JobState value for this job (per ADR-0040).
	job_state?: #JobState

	// job_progress_pct is the completion percentage 0-100; absent for terminal or not-yet-started jobs (per ADR-0040).
	job_progress_pct?: int & >=0 & <=100

	// polling_endpoint names the control-plane tool to call for status or result retrieval
	// (per ADR-0040). Canonical enum #PollingEndpoint includes "launch.status" per ADR-0069.
	polling_endpoint?: #PollingEndpoint

	// estimated_completion_ms is a best-effort completion estimate in milliseconds; absent when unknown (per ADR-0040).
	estimated_completion_ms?: int & >=0

	// sequence_number is the last known monotonic sequence counter for the associated job (per ADR-0040).
	sequence_number?: int & >=0
}

// #HintsLaunchStack is the launch fragment of #Hints, populated by the launch BC only
// (per ADR-0069).
#HintsLaunchStack: {
	// stack_id is the UUIDv7 of the launch Stack a response belongs to (per ADR-0069). Launch BC only.
	stack_id?: =~"^[0-9A-HJKMNP-TV-Z]{26}$"

	// stack_state is the current launch StackState for the associated stack (per ADR-0069). Launch BC only.
	stack_state?: #StackState
}

// #HintsTierDiagnostics is the capability-attribution fragment of #Hints: the runtime
// tier actually exercised by this invocation (per ADR-0042/ADR-0043).
#HintsTierDiagnostics: {
	// simd_tier_used is a diagnostic annotation identifying the SIMD tier chosen at runtime (per ADR-0042/ADR-0043).
	simd_tier_used?: #SimdTier

	// walker_tier_used is a diagnostic annotation identifying the DirWalker tier chosen at runtime (per ADR-0042).
	walker_tier_used?: #WalkerTier
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
	purpose: #ShortText
}