// DDD role: ValueObject
package schemas

// #JailedPath represents a filesystem path that has been validated as safe.
// Construction of a #JailedPath implies the runtime has verified both invariants below.
#JailedPath: {
	// absolute is the fully-resolved, canonical absolute path (no '..' or symlink escapes).
	absolute: string & =~"^/"

	// within_allowlist_root confirms that absolute starts with one of the configured
	// security_policy.allowlist.roots entries. Must be true; false values are rejected
	// at construction time by the jailing layer.
	within_allowlist_root: true
}

// #PageCursor is an opaque, base64-encoded pagination cursor.
// Callers MUST treat the payload as opaque; never construct cursors manually.
#PageCursor: {
	// encoding is always "base64" for this version of the protocol.
	encoding: "base64"

	// opaque_payload is the base64url-encoded (no-padding) continuation token.
	opaque_payload: string & =~"^[A-Za-z0-9_-]+$"
}

// #ProgressToken tracks incremental progress for long-running tools.
// Tools that support streaming progress emit this alongside partial results.
#ProgressToken: {
	// token is the opaque progress identifier issued by the runtime.
	token: string

	// total_known is the estimated total unit count; absent when unknown.
	total_known?: uint & >=1
}

// #ToolResult is the canonical envelope returned by every substrate tool.
// It bundles a human-readable summary, optional machine-readable content, and hints.
#ToolResult: {
	// content_text_summary is a concise, redacted human-readable description
	// of what the tool produced. Shown verbatim in MCP client UIs.
	content_text_summary: string

	// structured_content carries machine-readable output (JSON-compatible map).
	// May be empty ({}) when no structured data is produced.
	structured_content: {[string]: _}

	// hints provides optional guidance to the caller about follow-up actions,
	// quota state, or error recovery. Keys are drawn from #HintKey.
	hints: #HintsMap
}

// Cross-BC value objects shared between the async-job BC and other bounded contexts.
// These mirror definitions in job.cue and simd_capability.cue; the shared kernel
// owns the canonical regexes used by adapters that must not import BC-specific schemas.

// #JobId is a ULID (Crockford base32, 26 characters) uniquely identifying an async job.
// Per ADR-0040; time-ordered, monotonic within a millisecond.
#JobId: string & =~"^[0-9A-HJKMNP-TV-Z]{26}$"

// #CorrelationId is an alias for #JobId used when a job ID serves as the
// request-chain correlation identifier per ADR-0038 amendment.
#CorrelationId: #JobId

// #ClientId identifies the originating MCP client session per ADR-0040.
// Pattern: alphanumeric with dots, underscores, and hyphens; 1–64 characters.
#ClientId: string & =~"^[A-Za-z0-9._-]{1,64}$"

// #SimdTier enumerates the hardware SIMD capability tiers detected at startup per ADR-0043.
// Tiers are ordered weakest-to-strongest: portable < sse2 < sse42 < avx2 < avx512.
// "neon" is the ARM equivalent of avx2.
#SimdTier: "avx512" | "avx2" | "sse42" | "sse2" | "neon" | "portable"

// #PageSize is the shared pagination value object enforced at the domain port
// boundary per ADR-0060. The domain range is 1..=10000 with a default of 50.
// Handler-level per-tool caps (e.g. 500 for fs.find/proc.list/text.search per
// ADR-0008) clamp the requested value DOWN after this domain validation; they
// never widen the range. Line- and record-oriented tools (subprocess.result,
// subprocess.search, net.tcp_list/udp_list) use a default of 100 (the
// PageSize::DEFAULT_PAGINATION associated constant) rather than 50, while still
// honoring the same 1..=10000 domain bounds.
#PageSize: int & >=1 & <=10000 | *50
