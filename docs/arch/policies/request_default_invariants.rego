# package substrate.request_default_invariants
#
# Validates that MCP request structs with `#[serde(default = "fn")]` field
# overrides do NOT also use `#[derive(Default)]`, and that the
# `is_null() || empty_object` handler shortcut is not combined with a
# derive-generated Default impl. Per ADR-0061.
#
# Cross-references:
#   ADR-0061 — inbound request default and validation policy
#   ADR-0052 — subprocess bounded context (SubprocessListRequest)
#   ADR-0057 — subprocess output pagination and search
#   ADR-0059 — universal wait/timeout enforcement (prior art for this pattern)
#
# Input shape (produced by scripts/extract_request_structs.py):
#   {
#     "structs": [
#       {
#         "name":             "SubprocessListRequest",
#         "derive_default":   false,          // false = manual impl, true = #[derive(Default)]
#         "has_null_shortcut": true,           // true = handler uses is_null()||empty_object shortcut
#         "fields": [
#           {
#             "name":             "page_size",
#             "serde_default_fn": "default_page_size"  // null when no custom fn
#           }
#         ]
#       }
#     ]
#   }
#
# Test vectors (inline):
#
#   PASS — SubprocessListRequest: manual Default (derive_default=false), field has serde_default_fn
#   input = {"structs": [{"name": "SubprocessListRequest", "derive_default": false,
#             "has_null_shortcut": true, "fields": [{"name": "page_size",
#             "serde_default_fn": "default_page_size"}]}]}
#   expected: count(deny) == 0
#
#   FAIL — derive(Default) + serde(default = "fn") mismatch
#   input = {"structs": [{"name": "SomeRequest", "derive_default": true,
#             "has_null_shortcut": false, "fields": [{"name": "page_size",
#             "serde_default_fn": "default_page_size"}]}]}
#   expected deny contains: "#[serde(default = \"default_page_size\")]"
#
#   FAIL — shortcut used with derive(Default)
#   input = {"structs": [{"name": "SomeRequest", "derive_default": true,
#             "has_null_shortcut": true, "fields": [{"name": "cursor",
#             "serde_default_fn": null}]}]}
#   expected deny contains: "null/empty-object shortcut used with #[derive(Default)]"
#
#   PASS — derive(Default) but no serde_default_fn fields
#   input = {"structs": [{"name": "SomeRequest", "derive_default": true,
#             "has_null_shortcut": false, "fields": [{"name": "cursor",
#             "serde_default_fn": null}]}]}
#   expected: count(deny) == 0

package substrate.request_default_invariants

import rego.v1

# ---------------------------------------------------------------------------
# Invariant 1: a request struct that has both #[derive(Default)] AND a field
# with #[serde(default = "fn")] is a mismatch: the derive-generated Default
# returns the Rust zero-value, bypassing the API-contract default.
# ---------------------------------------------------------------------------

deny contains msg if {
	s := input.structs[_]
	s.derive_default == true
	f := s.fields[_]
	f.serde_default_fn != null
	msg := sprintf(
		"%s: field '%s' declares #[serde(default = \"%s\")] but the struct also derives Default; write a manual Default impl that matches the serde defaults, or remove #[derive(Default)]",
		[s.name, f.name, f.serde_default_fn],
	)
}

# ---------------------------------------------------------------------------
# Invariant 2: a handler that uses the is_null()||empty_object shortcut MUST
# NOT rely on #[derive(Default)] — that bypasses all #[serde(default = "fn")]
# overrides and delivers Rust zero-values instead of API-contract defaults.
# ---------------------------------------------------------------------------

deny contains msg if {
	s := input.structs[_]
	s.derive_default == true
	s.has_null_shortcut == true
	msg := sprintf(
		"%s: null/empty-object shortcut used with #[derive(Default)]; replace with a manual Default impl that honors all #[serde(default = \"fn\")] overrides",
		[s.name],
	)
}

# ---------------------------------------------------------------------------
# allow — true only when all deny rules produce no messages
# ---------------------------------------------------------------------------

default allow := false

allow if count(deny) == 0
