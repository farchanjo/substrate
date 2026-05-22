package substrate.tool_annotations

import rego.v1

# ---------------------------------------------------------------------------
# Tests for read-only tool annotation enforcement
# ---------------------------------------------------------------------------

test_fs_find_correct_annotations_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "fs.find",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    }
}

test_fs_remove_read_only_hint_true_denied if {
    deny["fs.remove: readOnlyHint must be false, got true"] with input as {
        "tool_name": "fs.remove",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": true,
            "idempotentHint": false,
            "openWorldHint": false,
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for destructive tool annotation enforcement
# ---------------------------------------------------------------------------

test_fs_remove_correct_annotations_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "fs.remove",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": true,
            "idempotentHint": false,
            "openWorldHint": false,
        },
    }
}

test_proc_signal_missing_destructive_hint_denied if {
    deny["proc.signal: destructiveHint must be true, got false"] with input as {
        "tool_name": "proc.signal",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false,
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for write/create tool annotation enforcement
# ---------------------------------------------------------------------------

test_fs_mkdir_correct_annotations_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "fs.mkdir",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false,
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for unknown tool enforcement
# ---------------------------------------------------------------------------

test_unknown_tool_name_denied if {
    deny["net.upload: tool name not in annotation matrix; register it or fix the name"] with input as {
        "tool_name": "net.upload",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false,
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for archive.hash read-only enforcement
# ---------------------------------------------------------------------------

test_archive_hash_incorrectly_marked_destructive_denied if {
    deny["archive.hash: destructiveHint must be false, got true"] with input as {
        "tool_name": "archive.hash",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": true,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    }
}
