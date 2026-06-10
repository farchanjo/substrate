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

# ---------------------------------------------------------------------------
# Tests for read-only classification of job, net, subprocess, sys, text tools
# ---------------------------------------------------------------------------

test_net_tcp_list_read_only_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "net.tcp_list",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    }
}

test_job_result_read_only_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "job.result",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    }
}

test_subprocess_search_read_only_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "subprocess.search",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    }
}

test_sys_load_average_read_only_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "sys.load_average",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    }
}

test_text_tail_read_only_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "text.tail",
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for write-create classification of archive.gzip tools
# ---------------------------------------------------------------------------

test_archive_gzip_compress_write_create_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "archive.gzip.compress",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false,
        },
    }
}

test_archive_gzip_decompress_write_create_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "archive.gzip.decompress",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false,
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for destructive classification of subprocess.spawn and job.cancel
# ---------------------------------------------------------------------------

test_subprocess_spawn_destructive_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "subprocess.spawn",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": true,
            "idempotentHint": false,
            "openWorldHint": false,
        },
        "security_policy": {"dry_run_required_for": ["subprocess.spawn"]},
    }
}

test_subprocess_signal_destructive_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "subprocess.signal",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": true,
            "idempotentHint": false,
            "openWorldHint": false,
        },
        "security_policy": {"dry_run_required_for": ["subprocess.signal"]},
    }
}

test_job_cancel_destructive_allowed if {
    count(deny) == 0 with input as {
        "tool_name": "job.cancel",
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": true,
            "idempotentHint": false,
            "openWorldHint": false,
        },
        "security_policy": {"dry_run_required_for": ["job.cancel"]},
    }
}
