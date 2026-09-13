package substrate.request_default_invariants_test

import rego.v1

import data.substrate.request_default_invariants

# ---------------------------------------------------------------------------
# Shared fixture helpers
# ---------------------------------------------------------------------------

# A struct with a manual Default impl (no derive), one serde_default_fn field,
# and a null/empty-object shortcut in its handler.
# Represents the post-fix SubprocessListRequest (ADR-0061 PASS case).
_subprocess_list_fixed := {
	"name": "SubprocessListRequest",
	"derive_default": false,
	"has_null_shortcut": true,
	"fields": [
		{"name": "page_size", "serde_default_fn": "default_page_size"},
		{"name": "state_filter", "serde_default_fn": null},
		{"name": "page_cursor", "serde_default_fn": null},
		{"name": "client_id", "serde_default_fn": null},
	],
}

# A struct with #[derive(Default)] AND a serde_default_fn field (FAIL case).
_broken_request := {
	"name": "BrokenRequest",
	"derive_default": true,
	"has_null_shortcut": false,
	"fields": [
		{"name": "page_size", "serde_default_fn": "default_page_size"},
		{"name": "cursor", "serde_default_fn": null},
	],
}

# A struct with #[derive(Default)] but no serde_default_fn fields (PASS — no mismatch).
_safe_derive_request := {
	"name": "SafeDeriveRequest",
	"derive_default": true,
	"has_null_shortcut": false,
	"fields": [
		{"name": "cursor", "serde_default_fn": null},
		{"name": "limit", "serde_default_fn": null},
	],
}

# A struct with the shortcut AND #[derive(Default)] but no serde_default_fn (FAIL — shortcut invariant).
_shortcut_with_derive := {
	"name": "ShortcutDeriveRequest",
	"derive_default": true,
	"has_null_shortcut": true,
	"fields": [
		{"name": "cursor", "serde_default_fn": null},
	],
}

# A struct with a manual Default, a serde_default_fn field, NO shortcut (PASS — FsFindRequest pattern).
_fs_find_fixed := {
	"name": "FsFindRequest",
	"derive_default": false,
	"has_null_shortcut": false,
	"fields": [
		{"name": "root", "serde_default_fn": null},
		{"name": "max_depth", "serde_default_fn": "default_max_depth"},
		{"name": "page_size", "serde_default_fn": "default_page_size"},
		{"name": "pattern", "serde_default_fn": "default_glob"},
	],
}

# ---------------------------------------------------------------------------
# test_invariant1_derive_and_serde_fn_denied
# Invariant 1: derive_default=true + serde_default_fn!=null → deny.
# FAIL vector: BrokenRequest has derive_default=true and page_size with fn.
# ---------------------------------------------------------------------------

test_invariant1_derive_and_serde_fn_denied if {
	result := request_default_invariants.deny with input as {
		"structs": [_broken_request],
	}
	some msg in result
	contains(msg, "#[serde(default = \"default_page_size\")]")
}

# ---------------------------------------------------------------------------
# test_invariant1_deny_message_contains_struct_and_field
# Invariant 1: the deny message names both the struct and the offending field.
# ---------------------------------------------------------------------------

test_invariant1_deny_message_contains_struct_and_field if {
	result := request_default_invariants.deny with input as {
		"structs": [_broken_request],
	}
	some msg in result
	contains(msg, "BrokenRequest")
	contains(msg, "page_size")
}

# ---------------------------------------------------------------------------
# test_invariant1_manual_default_with_serde_fn_allowed
# Invariant 1 inverse: derive_default=false (manual impl) + serde_default_fn → allow.
# PASS vector: SubprocessListRequest after tactical fix.
# ---------------------------------------------------------------------------

test_invariant1_manual_default_with_serde_fn_allowed if {
	count(request_default_invariants.deny) == 0 with input as {
		"structs": [_subprocess_list_fixed],
	}
}

# ---------------------------------------------------------------------------
# test_invariant1_derive_default_no_serde_fn_allowed
# Invariant 1 inverse: derive_default=true but NO serde_default_fn field → allow.
# PASS vector: SafeDeriveRequest — pure Option<T> fields, no custom default fns.
# ---------------------------------------------------------------------------

