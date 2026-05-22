package substrate.error_catalog_completeness

import rego.v1

# ---------------------------------------------------------------------------
# Tests for Invariant 1: every declared ErrorCode MUST have a catalog entry
# ---------------------------------------------------------------------------

test_single_code_with_full_catalog_entry_allowed if {
    count(deny) == 0 with input as {
        "error_codes": ["SUBSTRATE_OK"],
        "catalog": [
            {
                "code": "SUBSTRATE_OK",
                "http_jsonrpc_code": -32001,
                "recovery_hint": "No action required.",
                "category": "internal",
            },
        ],
    }
}

test_declared_code_missing_from_catalog_denied if {
    deny["SUBSTRATE_PATH_OUTSIDE_ALLOWLIST: declared ErrorCode has no entry in ErrorCatalog"] with input as {
        "error_codes": ["SUBSTRATE_OK", "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"],
        "catalog": [
            {
                "code": "SUBSTRATE_OK",
                "http_jsonrpc_code": -32001,
                "recovery_hint": "No action required.",
                "category": "internal",
            },
        ],
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 2: catalog entries must reference declared codes
# ---------------------------------------------------------------------------

test_orphan_catalog_entry_denied if {
    deny["SUBSTRATE_GHOST: catalog entry references an ErrorCode that is not declared"] with input as {
        "error_codes": ["SUBSTRATE_OK"],
        "catalog": [
            {
                "code": "SUBSTRATE_OK",
                "http_jsonrpc_code": -32001,
                "recovery_hint": "No action required.",
                "category": "internal",
            },
            {
                "code": "SUBSTRATE_GHOST",
                "http_jsonrpc_code": -32005,
                "recovery_hint": "Ignore.",
                "category": "internal",
            },
        ],
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 4: http_jsonrpc_code must be in -32099..-32000
# ---------------------------------------------------------------------------

test_jsonrpc_code_outside_range_denied if {
    deny["SUBSTRATE_BAD_CODE: http_jsonrpc_code -32700 is outside the substrate-reserved range -32099..-32000"] with input as {
        "error_codes": ["SUBSTRATE_BAD_CODE"],
        "catalog": [
            {
                "code": "SUBSTRATE_BAD_CODE",
                "http_jsonrpc_code": -32700,
                "recovery_hint": "Retry.",
                "category": "protocol",
            },
        ],
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 5: recovery_hint must be <= 150 characters
# ---------------------------------------------------------------------------

test_recovery_hint_too_long_denied if {
    deny["SUBSTRATE_LONG_HINT: recovery_hint must be ≤150 characters, got 151"] with input as {
        "error_codes": ["SUBSTRATE_LONG_HINT"],
        "catalog": [
            {
                "code": "SUBSTRATE_LONG_HINT",
                "http_jsonrpc_code": -32003,
                "recovery_hint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "category": "resource",
            },
        ],
    }
}

# ---------------------------------------------------------------------------
# Tests for Invariant 6: category must be in the canonical set
# ---------------------------------------------------------------------------

test_invalid_category_denied if {
    deny["SUBSTRATE_BOGUS_CAT: category 'networking' is not in the allowed set {security,resource,argument,protocol,internal,startup,kernel}"] with input as {
        "error_codes": ["SUBSTRATE_BOGUS_CAT"],
        "catalog": [
            {
                "code": "SUBSTRATE_BOGUS_CAT",
                "http_jsonrpc_code": -32004,
                "recovery_hint": "Check docs.",
                "category": "networking",
            },
        ],
    }
}

test_all_canonical_categories_accepted if {
    count(deny) == 0 with input as {
        "error_codes": ["SUBSTRATE_SEC"],
        "catalog": [
            {
                "code": "SUBSTRATE_SEC",
                "http_jsonrpc_code": -32010,
                "recovery_hint": "Check permissions.",
                "category": "security",
            },
        ],
    }
}
