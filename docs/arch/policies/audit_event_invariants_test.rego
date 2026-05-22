package substrate.audit_event

import rego.v1

# ---------------------------------------------------------------------------
# Tests for Invariant 1: outcome must be in the allowed enum
# ---------------------------------------------------------------------------

test_valid_success_outcome_allowed if {
    count(deny) == 0 with input as {
        "seq": 0,
        "correlation_id": "018f4e2a-9b3c-7d1e-a4f5-6789abcdef01",
        "timestamp": "2026-05-21T14:30:00Z",
        "tool_name": "fs.read",
        "outcome": "success",
    }
}

test_unknown_outcome_denied if {
    deny["audit event seq=7: outcome 'unknown' is not in allowed set {success,error,timeout,cancelled,denied}"] with input as {
        "seq": 7,
        "correlation_id": "018f4e2a-9b3c-7d1e-a4f5-6789abcdef08",
        "timestamp": "2026-05-21T14:38:00Z",
        "tool_name": "fs.read",
        "outcome": "unknown",
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 2: error/timeout outcome requires error_code
# ---------------------------------------------------------------------------

test_error_outcome_with_code_allowed if {
    count(deny) == 0 with input as {
        "seq": 1,
        "correlation_id": "018f4e2a-9b3c-7d1e-a4f5-6789abcdef02",
        "timestamp": "2026-05-21T14:31:00.000Z",
        "tool_name": "fs.remove",
        "outcome": "error",
        "error_code": "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST",
    }
}

test_error_outcome_without_code_denied if {
    deny["audit event seq=3: outcome 'error' requires error_code to be present"] with input as {
        "seq": 3,
        "correlation_id": "018f4e2a-9b3c-7d1e-a4f5-6789abcdef04",
        "timestamp": "2026-05-21T14:33:00Z",
        "tool_name": "fs.remove",
        "outcome": "error",
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 3: seq must be non-negative
# ---------------------------------------------------------------------------

test_negative_seq_denied if {
    deny["audit event seq=-1: seq must be a non-negative integer"] with input as {
        "seq": -1,
        "correlation_id": "018f4e2a-9b3c-7d1e-a4f5-6789abcdef05",
        "timestamp": "2026-05-21T14:34:00Z",
        "tool_name": "fs.read",
        "outcome": "success",
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 4: correlation_id must be UUIDv7
# ---------------------------------------------------------------------------

test_non_uuid_correlation_id_denied if {
    deny["audit event seq=4: correlation_id 'not-a-uuid' does not match UUIDv7 format"] with input as {
        "seq": 4,
        "correlation_id": "not-a-uuid",
        "timestamp": "2026-05-21T14:35:00Z",
        "tool_name": "fs.read",
        "outcome": "success",
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 5: timestamp must have Z suffix
# ---------------------------------------------------------------------------

test_timestamp_without_z_suffix_denied if {
    deny["audit event seq=5: timestamp '2026-05-21T14:36:00+02:00' must be ISO 8601 with Z suffix"] with input as {
        "seq": 5,
        "correlation_id": "018f4e2a-9b3c-7d1e-a4f5-6789abcdef06",
        "timestamp": "2026-05-21T14:36:00+02:00",
        "tool_name": "fs.read",
        "outcome": "success",
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 6: tool_name must match namespaced pattern
# ---------------------------------------------------------------------------

test_invalid_namespace_in_tool_name_denied if {
    deny["audit event seq=6: tool_name 'net.connect' does not match namespaced dot-notation pattern ^(fs|proc|sys|text|archive)\\.[a-z_]+$"] with input as {
        "seq": 6,
        "correlation_id": "018f4e2a-9b3c-7d1e-a4f5-6789abcdef07",
        "timestamp": "2026-05-21T14:37:00Z",
        "tool_name": "net.connect",
        "outcome": "success",
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 7: workflow_id when present must be UUIDv7
# ---------------------------------------------------------------------------

test_invalid_workflow_id_denied if {
    deny["audit event seq=8: workflow_id 'bad-workflow-id' does not match UUIDv7 format"] with input as {
        "seq": 8,
        "correlation_id": "018f4e2a-9b3c-7d1e-a4f5-6789abcdef09",
        "timestamp": "2026-05-21T14:39:00Z",
        "tool_name": "fs.read",
        "outcome": "success",
        "workflow_id": "bad-workflow-id",
    }
}

test_valid_workflow_id_allowed if {
    count(deny) == 0 with input as {
        "seq": 2,
        "correlation_id": "018f4e2a-9b3c-7d1e-a4f5-6789abcdef03",
        "timestamp": "2026-05-21T14:32:00.001Z",
        "tool_name": "proc.signal",
        "outcome": "timeout",
        "error_code": "SUBSTRATE_TIMEOUT",
        "workflow_id": "018f4e2b-0000-7000-8000-000000000002",
    }
}