test_invariant1_derive_default_no_serde_fn_allowed if {
	count(request_default_invariants.deny) == 0 with input as {
		"structs": [_safe_derive_request],
	}
}

# ---------------------------------------------------------------------------
# test_invariant2_shortcut_with_derive_denied
# Invariant 2: has_null_shortcut=true + derive_default=true → deny.
# FAIL vector: ShortcutDeriveRequest.
# ---------------------------------------------------------------------------

test_invariant2_shortcut_with_derive_denied if {
	result := request_default_invariants.deny with input as {
		"structs": [_shortcut_with_derive],
	}
	some msg in result
	contains(msg, "null/empty-object shortcut used with #[derive(Default)]")
}

# ---------------------------------------------------------------------------
# test_invariant2_deny_message_contains_struct_name
# Invariant 2: the deny message names the offending struct.
# ---------------------------------------------------------------------------

test_invariant2_deny_message_contains_struct_name if {
	result := request_default_invariants.deny with input as {
		"structs": [_shortcut_with_derive],
	}
	some msg in result
	contains(msg, "ShortcutDeriveRequest")
}

# ---------------------------------------------------------------------------
# test_invariant2_shortcut_with_manual_default_allowed
# Invariant 2 inverse: shortcut + manual Default (derive_default=false) → allow.
# PASS vector: SubprocessListRequest post-fix (has shortcut, but manual Default).
# ---------------------------------------------------------------------------

test_invariant2_shortcut_with_manual_default_allowed if {
	count(request_default_invariants.deny) == 0 with input as {
		"structs": [_subprocess_list_fixed],
	}
}

# ---------------------------------------------------------------------------
# test_fs_find_fixed_allowed
# FsFindRequest pattern: manual Default, serde_default_fn fields, NO shortcut → allow.
# PASS vector: FsFindRequest after ADR-0061 migration.
# ---------------------------------------------------------------------------

test_fs_find_fixed_allowed if {
	count(request_default_invariants.deny) == 0 with input as {
		"structs": [_fs_find_fixed],
	}
}

# ---------------------------------------------------------------------------
# test_multiple_structs_mixed_only_broken_denied
# Mixed input: one fixed struct + one broken struct → only the broken one is denied.
# ---------------------------------------------------------------------------

test_multiple_structs_mixed_only_broken_denied if {
	result := request_default_invariants.deny with input as {
		"structs": [_subprocess_list_fixed, _broken_request, _fs_find_fixed],
	}
	# At least one message for BrokenRequest
	some msg in result
	contains(msg, "BrokenRequest")
	# No message for SubprocessListRequest or FsFindRequest
	not any_msg_contains_name(result, "SubprocessListRequest")
	not any_msg_contains_name(result, "FsFindRequest")
}

# Helper: true if any message in `msgs` contains `name`.
any_msg_contains_name(msgs, name) if {
	some m in msgs
	contains(m, name)
}

# ---------------------------------------------------------------------------
# test_empty_structs_list_allowed
# Trivial PASS: no structs, no violations.
# ---------------------------------------------------------------------------

test_empty_structs_list_allowed if {
	count(request_default_invariants.deny) == 0 with input as {
		"structs": [],
	}
}

# ---------------------------------------------------------------------------
# test_allow_true_when_all_pass
# allow=true when deny is empty.
# PASS vector: only well-formed structs.
# ---------------------------------------------------------------------------

test_allow_true_when_all_pass if {
	request_default_invariants.allow with input as {
		"structs": [_subprocess_list_fixed, _safe_derive_request, _fs_find_fixed],
	}
}

# ---------------------------------------------------------------------------
# test_allow_false_when_any_fail
# allow=false when deny is non-empty.
# FAIL vector: BrokenRequest in the list.
# ---------------------------------------------------------------------------

test_allow_false_when_any_fail if {
	not request_default_invariants.allow with input as {
		"structs": [_subprocess_list_fixed, _broken_request],
	}
}
