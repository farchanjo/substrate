# package substrate.audit_event
#
# Validates structural and semantic invariants for audit events emitted by the
# substrate MCP server. Events are written to the audit log on every tool
# invocation and significant lifecycle transition.
#
# Invariants enforced:
#   - outcome must be in the allowed enum
#   - when outcome ∈ {"error","timeout"}, error_code MUST be present
#   - seq is a non-negative integer
#   - correlation_id matches UUIDv7 (RFC 9562) format
#   - timestamp matches ISO 8601 with Z suffix
#   - tool_name matches the namespaced dot-notation pattern from naming.rego
#   - workflow_id, when present, must also be a UUIDv7
#
# Input shape:
#   {
#     "seq":            0,
#     "correlation_id": "018f4e2a-9b3c-7d1e-a4f5-6789abcdef01",
#     "timestamp":      "2026-05-21T14:30:00.123456Z",
#     "tool_name":      "fs.read",
#     "outcome":        "success",
#     "error_code":     null,           // optional; required when outcome is error/timeout
#     "workflow_id":    null            // optional UUIDv7
#   }
#
# Test vectors (inline):
#
#   PASS — successful fs.read event
#   input = {
#     "seq":0,"correlation_id":"018f4e2a-9b3c-7d1e-a4f5-6789abcdef01",
#     "timestamp":"2026-05-21T14:30:00Z","tool_name":"fs.read","outcome":"success"
#   }
#
#   PASS — error outcome with error_code present
#   input = {
#     "seq":1,"correlation_id":"018f4e2a-9b3c-7d1e-a4f5-6789abcdef02",
#     "timestamp":"2026-05-21T14:31:00.000Z","tool_name":"archive.tar.extract",
#     "outcome":"error","error_code":"SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"
#   }
#
#   PASS — timeout outcome with error_code and workflow_id
#   input = {
#     "seq":2,"correlation_id":"018f4e2a-9b3c-7d1e-a4f5-6789abcdef03",
#     "timestamp":"2026-05-21T14:32:00.001Z","tool_name":"proc.signal",
#     "outcome":"timeout","error_code":"SUBSTRATE_TIMEOUT",
#     "workflow_id":"018f4e2b-0000-7000-8000-000000000001"
#   }
#
#   FAIL — error outcome missing error_code
#   input = {
#     "seq":3,"correlation_id":"018f4e2a-9b3c-7d1e-a4f5-6789abcdef04",
#     "timestamp":"2026-05-21T14:33:00Z","tool_name":"fs.remove","outcome":"error"
#   }
#   expected deny contains: "audit event seq=3: outcome 'error' requires error_code to be present"
#
#   FAIL — negative seq
#   input = {
#     "seq":-1,"correlation_id":"018f4e2a-9b3c-7d1e-a4f5-6789abcdef05",
#     "timestamp":"2026-05-21T14:34:00Z","tool_name":"fs.read","outcome":"success"
#   }
#   expected deny contains: "audit event seq=-1: seq must be a non-negative integer"
#
#   FAIL — correlation_id not UUIDv7
#   input = {
#     "seq":4,"correlation_id":"not-a-uuid",
#     "timestamp":"2026-05-21T14:35:00Z","tool_name":"fs.read","outcome":"success"
#   }
#   expected deny contains: "audit event seq=4: correlation_id 'not-a-uuid' does not match UUIDv7 format"
#
#   FAIL — timestamp missing Z suffix
#   input = {
#     "seq":5,"correlation_id":"018f4e2a-9b3c-7d1e-a4f5-6789abcdef06",
#     "timestamp":"2026-05-21T14:36:00+02:00","tool_name":"fs.read","outcome":"success"
#   }
#   expected deny contains: "audit event seq=5: timestamp '2026-05-21T14:36:00+02:00' must be ISO 8601 with Z suffix"
#
#   FAIL — tool_name uses invalid namespace
#   input = {
#     "seq":6,"correlation_id":"018f4e2a-9b3c-7d1e-a4f5-6789abcdef07",
#     "timestamp":"2026-05-21T14:37:00Z","tool_name":"net.connect","outcome":"success"
#   }
#   expected deny contains: "audit event seq=6: tool_name 'net.connect' does not match namespaced dot-notation pattern"
#
#   FAIL — outcome not in allowed enum
#   input = {
#     "seq":7,"correlation_id":"018f4e2a-9b3c-7d1e-a4f5-6789abcdef08",
#     "timestamp":"2026-05-21T14:38:00Z","tool_name":"fs.read","outcome":"unknown"
#   }
#   expected deny contains: "audit event seq=7: outcome 'unknown' is not in allowed set"
#
#   FAIL — workflow_id present but not UUIDv7
#   input = {
#     "seq":8,"correlation_id":"018f4e2a-9b3c-7d1e-a4f5-6789abcdef09",
#     "timestamp":"2026-05-21T14:39:00Z","tool_name":"fs.read","outcome":"success",
#     "workflow_id":"bad-workflow-id"
#   }
#   expected deny contains: "audit event seq=8: workflow_id 'bad-workflow-id' does not match UUIDv7 format"

