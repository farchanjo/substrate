# package substrate.wait_timeout_invariants
#
# Validates that every tool schema declaring a `wait_ms` long-poll property
# also declares a non-zero `default` and respects the server-side cap per
# ADR-0059. Enforced at policy-evaluation time (CI conftest gate) when
# rendering tool schemas (`schema_subprocess_result`, `schema_job_result`,
# `schema_subprocess_search`) into the per-tool JSON schema artifact.
#
# Cross-references:
#   ADR-0059 — universal wait/timeout enforcement
#   ADR-0040 — async job control-plane (long-poll cap)
#   ADR-0052 — subprocess execution architecture
#   ADR-0057 — subprocess output pagination and search
#
# Input shape (rendered JSON schema for a single tool):
#   {
#     "tool":   "<tool_name>",
#     "schema": {
#       "type": "object",
#       "properties": {
#         "wait_ms": {
#           "type":    "integer",
#           "minimum": 0,
#           "default": 5000,
#           "maximum": 30000          # optional but recommended
#         },
#         ...
#       }
#     },
#     "config": {
#       "result_max_wait_ms":     30000,
#       "result_default_wait_ms": 5000
#     }
#   }
#
# Test vectors (inline):
#
#   PASS — minimal valid wait_ms (default 5000, cap 30000)
#   input = {
#     "tool": "subprocess.result",
#     "schema": {"properties": {"wait_ms": {"type": "integer", "minimum": 0, "default": 5000}}},
#     "config": {"result_max_wait_ms": 30000, "result_default_wait_ms": 5000}
#   }
#
#   FAIL — wait_ms with default 0
#   expected deny contains: "wait_ms.default must be > 0"
#
#   FAIL — wait_ms with maximum above server cap
#   schema.properties.wait_ms.maximum = 60000
#   config.result_max_wait_ms          = 30000
#   expected deny contains: "wait_ms.maximum must be <= result_max_wait_ms"
#
#   FAIL — wait_ms.default differs from config.result_default_wait_ms (drift)
#   expected deny contains: "wait_ms.default must match config.result_default_wait_ms"

package substrate.wait_timeout_invariants

import rego.v1

# ---------------------------------------------------------------------------
# Invariant 1: any wait_ms property MUST declare a non-zero default
# ---------------------------------------------------------------------------

deny contains msg if {
	wait := input.schema.properties.wait_ms
	wait.default == 0
	msg := sprintf(
		"%s: wait_ms.default must be > 0 per ADR-0059; got 0",
		[input.tool],
	)
}

deny contains msg if {
	wait := input.schema.properties.wait_ms
	not wait.default
	msg := sprintf(
		"%s: wait_ms.default is required per ADR-0059",
		[input.tool],
	)
}

# ---------------------------------------------------------------------------
# Invariant 2: wait_ms.maximum, when declared, MUST be <= result_max_wait_ms
# ---------------------------------------------------------------------------

deny contains msg if {
	wait := input.schema.properties.wait_ms
	cap := input.config.result_max_wait_ms
	wait.maximum > cap
	msg := sprintf(
		"%s: wait_ms.maximum (%d) must be <= result_max_wait_ms (%d) per ADR-0059",
		[input.tool, wait.maximum, cap],
	)
}

# ---------------------------------------------------------------------------
# Invariant 3: wait_ms.default MUST match config.result_default_wait_ms
# (prevents schema drift from the configured default)
# ---------------------------------------------------------------------------

deny contains msg if {
	wait := input.schema.properties.wait_ms
	cfg_default := input.config.result_default_wait_ms
	wait.default != cfg_default
	wait.default > 0
	msg := sprintf(
		"%s: wait_ms.default (%d) must match config.result_default_wait_ms (%d) per ADR-0059",
		[input.tool, wait.default, cfg_default],
	)
}

# ---------------------------------------------------------------------------
# Invariant 4: wait_ms.minimum MUST be 0 (explicit fast-return opt-out)
# ---------------------------------------------------------------------------

deny contains msg if {
	wait := input.schema.properties.wait_ms
	wait.minimum != 0
	msg := sprintf(
		"%s: wait_ms.minimum must be 0 to preserve explicit fast-return opt-out; got %d",
		[input.tool, wait.minimum],
	)
}

# ---------------------------------------------------------------------------
# allow — true only when all deny rules produce no messages
# ---------------------------------------------------------------------------

default allow := false

allow if count(deny) == 0
