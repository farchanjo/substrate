# package substrate.error_catalog_completeness
#
# Validates that the ErrorCatalog is internally consistent and complete with
# respect to the set of ErrorCode values declared in the substrate domain.
#
# Every ErrorCode MUST have exactly one catalog entry.
# Every entry MUST carry: code, http_jsonrpc_code, recovery_hint, category.
# http_jsonrpc_code is constrained to the substrate-reserved range -32099..-32000
#   (exclusive of the JSON-RPC standard codes -32700/-32600/-32601/-32602/-32603).
# recovery_hint MUST be ≤150 characters.
# category MUST be one of the seven canonical values.
#
# Input shape:
#   {
#     "error_codes": ["SUBSTRATE_OK", "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"],
#     "catalog": [
#       {
#         "code":             "SUBSTRATE_OK",
#         "http_jsonrpc_code": -32001,
#         "recovery_hint":    "No action required.",
#         "category":         "internal"
#       }
#     ]
#   }
#
# Test vectors (inline):
#
#   PASS — single-entry catalog fully covers declared codes
#   input = {
#     "error_codes": ["SUBSTRATE_OK"],
#     "catalog": [
#       {"code":"SUBSTRATE_OK","http_jsonrpc_code":-32001,"recovery_hint":"No action required.","category":"internal"}
#     ]
#   }
#
#   FAIL — error code has no catalog entry
#   input = {
#     "error_codes": ["SUBSTRATE_OK","SUBSTRATE_PATH_OUTSIDE_ALLOWLIST"],
#     "catalog": [
#       {"code":"SUBSTRATE_OK","http_jsonrpc_code":-32001,"recovery_hint":"No action required.","category":"internal"}
#     ]
#   }
#   expected deny contains: "SUBSTRATE_PATH_OUTSIDE_ALLOWLIST: declared ErrorCode has no entry in ErrorCatalog"
#
#   FAIL — catalog entry missing recovery_hint field
#   input = {
#     "error_codes": ["SUBSTRATE_MISSING_HINT"],
#     "catalog": [
#       {"code":"SUBSTRATE_MISSING_HINT","http_jsonrpc_code":-32002,"category":"argument"}
#     ]
#   }
#   expected deny contains: "SUBSTRATE_MISSING_HINT: catalog entry is missing required field 'recovery_hint'"
#
#   FAIL — recovery_hint exceeds 150 characters
#   input = {
#     "error_codes": ["SUBSTRATE_LONG_HINT"],
#     "catalog": [
#       {"code":"SUBSTRATE_LONG_HINT","http_jsonrpc_code":-32003,"recovery_hint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","category":"resource"}
#     ]
#   }
#   expected deny contains: "SUBSTRATE_LONG_HINT: recovery_hint must be ≤150 characters, got 151"
#
#   FAIL — http_jsonrpc_code outside allowed range
#   input = {
#     "error_codes": ["SUBSTRATE_BAD_CODE"],
#     "catalog": [
#       {"code":"SUBSTRATE_BAD_CODE","http_jsonrpc_code":-32700,"recovery_hint":"Retry.","category":"protocol"}
#     ]
#   }
#   expected deny contains: "SUBSTRATE_BAD_CODE: http_jsonrpc_code -32700 is outside the substrate-reserved range -32099..-32000"
#
#   FAIL — category is not a canonical value
#   input = {
#     "error_codes": ["SUBSTRATE_BOGUS_CAT"],
#     "catalog": [
#       {"code":"SUBSTRATE_BOGUS_CAT","http_jsonrpc_code":-32004,"recovery_hint":"Check docs.","category":"networking"}
#     ]
#   }
#   expected deny contains: "SUBSTRATE_BOGUS_CAT: category 'networking' is not in the allowed set"
#
#   FAIL — catalog entry references a code that is not in error_codes
#   input = {
#     "error_codes": ["SUBSTRATE_OK"],
#     "catalog": [
#       {"code":"SUBSTRATE_OK","http_jsonrpc_code":-32001,"recovery_hint":"No action required.","category":"internal"},
#       {"code":"SUBSTRATE_GHOST","http_jsonrpc_code":-32005,"recovery_hint":"Ignore.","category":"internal"}
#     ]
#   }
#   expected deny contains: "SUBSTRATE_GHOST: catalog entry references an ErrorCode that is not declared"

