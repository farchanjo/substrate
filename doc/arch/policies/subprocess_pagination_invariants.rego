# package substrate.subprocess_pagination_invariants
#
# Validates #Pagination blocks and #SubprocessSearchRequest fields for the
# subprocess output pagination and search protocol per ADR-0057.
# Enforced at policy-evaluation time (CI conftest gate) and at runtime
# inside the substrate-subprocess adapter on every paginated result or search call.
#
# Cross-references:
#   ADR-0057 — subprocess output pagination and search
#   ADR-0060 — PageSize value object at domain port boundaries (1..=10_000)
#   ADR-0052 — subprocess bounded context decision
#   ADR-0040 — job control-plane (UUIDv7 job_id triple-equality)
#
# Input shape for pagination rules (pagination block inside result or search request):
#   {
#     "pagination": {
#       "offset":    <int>,              // 0-based line offset
#       "page_size": <int>,              // 1..10000
#       "order":     "Tail" | "Head"    // optional, default Tail
#     }
#   }
#
# Input shape for search rules:
#   {
#     "search": {
#       "pattern": "<string>",           // 1..1024 chars
#       "streams": ["stdout","stderr"]   // optional; each element must be stdout or stderr
#     }
#   }
#
# Test vectors (inline):
#
#   PASS — minimal valid pagination (offset=0, page_size=100, order absent)
#   input = {"pagination": {"offset": 0, "page_size": 100}}
#
#   PASS — explicit Head order
#   input = {"pagination": {"offset": 5, "page_size": 50, "order": "Head"}}
#
#   FAIL — offset < 0
#   input = {"pagination": {"offset": -1, "page_size": 10}}
#   expected deny contains: "pagination.offset must be >= 0"
#
#   FAIL — page_size = 0
#   input = {"pagination": {"offset": 0, "page_size": 0}}
#   expected deny contains: "pagination.page_size must be in [1, 10000]"
#
#   FAIL — page_size > 10000
#   input = {"pagination": {"offset": 0, "page_size": 10001}}
#   expected deny contains: "pagination.page_size must be in [1, 10000]"
#
#   FAIL — order not in {Tail, Head}
#   input = {"pagination": {"offset": 0, "page_size": 10, "order": "Middle"}}
#   expected deny contains: "pagination.order must be Tail or Head"
#
#   FAIL — pattern empty
#   input = {"search": {"pattern": ""}}
#   expected deny contains: "search.pattern length must be in [1, 1024]"
#
#   FAIL — pattern too long (1025 chars)
#   input = {"search": {"pattern": "x" * 1025}}  (conceptual)
#   expected deny contains: "search.pattern length must be in [1, 1024]"
#
#   FAIL — stream element not in {stdout, stderr}
#   input = {"search": {"pattern": "foo", "streams": ["stdout", "stdin"]}}
#   expected deny contains: "search.streams element must be stdout or stderr"

package substrate.subprocess_pagination_invariants

import rego.v1

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

_valid_orders := {"Tail", "Head"}

_valid_streams := {"stdout", "stderr"}

_max_page_size := 10000

_max_pattern_length := 1024

# ---------------------------------------------------------------------------
# Invariant 1: pagination.offset MUST be >= 0
# ---------------------------------------------------------------------------

deny contains msg if {
	offset := input.pagination.offset
	offset < 0
	msg := sprintf(
		"subprocess.result/search: pagination.offset must be >= 0; got %d",
		[offset],
	)
}

# ---------------------------------------------------------------------------
# Invariant 2: pagination.page_size MUST be in [1, 10000]
# ---------------------------------------------------------------------------

deny contains msg if {
	page_size := input.pagination.page_size
	page_size < 1
	msg := sprintf(
		"subprocess.result/search: pagination.page_size must be in [1, 10000]; got %d",
		[page_size],
	)
}

deny contains msg if {
	page_size := input.pagination.page_size
	page_size > _max_page_size
	msg := sprintf(
		"subprocess.result/search: pagination.page_size must be in [1, 10000]; got %d",
		[page_size],
	)
}

# ---------------------------------------------------------------------------
# Invariant 3: pagination.order, when present, MUST be Tail or Head
# ---------------------------------------------------------------------------

deny contains msg if {
	order := input.pagination.order
	not _valid_orders[order]
	msg := sprintf(
		"subprocess.result/search: pagination.order must be Tail or Head; got '%s'",
		[order],
	)
}

# ---------------------------------------------------------------------------
# Invariant 4: search.pattern length MUST be in [1, 1024]
# ---------------------------------------------------------------------------

deny contains msg if {
	pattern := input.search.pattern
	count(pattern) == 0
	msg := "subprocess.search: search.pattern length must be in [1, 1024]; got 0"
}

deny contains msg if {
	pattern := input.search.pattern
	count(pattern) > _max_pattern_length
	msg := sprintf(
		"subprocess.search: search.pattern length must be in [1, 1024]; got %d",
		[count(pattern)],
	)
}

# ---------------------------------------------------------------------------
# Invariant 5: search.streams elements, when streams is set, MUST be stdout or stderr
# ---------------------------------------------------------------------------

deny contains msg if {
	stream := input.search.streams[_]
	not _valid_streams[stream]
	msg := sprintf(
		"subprocess.search: search.streams element must be stdout or stderr; got '%s'",
		[stream],
	)
}

# ---------------------------------------------------------------------------
# allow — true only when all deny rules produce no messages
# ---------------------------------------------------------------------------

default allow := false

allow if count(deny) == 0
