// DDD role: AggregateRoot
package schemas

import "strings"

// #ToolNamespace enumerates the nine stable tool namespaces.
// The "job" namespace was added 2026-05-21 per ADR-0040 (async job control-plane).
// The "subprocess" namespace was added per ADR-0052 and "net" per ADR-0058.
// The "launch" namespace was added per ADR-0069 (declarative process orchestration).
#ToolNamespace: "fs" | "proc" | "sys" | "text" | "archive" | "job" | "subprocess" | "net" | "launch"

// #ToolBucket classifies every tool into a dispatch bucket per ADR-0040.
// A_sync_inline: snapshot-instant, always synchronous (e.g. sys.uname, sys.info).
// B_auto_mode: inline if below threshold, promoted to async job if above.
// C_always_async: job dispatch is mandatory, no streaming (e.g. archive.tar.create).
// D_sync_side_effect: fast commit, audit fire-and-forget (e.g. fs.mkdir, proc.signal).
// E_always_async_streaming: always async with streaming progress (subprocess.spawn
// only) per the ADR-0040 2026-05-24 amendment and ADR-0052/ADR-0054.
#ToolBucket: "A_sync_inline" | "B_auto_mode" | "C_always_async" | "D_sync_side_effect" | "E_always_async_streaming"

// #ToolAnnotations carries MCP hint booleans that guide client behavior.
// Defaults represent the safest posture (writable, non-destructive, non-idempotent, closed-world).
#ToolAnnotations: {
	// readOnlyHint signals that this tool never mutates state.
	readOnlyHint: bool | *false

	// destructiveHint signals that this tool may cause irreversible side effects.
	destructiveHint: bool | *false

	// idempotentHint signals that repeated identical calls produce identical outcomes.
	idempotentHint: bool | *false

	// openWorldHint signals that this tool may access resources outside the allowlist.
	openWorldHint: bool | *false
}

// #ToolSpec is the aggregate root for a single registered MCP tool.
// It binds identity, schema contracts, and behavioral annotations together.
#ToolSpec: {
	// name must follow <namespace>_<snake_case> and be non-empty (wire form per
	// ADR-0062). "job" namespace added 2026-05-21 per ADR-0040; "subprocess" per
	// ADR-0052; "net" per ADR-0058; "launch" per ADR-0069.
	name: string & =~"^(fs|proc|sys|text|archive|job|subprocess|net|launch)_[a-z][a-z0-9_]*$"

	// description is the thin one-liner shown verbatim in client tool listings.
	// Capped at 100 chars per the ADR-0007 2026-05-22 amendment (MCP + skill
	// synergy): no USE/DOES/ARGS/RETURNS/NEXT/AVOID labels; the full lookup
	// reference lives in the companion substrate skill.
	description: string & strings.MaxRunes(100)

	// namespace is derived from the name prefix; kept explicit for query convenience.
	namespace: #ToolNamespace

	// input_schema is a JSON Schema object (opaque map) describing accepted arguments.
	input_schema: {[string]: _}

	// output_schema is a JSON Schema object describing structured return values.
	output_schema: {[string]: _}

	// annotations controls MCP client hints for this tool.
	annotations: #ToolAnnotations

	// bucket classifies this tool into a dispatch bucket per ADR-0040.
	// Bucket assignment is static (compile-time) except for Bucket B whose actual
	// dispatch path (inline vs. async job) is resolved at runtime based on input size.
	bucket: #ToolBucket
}
