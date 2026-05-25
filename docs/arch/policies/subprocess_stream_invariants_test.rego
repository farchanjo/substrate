package substrate.subprocess_stream_invariants_test

import rego.v1

import data.substrate.subprocess_stream_invariants

# ---------------------------------------------------------------------------
# Shared fixture helpers
# ---------------------------------------------------------------------------

# A minimal valid #StreamChunk for use as a base in each test.
_valid_chunk := {
	"job_id": "01960000-0000-7000-8000-000000000001",
	"stream": "stdout",
	"seq": 0,
	"chunk_base64": "aGVsbG8=",
	"chunk_bytes": 5,
	"byte_offset": 0,
	"timestamp": "2026-05-24T00:00:00Z",
}

# A valid UUIDv7 progress_token that matches job_id above.
_valid_token := "01960000-0000-7000-8000-000000000001"

# ---------------------------------------------------------------------------
# test_seq_negative_denied
# Invariant 1: seq < 0 must be denied.
# FAIL vector: seq = -1
# ---------------------------------------------------------------------------

test_seq_negative_denied if {
	result := subprocess_stream_invariants.deny with input as {
		"streamChunk": object.union(_valid_chunk, {"seq": -1}),
	}
	some msg
	result[msg]
	contains(msg, "seq must be >= 0")
}

# ---------------------------------------------------------------------------
# test_seq_zero_allowed
# Invariant 1 inverse: seq = 0 must pass.
# PASS vector: seq = 0
# ---------------------------------------------------------------------------

test_seq_zero_allowed if {
	count(subprocess_stream_invariants.deny) == 0 with input as {
		"streamChunk": _valid_chunk,
		"progressToken": _valid_token,
	}
}

# ---------------------------------------------------------------------------
# test_stream_invalid_denied
# Invariant 2: stream not in {"stdout","stderr"} must be denied.
# FAIL vector: stream = "stdin"
# ---------------------------------------------------------------------------

test_stream_invalid_denied if {
	result := subprocess_stream_invariants.deny with input as {
		"streamChunk": object.union(_valid_chunk, {"stream": "stdin"}),
	}
	some msg
	result[msg]
	contains(msg, "stream must be 'stdout' or 'stderr'")
}

# ---------------------------------------------------------------------------
# test_stream_stderr_allowed
# Invariant 2 inverse: stream = "stderr" must pass.
# PASS vector: stream = "stderr"
# ---------------------------------------------------------------------------

test_stream_stderr_allowed if {
	count(subprocess_stream_invariants.deny) == 0 with input as {
		"streamChunk": object.union(_valid_chunk, {"stream": "stderr"}),
		"progressToken": _valid_token,
	}
}

# ---------------------------------------------------------------------------
# test_chunk_bytes_over_cap_denied
# Invariant 3: chunk_bytes > 4096 must be denied.
# FAIL vector: chunk_bytes = 4097
# ---------------------------------------------------------------------------

test_chunk_bytes_over_cap_denied if {
	result := subprocess_stream_invariants.deny with input as {
		"streamChunk": object.union(_valid_chunk, {"chunk_bytes": 4097}),
	}
	some msg
	result[msg]
	contains(msg, "chunk_bytes 4097 exceeds the maximum of 4096")
}

# ---------------------------------------------------------------------------
# test_chunk_bytes_at_cap_allowed
# Invariant 3 inverse: chunk_bytes = 4096 must pass.
# PASS vector: chunk_bytes = 4096
# ---------------------------------------------------------------------------

test_chunk_bytes_at_cap_allowed if {
	count(subprocess_stream_invariants.deny) == 0 with input as {
		"streamChunk": object.union(_valid_chunk, {"chunk_bytes": 4096}),
		"progressToken": _valid_token,
	}
}

# ---------------------------------------------------------------------------
# test_progress_token_invalid_denied
# Invariant 4: progressToken not matching UUIDv7 hyphenated format must be denied.
# FAIL vector: progressToken = "not-a-uuid"
# ---------------------------------------------------------------------------

test_progress_token_invalid_denied if {
	result := subprocess_stream_invariants.deny with input as {
		"streamChunk": _valid_chunk,
		"progressToken": "not-a-uuid",
	}
	some msg
	result[msg]
	contains(msg, "progress_token must be a UUIDv7")
}

# ---------------------------------------------------------------------------
# test_progress_token_v4_denied
# Invariant 4: a UUIDv4 (version digit = 4, not 7) must be denied.
# FAIL vector: progressToken = "550e8400-e29b-41d4-a716-446655440000"
# ---------------------------------------------------------------------------

test_progress_token_v4_denied if {
	result := subprocess_stream_invariants.deny with input as {
		"streamChunk": _valid_chunk,
		"progressToken": "550e8400-e29b-41d4-a716-446655440000",
	}
	some msg
	result[msg]
	contains(msg, "progress_token must be a UUIDv7")
}

# ---------------------------------------------------------------------------
# test_progress_token_valid_allowed
# Invariant 4 inverse: valid UUIDv7 progress_token must pass.
# PASS vector: progressToken = "01960000-0000-7000-8000-000000000001"
# ---------------------------------------------------------------------------

test_progress_token_valid_allowed if {
	count(subprocess_stream_invariants.deny) == 0 with input as {
		"streamChunk": _valid_chunk,
		"progressToken": _valid_token,
	}
}

# ---------------------------------------------------------------------------
# test_progress_token_absent_allowed
# Invariant 4: absent progressToken field must not trigger the deny rule.
# PASS vector: no progressToken key in input
# ---------------------------------------------------------------------------

test_progress_token_absent_allowed if {
	count(subprocess_stream_invariants.deny) == 0 with input as {
		"streamChunk": _valid_chunk,
	}
}

# ---------------------------------------------------------------------------
# test_happy_path_allow_rule_true
# All invariants satisfied with all fields at boundary values: allow = true.
# PASS vector: full valid chunk at max chunk_bytes with valid progressToken
# ---------------------------------------------------------------------------

test_happy_path_allow_rule_true if {
	subprocess_stream_invariants.allow with input as {
		"streamChunk": {
			"job_id": "01960000-0000-7000-8000-000000000001",
			"stream": "stderr",
			"seq": 255,
			"chunk_base64": "AAEC",
			"chunk_bytes": 4096,
			"byte_offset": 1048576,
			"timestamp": "2026-05-24T12:34:56.789Z",
		},
		"progressToken": "01960000-0000-7000-8000-000000000001",
	}
}
