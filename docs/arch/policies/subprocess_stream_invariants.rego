# package substrate.subprocess_stream_invariants
#
# Validates #StreamChunk and notifications/progress event fields for the
# subprocess stream-multiplex protocol per ADR-0054.
# Enforced at policy-evaluation time (CI conftest gate) and at runtime
# inside the substrate-subprocess adapter on every chunk emission.
#
# Cross-references:
#   ADR-0054 — subprocess stdout/stderr stream multiplex via notifications/progress
#   ADR-0040 — job control-plane (UUIDv7 progress_token triple-equality)
#   ADR-0007 — tool card narrative arc (structuredContent envelope)
#
# Input shape:
#   {
#     "streamChunk": {
#       "job_id":        "<UUIDv7 hyphenated>",
#       "stream":        "stdout" | "stderr",
#       "seq":           <int>,
#       "chunk_base64":  "<string>",
#       "chunk_bytes":   <int>,
#       "byte_offset":   <int>,
#       "timestamp":     "<RFC 3339 string>"
#     },
#     "progressToken": "<UUIDv7 hyphenated>"   // optional; validated when present
#   }
#
# Test vectors (inline):
#
#   PASS — minimal valid chunk
#   input = {"streamChunk": {"job_id": "01960000-0000-7000-8000-000000000001",
#     "stream": "stdout", "seq": 0, "chunk_base64": "aGVsbG8=",
#     "chunk_bytes": 5, "byte_offset": 0, "timestamp": "2026-05-24T00:00:00Z"},
#     "progressToken": "01960000-0000-7000-8000-000000000001"}
#
#   FAIL — seq < 0
#   input = {"streamChunk": {"job_id": "01960000-0000-7000-8000-000000000001",
#     "stream": "stdout", "seq": -1, "chunk_base64": "", "chunk_bytes": 0,
#     "byte_offset": 0, "timestamp": "2026-05-24T00:00:00Z"}}
#   expected deny contains: "seq must be >= 0"
#
#   FAIL — stream not in {"stdout","stderr"}
#   input = {"streamChunk": {"job_id": "01960000-0000-7000-8000-000000000001",
#     "stream": "stdin", "seq": 0, "chunk_base64": "", "chunk_bytes": 0,
#     "byte_offset": 0, "timestamp": "2026-05-24T00:00:00Z"}}
#   expected deny contains: "stream must be 'stdout' or 'stderr'"
#
#   FAIL — chunk_bytes > 4096
#   input = {"streamChunk": {"job_id": "01960000-0000-7000-8000-000000000001",
#     "stream": "stdout", "seq": 0, "chunk_base64": "", "chunk_bytes": 4097,
#     "byte_offset": 0, "timestamp": "2026-05-24T00:00:00Z"}}
#   expected deny contains: "chunk_bytes 4097 exceeds the maximum of 4096"
#
#   FAIL — progressToken not matching UUIDv7 hyphenated format
#   input = {"streamChunk": {"job_id": "01960000-0000-7000-8000-000000000001",
#     "stream": "stdout", "seq": 0, "chunk_base64": "", "chunk_bytes": 0,
#     "byte_offset": 0, "timestamp": "2026-05-24T00:00:00Z"},
#     "progressToken": "not-a-uuid"}
#   expected deny contains: "progress_token must be a UUIDv7"

package substrate.subprocess_stream_invariants

import rego.v1

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

# Valid stream values per ADR-0054.
_valid_streams := {"stdout", "stderr"}

# Hard cap on decoded byte count per chunk per ADR-0054 §"Tokio Task Architecture".
_max_chunk_bytes := 4096

# UUIDv7 hyphenated format pattern per ADR-0040.
_uuidv7_pattern := `^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`

# ---------------------------------------------------------------------------
# Invariant 1: seq MUST be >= 0 (zero-based monotonic sequence number)
# ---------------------------------------------------------------------------

deny contains msg if {
	input.streamChunk.seq < 0
	msg := sprintf(
		"seq must be >= 0; got %d",
		[input.streamChunk.seq],
	)
}

# ---------------------------------------------------------------------------
# Invariant 2: stream MUST be "stdout" or "stderr"
# ---------------------------------------------------------------------------

deny contains msg if {
	stream := input.streamChunk.stream
	not _valid_streams[stream]
	msg := sprintf(
		"stream must be 'stdout' or 'stderr'; got '%s'",
		[stream],
	)
}

# ---------------------------------------------------------------------------
# Invariant 3: chunk_bytes MUST NOT exceed 4096 (ADR-0054 hard cap)
# ---------------------------------------------------------------------------

deny contains msg if {
	bytes := input.streamChunk.chunk_bytes
	bytes > _max_chunk_bytes
	msg := sprintf(
		"chunk_bytes %d exceeds the maximum of %d (4 KiB hard cap per ADR-0054)",
		[bytes, _max_chunk_bytes],
	)
}

# ---------------------------------------------------------------------------
# Invariant 4: progressToken, when present, MUST match the UUIDv7 hyphenated format
# ---------------------------------------------------------------------------

deny contains msg if {
	token := input.progressToken
	token != null
	not regex.match(_uuidv7_pattern, token)
	msg := sprintf(
		"progress_token must be a UUIDv7 in hyphenated form (e.g. xxxxxxxx-xxxx-7xxx-[89ab]xxx-xxxxxxxxxxxx); got '%s'",
		[token],
	)
}

# ---------------------------------------------------------------------------
# allow — true only when all deny rules produce no messages
# ---------------------------------------------------------------------------

default allow := false

allow if count(deny) == 0