package substrate.error_catalog_completeness

import rego.v1

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

# Canonical category values (ADR-0004, §ErrorCategory).
_allowed_categories := {
    "security",
    "resource",
    "argument",
    "protocol",
    "internal",
    "startup",
    "kernel",
}

# substrate-reserved JSON-RPC application-error range.
_jsonrpc_min := -32099
_jsonrpc_max := -32000

# JSON-RPC standard codes that substrate MUST never claim (RFC 7807 / JSON-RPC 2.0 spec).
_jsonrpc_standard_codes := {-32700, -32600, -32601, -32602, -32603}

# ---------------------------------------------------------------------------
# Index helpers — build a lookup map from catalog for O(1) access
# ---------------------------------------------------------------------------

# Map: code string -> catalog entry object.
_catalog_by_code[entry.code] := entry if {
    entry := input.catalog[_]
}

# Set of declared ErrorCode values.
_declared_codes := {c | c := input.error_codes[_]}

# ---------------------------------------------------------------------------
# Invariant 1: every declared ErrorCode MUST have a catalog entry
# ---------------------------------------------------------------------------

deny contains msg if {
    code := _declared_codes[_]
    not _catalog_by_code[code]
    msg := sprintf(
        "%s: declared ErrorCode has no entry in ErrorCatalog",
        [code],
    )
}

# ---------------------------------------------------------------------------
# Invariant 2: every catalog entry MUST reference a declared ErrorCode
# ---------------------------------------------------------------------------

deny contains msg if {
    entry := input.catalog[_]
    not _declared_codes[entry.code]
    msg := sprintf(
        "%s: catalog entry references an ErrorCode that is not declared",
        [entry.code],
    )
}

# ---------------------------------------------------------------------------
# Invariant 3: required fields must be present on every catalog entry
# ---------------------------------------------------------------------------

_required_fields := ["code", "http_jsonrpc_code", "recovery_hint", "category"]

deny contains msg if {
    entry := input.catalog[_]
    field := _required_fields[_]
    not entry[field]
    msg := sprintf(
        "%s: catalog entry is missing required field '%s'",
        [entry.code, field],
    )
}

# ---------------------------------------------------------------------------
# Invariant 4: http_jsonrpc_code must be in -32099..-32000 (inclusive) and
#              must not collide with any JSON-RPC 2.0 standard code
# ---------------------------------------------------------------------------

deny contains msg if {
    entry := input.catalog[_]
    jc := entry.http_jsonrpc_code
    not _valid_jsonrpc_code(jc)
    msg := sprintf(
        "%s: http_jsonrpc_code %d is outside the substrate-reserved range -32099..-32000",
        [entry.code, jc],
    )
}

_valid_jsonrpc_code(jc) if {
    jc >= _jsonrpc_min
    jc <= _jsonrpc_max
    not _jsonrpc_standard_codes[jc]
}

# ---------------------------------------------------------------------------
# Invariant 5: recovery_hint must be ≤150 characters
# ---------------------------------------------------------------------------

deny contains msg if {
    entry := input.catalog[_]
    hint := entry.recovery_hint
    count(hint) > 150
    msg := sprintf(
        "%s: recovery_hint must be ≤150 characters, got %d",
        [entry.code, count(hint)],
    )
}

# ---------------------------------------------------------------------------
# Invariant 6: category must be in the canonical set
# ---------------------------------------------------------------------------

deny contains msg if {
    entry := input.catalog[_]
    cat := entry.category
    not _allowed_categories[cat]
    msg := sprintf(
        "%s: category '%s' is not in the allowed set {security,resource,argument,protocol,internal,startup,kernel}",
        [entry.code, cat],
    )
}

# ---------------------------------------------------------------------------
# allow — true only when deny is empty
# ---------------------------------------------------------------------------

default allow := false

allow if {
    count(deny) == 0
}
