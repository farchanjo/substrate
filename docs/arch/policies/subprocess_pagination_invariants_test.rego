package substrate.subprocess_pagination_invariants_test

import rego.v1

import data.substrate.subprocess_pagination_invariants

# ---------------------------------------------------------------------------
# Shared fixture helpers
# ---------------------------------------------------------------------------

# A minimal valid pagination block (offset=0, page_size=100, no order).
_valid_pagination := {
	"offset":    0,
	"page_size": 100,
}

# A minimal valid search block (pattern non-empty, no streams).
_valid_search := {"pattern": "^ERROR"}

# ---------------------------------------------------------------------------
# test_offset_negative_denied
# Invariant 1: offset < 0 must be denied.
# FAIL vector: offset = -1
# ---------------------------------------------------------------------------

test_offset_negative_denied if {
	result := subprocess_pagination_invariants.deny with input as {
		"pagination": object.union(_valid_pagination, {"offset": -1}),
	}
	some msg
	result[msg]
	contains(msg, "pagination.offset must be >= 0")
}

# ---------------------------------------------------------------------------
# test_offset_zero_allowed
# Invariant 1 inverse: offset = 0 must pass.
# PASS vector: offset = 0
# ---------------------------------------------------------------------------

test_offset_zero_allowed if {
	count(subprocess_pagination_invariants.deny) == 0 with input as {
		"pagination": _valid_pagination,
	}
}

# ---------------------------------------------------------------------------
# test_offset_large_allowed
# Invariant 1 inverse: large positive offset must pass.
# PASS vector: offset = 99999
# ---------------------------------------------------------------------------

test_offset_large_allowed if {
	count(subprocess_pagination_invariants.deny) == 0 with input as {
		"pagination": object.union(_valid_pagination, {"offset": 99999}),
	}
}

# ---------------------------------------------------------------------------
# test_page_size_zero_denied
# Invariant 2: page_size = 0 must be denied.
# FAIL vector: page_size = 0
# ---------------------------------------------------------------------------

test_page_size_zero_denied if {
	result := subprocess_pagination_invariants.deny with input as {
		"pagination": object.union(_valid_pagination, {"page_size": 0}),
	}
	some msg
	result[msg]
	contains(msg, "pagination.page_size must be in [1, 10000]")
}

# ---------------------------------------------------------------------------
# test_page_size_over_cap_denied
# Invariant 2: page_size > 10000 must be denied.
# FAIL vector: page_size = 10001
# ---------------------------------------------------------------------------

test_page_size_over_cap_denied if {
	result := subprocess_pagination_invariants.deny with input as {
		"pagination": object.union(_valid_pagination, {"page_size": 10001}),
	}
	some msg
	result[msg]
	contains(msg, "pagination.page_size must be in [1, 10000]")
}

# ---------------------------------------------------------------------------
# test_page_size_one_allowed
# Invariant 2 inverse: page_size = 1 must pass.
# PASS vector: page_size = 1
# ---------------------------------------------------------------------------

test_page_size_one_allowed if {
	count(subprocess_pagination_invariants.deny) == 0 with input as {
		"pagination": object.union(_valid_pagination, {"page_size": 1}),
	}
}

# ---------------------------------------------------------------------------
# test_page_size_at_cap_allowed
# Invariant 2 inverse: page_size = 10000 (boundary) must pass.
# PASS vector: page_size = 10000
# ---------------------------------------------------------------------------

test_page_size_at_cap_allowed if {
	count(subprocess_pagination_invariants.deny) == 0 with input as {
		"pagination": object.union(_valid_pagination, {"page_size": 10000}),
	}
}

# ---------------------------------------------------------------------------
# test_order_invalid_denied
# Invariant 3: order not in {Tail, Head} must be denied.
# FAIL vector: order = "Middle"
# ---------------------------------------------------------------------------

test_order_invalid_denied if {
	result := subprocess_pagination_invariants.deny with input as {
		"pagination": object.union(_valid_pagination, {"order": "Middle"}),
	}
	some msg
	result[msg]
	contains(msg, "pagination.order must be Tail or Head")
}

# ---------------------------------------------------------------------------
# test_order_tail_allowed
# Invariant 3 inverse: order = "Tail" must pass.
# PASS vector: order = "Tail"
# ---------------------------------------------------------------------------

