package substrate.security

import rego.v1

# ---------------------------------------------------------------------------
# Tests for Invariant 1: destructiveHint=true tools MUST be in dry_run list
# ---------------------------------------------------------------------------

test_destructive_tool_in_dry_run_list_allowed if {
    count(deny) == 0 with input as {
        "tool_spec": {
            "name": "fs.remove",
            "annotations": {"destructiveHint": true, "openWorldHint": false},
            "has_zip_slip_mitigation": false,
        },
        "security_policy": {
            "dry_run_required_for": ["fs.remove"],
            "signal_allowlist": ["SIGTERM"],
            "outbound_net_enabled": false,
            "features": [],
        },
    }
}

test_destructive_tool_missing_from_dry_run_denied if {
    deny["fs.remove: destructiveHint=true but tool is not listed in dry_run_required_for"] with input as {
        "tool_spec": {
            "name": "fs.remove",
            "annotations": {"destructiveHint": true, "openWorldHint": false},
            "has_zip_slip_mitigation": false,
        },
        "security_policy": {
            "dry_run_required_for": [],
            "signal_allowlist": ["SIGTERM"],
            "outbound_net_enabled": false,
            "features": [],
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 2: openWorldHint=true requires outbound-net feature
# ---------------------------------------------------------------------------

test_open_world_without_feature_denied if {
    deny["sys.fetch: openWorldHint=true requires feature 'outbound-net' to be enabled in security_policy.features"] with input as {
        "tool_spec": {
            "name": "sys.fetch",
            "annotations": {"destructiveHint": false, "openWorldHint": true},
            "has_zip_slip_mitigation": false,
        },
        "security_policy": {
            "dry_run_required_for": [],
            "signal_allowlist": ["SIGTERM"],
            "outbound_net_enabled": false,
            "features": [],
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 3: proc.signal requires non-empty signal_allowlist
# ---------------------------------------------------------------------------

test_proc_signal_with_empty_allowlist_denied if {
    deny["proc.signal: tool is registered but signal_allowlist is empty; at least one signal must be explicitly allowed"] with input as {
        "tool_spec": {
            "name": "proc.signal",
            "annotations": {"destructiveHint": true, "openWorldHint": false},
            "has_zip_slip_mitigation": false,
        },
        "security_policy": {
            "dry_run_required_for": ["proc.signal"],
            "signal_allowlist": [],
            "outbound_net_enabled": false,
            "features": [],
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 4: archive extract must reference Zip Slip mitigation
# ---------------------------------------------------------------------------

test_archive_extract_without_zip_slip_denied if {
    deny["archive.tar.extract: archive extraction tool MUST reference Zip Slip mitigation in its spec (has_zip_slip_mitigation must be true)"] with input as {
        "tool_spec": {
            "name": "archive.tar.extract",
            "annotations": {"destructiveHint": false, "openWorldHint": false},
            "has_zip_slip_mitigation": false,
        },
        "security_policy": {
            "dry_run_required_for": ["archive.tar.extract"],
            "signal_allowlist": ["SIGTERM"],
            "outbound_net_enabled": false,
            "features": [],
        },
    }
}

test_archive_extract_with_zip_slip_allowed if {
    count(deny) == 0 with input as {
        "tool_spec": {
            "name": "archive.zip.extract",
            "annotations": {"destructiveHint": false, "openWorldHint": false},
            "has_zip_slip_mitigation": true,
            "references_openat2_path_safety": true,
            "rejects_symlink_members": true,
        },
        "security_policy": {
            "dry_run_required_for": ["archive.zip.extract"],
            "signal_allowlist": ["SIGTERM"],
            "outbound_net_enabled": false,
            "features": [],
            "reject_hardlinks": false,
            "archive_allow_symlinks": false,
        },
        "startup_error_schema": "substrate-startup-error/v1",
    }
}
