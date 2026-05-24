# package substrate.subprocess_invariants
#
# Validates a #SubprocessRequest before any child process is spawned.
# These invariants are evaluated at policy-evaluation time (CI conftest gate)
# and at runtime inside the substrate-subprocess adapter before the OS call.
#
# Cross-references:
#   ADR-0052 — subprocess bounded context decision
#   ADR-0053 — process lifecycle cascade contract
#   ADR-0004 — security model (Layer 1–5 enforcement sequence)
#
# Input shape:
#   {
#     "subprocess_request": {
#       "binary_path":     "<absolute path string>",
#       "args":            ["<arg>", ...],
#       "env_allowlist":   ["<VAR_NAME>", ...],
#       "env_override":    {"<KEY>": "<VALUE>"},
#       "cwd":             "<absolute path string>",
#       "stdin_kind":      "none" | "piped" | "file_path",
#       "stdin_file_path": "<string>",          // optional
#       "capture_kind":    "stream" | "in_memory" | "tmp_file",
#       "timeout_secs":    <int>                // optional
#     },
#     "elicitation_confirmed": <bool>           // optional; required for --allow-* args
#   }
#
# Test vectors (inline):
#
#   PASS — minimal valid spawn request
#   input = {
#     "subprocess_request": {
#       "binary_path":"/usr/bin/echo","args":["hello"],"env_allowlist":[],
#       "env_override":{},"cwd":"/tmp","stdin_kind":"none","capture_kind":"stream"
#     },
#     "elicitation_confirmed": true
#   }
#
#   FAIL — relative binary_path
#   input = {"subprocess_request":{"binary_path":"./hack","args":[],"env_allowlist":[],
#     "env_override":{},"cwd":"/tmp","stdin_kind":"none","capture_kind":"stream"}}
#   expected deny contains: "binary_path must be an absolute path"
#
#   FAIL — LD_PRELOAD in env_allowlist
#   input = {"subprocess_request":{"binary_path":"/usr/bin/echo","args":[],
#     "env_allowlist":["LD_PRELOAD"],"env_override":{},"cwd":"/tmp",
#     "stdin_kind":"none","capture_kind":"stream"}}
#   expected deny contains: "banned env var 'LD_PRELOAD'"
#
#   FAIL — timeout_secs > 86400
#   input = {"subprocess_request":{"binary_path":"/usr/bin/sleep","args":["99999"],
#     "env_allowlist":[],"env_override":{},"cwd":"/tmp","stdin_kind":"none",
#     "capture_kind":"stream","timeout_secs":86401}}
#   expected deny contains: "timeout_secs must not exceed 86400"

package substrate.subprocess_invariants

import rego.v1

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

# Unconditionally banned environment variable names regardless of allowlist.
# These variables enable library injection / dynamic linker hijacking attacks.
_banned_env_vars := {
	"LD_PRELOAD",
	"DYLD_INSERT_LIBRARIES",
	"LD_LIBRARY_PATH",
	"DYLD_LIBRARY_PATH",
}

# Valid capture_kind values.
_valid_capture_kinds := {"stream", "in_memory", "tmp_file"}

# Maximum permitted timeout in seconds (24 hours).
_max_timeout_secs := 86400

# ---------------------------------------------------------------------------
# Invariant 1: binary_path MUST be an absolute path (begins with /)
# ---------------------------------------------------------------------------

deny contains msg if {
	path := input.subprocess_request.binary_path
	not startswith(path, "/")
	msg := sprintf(
		"binary_path must be an absolute path (starts with /); got '%s'",
		[path],
	)
}

# ---------------------------------------------------------------------------
# Invariant 2: cwd MUST be an absolute path (begins with /)
# ---------------------------------------------------------------------------

deny contains msg if {
	cwd := input.subprocess_request.cwd
	not startswith(cwd, "/")
	msg := sprintf(
		"cwd must be an absolute path (starts with /); got '%s'",
		[cwd],
	)
}

# ---------------------------------------------------------------------------
# Invariant 3: env_allowlist MUST NOT contain banned env var names
# ---------------------------------------------------------------------------

deny contains msg if {
	some var
	input.subprocess_request.env_allowlist[_] == var
	_banned_env_vars[var]
	msg := sprintf(
		"banned env var '%s' is not permitted in env_allowlist — library-injection vectors are unconditionally blocked",
		[var],
	)
}

# ---------------------------------------------------------------------------
# Invariant 4: env_override MUST NOT contain banned env var keys
# ---------------------------------------------------------------------------

deny contains msg if {
	some key
	input.subprocess_request.env_override[key]
	_banned_env_vars[key]
	msg := sprintf(
		"banned env var '%s' is not permitted in env_override — library-injection vectors are unconditionally blocked",
		[key],
	)
}

# ---------------------------------------------------------------------------
# Invariant 5: args containing --allow-* flags require elicitation_confirmed
# This guards against privilege-escalation argument injection that may be
# silently accepted by certain target binaries.
# ---------------------------------------------------------------------------

deny contains msg if {
	arg := input.subprocess_request.args[_]
	startswith(arg, "--allow-")
	not input.elicitation_confirmed == true
	msg := sprintf(
		"arg '%s' starts with --allow- which requires elicitation_confirmed=true before spawning",
		[arg],
	)
}

# ---------------------------------------------------------------------------
# Invariant 6: stdin_kind == "file_path" requires stdin_file_path to be present
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocess_request.stdin_kind == "file_path"
	not input.subprocess_request.stdin_file_path
	msg := "stdin_kind is 'file_path' but stdin_file_path is not set"
}

# ---------------------------------------------------------------------------
# Invariant 7: capture_kind MUST be one of the three valid values
# ---------------------------------------------------------------------------

deny contains msg if {
	kind := input.subprocess_request.capture_kind
	not _valid_capture_kinds[kind]
	msg := sprintf(
		"capture_kind '%s' is invalid; must be one of: stream, in_memory, tmp_file",
		[kind],
	)
}

# ---------------------------------------------------------------------------
# Invariant 8: timeout_secs, when present, MUST NOT exceed 86400 (24 hours)
# ---------------------------------------------------------------------------

deny contains msg if {
	timeout := input.subprocess_request.timeout_secs
	timeout > _max_timeout_secs
	msg := sprintf(
		"timeout_secs %d exceeds the maximum of %d seconds (24 hours)",
		[timeout, _max_timeout_secs],
	)
}

# ---------------------------------------------------------------------------
# allow — true only when all deny rules produce no messages
# ---------------------------------------------------------------------------

default allow := false

allow if count(deny) == 0