test_order_tail_allowed if {
	count(subprocess_pagination_invariants.deny) == 0 with input as {
		"pagination": object.union(_valid_pagination, {"order": "Tail"}),
	}
}

# ---------------------------------------------------------------------------
# test_order_head_allowed
# Invariant 3 inverse: order = "Head" must pass.
# PASS vector: order = "Head"
# ---------------------------------------------------------------------------

test_order_head_allowed if {
	count(subprocess_pagination_invariants.deny) == 0 with input as {
		"pagination": object.union(_valid_pagination, {"order": "Head"}),
	}
}

# ---------------------------------------------------------------------------
# test_pattern_empty_denied
# Invariant 4: pattern length = 0 must be denied.
# FAIL vector: pattern = ""
# ---------------------------------------------------------------------------

test_pattern_empty_denied if {
	result := subprocess_pagination_invariants.deny with input as {
		"search": {"pattern": ""},
	}
	some msg
	result[msg]
	contains(msg, "search.pattern length must be in [1, 1024]")
}

# ---------------------------------------------------------------------------
# test_pattern_too_long_denied
# Invariant 4: pattern length > 1024 must be denied.
# FAIL vector: pattern = 1025 * "x" (represented via a known 1025-char string)
# ---------------------------------------------------------------------------

test_pattern_too_long_denied if {
	# Build a 1025-character string of "x" so count(pattern) > 1024.
	long_pattern := concat("", [c | _ := numbers.range(1, 1025)[_]; c := "x"])
	result := subprocess_pagination_invariants.deny with input as {
		"search": {"pattern": long_pattern},
	}
	some msg
	result[msg]
	contains(msg, "search.pattern length must be in [1, 1024]")
}

# ---------------------------------------------------------------------------
# test_pattern_valid_allowed
# Invariant 4 inverse: pattern length = 1 must pass.
# PASS vector: pattern = "x"
# ---------------------------------------------------------------------------

test_pattern_valid_allowed if {
	count(subprocess_pagination_invariants.deny) == 0 with input as {
		"search": _valid_search,
	}
}

# ---------------------------------------------------------------------------
# test_pattern_at_cap_allowed
# Invariant 4 inverse: pattern length = 1024 must pass.
# PASS vector: 1024 "a" characters
# ---------------------------------------------------------------------------

test_pattern_at_cap_allowed if {
	count(subprocess_pagination_invariants.deny) == 0 with input as {
		"search": {"pattern": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
	}
}

# ---------------------------------------------------------------------------
# test_streams_invalid_element_denied
# Invariant 5: stream element not in {stdout, stderr} must be denied.
# FAIL vector: streams = ["stdout", "stdin"]
# ---------------------------------------------------------------------------

test_streams_invalid_element_denied if {
	result := subprocess_pagination_invariants.deny with input as {
		"search": {"pattern": "foo", "streams": ["stdout", "stdin"]},
	}
	some msg
	result[msg]
	contains(msg, "search.streams element must be stdout or stderr")
}

# ---------------------------------------------------------------------------
# test_streams_stdout_only_allowed
# Invariant 5 inverse: streams = ["stdout"] must pass.
# PASS vector: streams = ["stdout"]
# ---------------------------------------------------------------------------

test_streams_stdout_only_allowed if {
	count(subprocess_pagination_invariants.deny) == 0 with input as {
		"search": {"pattern": "foo", "streams": ["stdout"]},
	}
}

# ---------------------------------------------------------------------------
# test_streams_stderr_only_allowed
# Invariant 5 inverse: streams = ["stderr"] must pass.
# PASS vector: streams = ["stderr"]
# ---------------------------------------------------------------------------

test_streams_stderr_only_allowed if {
	count(subprocess_pagination_invariants.deny) == 0 with input as {
		"search": {"pattern": "foo", "streams": ["stderr"]},
	}
}

# ---------------------------------------------------------------------------
# test_happy_path_combined_allow
# All invariants satisfied with pagination + search fields: allow = true.
# PASS vector: valid pagination + valid search pattern + valid streams
# ---------------------------------------------------------------------------

test_happy_path_combined_allow if {
	subprocess_pagination_invariants.allow with input as {
		"pagination": {
			"offset":    0,
			"page_size": 50,
			"order":     "Head",
		},
		"search": {
			"pattern": "^ERROR",
			"streams": ["stdout", "stderr"],
		},
	}
}