package substrate.audit_event

import rego.v1

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

# Allowed outcome values.
_allowed_outcomes := {"success", "error", "timeout", "cancelled", "denied"}

# Outcomes that require error_code to be present.
_error_outcomes := {"error", "timeout"}

# UUIDv7: 8-4-4-4-12 hex; version nibble in position 14 must be 7;
# variant nibble in position 19 must be 8, 9, a, or b (RFC 9562 §5.7).
_uuidv7_pattern := `^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`

# ISO 8601 with Z suffix — matches full datetime with optional fractional seconds.
_iso8601z_pattern := `^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?Z$`

# Namespaced tool-name pattern (mirrors naming.rego _tool_name_pattern).
_tool_name_pattern := `^(fs|proc|sys|text|archive)\.[a-z_]+$`

# ---------------------------------------------------------------------------
# Invariant 1: outcome must be in the allowed enum
# ---------------------------------------------------------------------------

deny contains msg if {
    outcome := input.outcome
    not _allowed_outcomes[outcome]
    msg := sprintf(
        "audit event seq=%d: outcome '%s' is not in allowed set {success,error,timeout,cancelled,denied}",
        [input.seq, outcome],
    )
}

# ---------------------------------------------------------------------------
# Invariant 2: when outcome is error or timeout, error_code MUST be present
# ---------------------------------------------------------------------------

deny contains msg if {
    outcome := input.outcome
    _error_outcomes[outcome]
    # error_code must be present and non-null
    not _error_code_present
    msg := sprintf(
        "audit event seq=%d: outcome '%s' requires error_code to be present",
        [input.seq, outcome],
    )
}

_error_code_present if {
    input.error_code != null
    input.error_code != ""
}

# ---------------------------------------------------------------------------
# Invariant 3: seq must be a non-negative integer
# ---------------------------------------------------------------------------

deny contains msg if {
    seq := input.seq
    seq < 0
    msg := sprintf(
        "audit event seq=%d: seq must be a non-negative integer",
        [seq],
    )
}

# ---------------------------------------------------------------------------
# Invariant 4: correlation_id must match UUIDv7 format
# ---------------------------------------------------------------------------

deny contains msg if {
    cid := input.correlation_id
    not regex.match(_uuidv7_pattern, cid)
    msg := sprintf(
        "audit event seq=%d: correlation_id '%s' does not match UUIDv7 format",
        [input.seq, cid],
    )
}

# ---------------------------------------------------------------------------
# Invariant 5: timestamp must be ISO 8601 with Z suffix
# ---------------------------------------------------------------------------

deny contains msg if {
    ts := input.timestamp
    not regex.match(_iso8601z_pattern, ts)
    msg := sprintf(
        "audit event seq=%d: timestamp '%s' must be ISO 8601 with Z suffix",
        [input.seq, ts],
    )
}

# ---------------------------------------------------------------------------
# Invariant 6: tool_name must match the namespaced dot-notation pattern
# ---------------------------------------------------------------------------

deny contains msg if {
    tn := input.tool_name
    not regex.match(_tool_name_pattern, tn)
    msg := sprintf(
        "audit event seq=%d: tool_name '%s' does not match namespaced dot-notation pattern ^(fs|proc|sys|text|archive)\\.[a-z_]+$",
        [input.seq, tn],
    )
}

# ---------------------------------------------------------------------------
# Invariant 7: workflow_id, when present and non-null, must be a UUIDv7
# ---------------------------------------------------------------------------

deny contains msg if {
    wid := input.workflow_id
    wid != null
    wid != ""
    not regex.match(_uuidv7_pattern, wid)
    msg := sprintf(
        "audit event seq=%d: workflow_id '%s' does not match UUIDv7 format",
        [input.seq, wid],
    )
}

# ---------------------------------------------------------------------------
# allow — true only when deny is empty
# ---------------------------------------------------------------------------

default allow := false

allow if {
    count(deny) == 0
}
