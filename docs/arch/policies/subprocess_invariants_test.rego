package substrate.subprocess_invariants_test

import rego.v1

import data.substrate.subprocess_invariants

# ---------------------------------------------------------------------------
# Shared fixture helpers
# ---------------------------------------------------------------------------

# A minimal valid SubprocessRequest for use as a base in each test.
_valid_request := {
	"binary_path": "/usr/bin/echo",
	"args": ["hello"],
	"env_allowlist": [],
	"env_override": {},
	"cwd": "/tmp/sandbox",
	"stdin_kind": "none",
	"capture_kind": "stream",
}

# ---------------------------------------------------------------------------
# test_relative_binary_path_denied
# Invariant 1: binary_path not starting with / must be denied.
# ---------------------------------------------------------------------------

test_relative_binary_path_denied if {
	result := subprocess_invariants.deny with input as {
		"subprocess_request": object.union(_valid_request, {"binary_path": "./hack"}),
		"elicitation_confirmed": true,
	}
	some msg
	result[msg]
	contains(msg, "binary_path must be an absolute path")
}

# ---------------------------------------------------------------------------
# test_relative_cwd_denied
# Invariant 2: cwd not starting with / must be denied.
# ---------------------------------------------------------------------------

test_relative_cwd_denied if {
	result := subprocess_invariants.deny with input as {
		"subprocess_request": object.union(_valid_request, {"cwd": "relative/dir"}),
		"elicitation_confirmed": true,
	}
	some msg
	result[msg]
	contains(msg, "cwd must be an absolute path")
}

# ---------------------------------------------------------------------------
# test_LD_PRELOAD_in_allowlist_denied
# Invariant 3: LD_PRELOAD in env_allowlist must be denied.
# ---------------------------------------------------------------------------

test_LD_PRELOAD_in_allowlist_denied if {
	result := subprocess_invariants.deny with input as {
		"subprocess_request": object.union(_valid_request, {"env_allowlist": ["PATH", "LD_PRELOAD"]}),
		"elicitation_confirmed": true,
	}
	some msg
	result[msg]
	contains(msg, "banned env var 'LD_PRELOAD'")
}

# ---------------------------------------------------------------------------
# test_LD_PRELOAD_in_override_denied
# Invariant 4: LD_PRELOAD as a key in env_override must be denied.
# ---------------------------------------------------------------------------

test_LD_PRELOAD_in_override_denied if {
	result := subprocess_invariants.deny with input as {
		"subprocess_request": object.union(_valid_request, {"env_override": {"LD_PRELOAD": "/evil/lib.so"}}),
		"elicitation_confirmed": true,
	}
	some msg
	result[msg]
	contains(msg, "banned env var 'LD_PRELOAD'")
}

# ---------------------------------------------------------------------------
# test_allow_flag_arg_without_elicitation_denied
# Invariant 5: --allow-* argument without elicitation_confirmed must be denied.
# ---------------------------------------------------------------------------

test_allow_flag_arg_without_elicitation_denied if {
	result := subprocess_invariants.deny with input as {
		"subprocess_request": object.union(_valid_request, {"args": ["--allow-root"]}),
	}
	some msg
	result[msg]
	contains(msg, "--allow-")
	contains(msg, "elicitation_confirmed=true")
}

# ---------------------------------------------------------------------------
# test_allow_flag_arg_with_elicitation_allowed
# Invariant 5 inverse: --allow-* argument WITH elicitation_confirmed=true must pass.
# ---------------------------------------------------------------------------

test_allow_flag_arg_with_elicitation_allowed if {
	count(subprocess_invariants.deny) == 0 with input as {
		"subprocess_request": object.union(_valid_request, {"args": ["--allow-root"]}),
		"elicitation_confirmed": true,
	}
}

# ---------------------------------------------------------------------------
# test_stdin_file_path_without_path_denied
# Invariant 6: stdin_kind="file_path" without stdin_file_path must be denied.
# ---------------------------------------------------------------------------

test_stdin_file_path_without_path_denied if {
	result := subprocess_invariants.deny with input as {
		"subprocess_request": object.union(_valid_request, {"stdin_kind": "file_path"}),
		"elicitation_confirmed": true,
	}
	some msg
	result[msg]
	contains(msg, "stdin_file_path is not set")
}

# ---------------------------------------------------------------------------
# test_unknown_capture_kind_denied
# Invariant 7: an unrecognized capture_kind must be denied.
# ---------------------------------------------------------------------------

test_unknown_capture_kind_denied if {
	result := subprocess_invariants.deny with input as {
		"subprocess_request": object.union(_valid_request, {"capture_kind": "stdout_only"}),
		"elicitation_confirmed": true,
	}
	some msg
	result[msg]
	contains(msg, "capture_kind 'stdout_only' is invalid")
}

# ---------------------------------------------------------------------------
# test_timeout_above_max_denied
# Invariant 8: timeout_secs > 86400 must be denied.
# ---------------------------------------------------------------------------

test_timeout_above_max_denied if {
	result := subprocess_invariants.deny with input as {
		"subprocess_request": object.union(_valid_request, {"timeout_secs": 86401}),
		"elicitation_confirmed": true,
	}
	some msg
	result[msg]
	contains(msg, "timeout_secs 86401 exceeds the maximum")
}

# ---------------------------------------------------------------------------
# test_happy_path_allowed
# All invariants satisfied: expect zero deny messages and allow=true.
# ---------------------------------------------------------------------------

test_happy_path_allowed if {
	count(subprocess_invariants.deny) == 0 with input as {
		"subprocess_request": {
			"binary_path": "/usr/bin/echo",
			"args": ["hello", "world"],
			"env_allowlist": ["PATH", "HOME"],
			"env_override": {"MY_VAR": "value"},
			"cwd": "/tmp/sandbox",
			"stdin_kind": "none",
			"capture_kind": "stream",
			"timeout_secs": 30,
		},
		"elicitation_confirmed": true,
	}
}

test_happy_path_allow_rule_true if {
	subprocess_invariants.allow with input as {
		"subprocess_request": {
			"binary_path": "/usr/bin/echo",
			"args": ["hello"],
			"env_allowlist": [],
			"env_override": {},
			"cwd": "/tmp/sandbox",
			"stdin_kind": "none",
			"capture_kind": "in_memory",
		},
		"elicitation_confirmed": true,
	}
}
