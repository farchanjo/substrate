# package substrate.tool_annotations
#
# Enforces that every registered MCP tool declares annotations that match the
# canonical defaults defined in the tool annotation matrix (docs/arch/schemas/mcp_tool_spec.cue).
#
# Input shape:
#   {
#     "tool_name": "fs.read",
#     "annotations": {
#       "readOnlyHint":    true,
#       "destructiveHint": false,
#       "idempotentHint":  true,
#       "openWorldHint":   false
#     }
#   }
#
# Test vectors (inline):
#
#   PASS — fs.find with correct annotations
#   input = {"tool_name":"fs.find","annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}
#
#   PASS — fs.remove with correct annotations
#   input = {"tool_name":"fs.remove","annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false}}
#
#   FAIL — fs.remove incorrectly marked readOnly
#   input = {"tool_name":"fs.remove","annotations":{"readOnlyHint":true,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false}}
#   expected deny: "fs.remove: readOnlyHint must be false, got true"
#
#   FAIL — proc.signal missing destructiveHint=true
#   input = {"tool_name":"proc.signal","annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":false,"openWorldHint":false}}
#   expected deny: "proc.signal: destructiveHint must be true, got false"
#
#   FAIL — archive.hash incorrectly marked destructive
#   input = {"tool_name":"archive.hash","annotations":{"readOnlyHint":true,"destructiveHint":true,"idempotentHint":true,"openWorldHint":false}}
#   expected deny: "archive.hash: destructiveHint must be false, got true"

package substrate.tool_annotations

import rego.v1

# ---------------------------------------------------------------------------
# Expected annotation matrix derived from the tool annotation specification.
# Keys match the dot-notation tool names used by the MCP server.
# ---------------------------------------------------------------------------

# Read-only, non-destructive, idempotent, closed-world tools.
_read_only_tools := {
    "fs.find", "fs.read", "fs.read_dir", "fs.stat",
    "proc.list", "proc.tree",
    "sys.info", "sys.platform", "sys.env", "sys.cwd",
    "sys.load_average", "sys.hostname",
    "text.search", "text.count_lines", "text.head", "text.tail",
    "archive.hash",
    "net.tcp_list", "net.udp_list", "net.tcp_stats", "net.connection_count",
    "job.list", "job.result", "job.status",
    "subprocess.list", "subprocess.result", "subprocess.search",
    "launch.list", "launch.status", "launch.logs",
}

# Writable (create/copy), non-destructive, non-idempotent, closed-world tools.
_write_create_tools := {
    "fs.mkdir", "fs.write", "fs.copy",
    "archive.tar.create", "archive.tar.extract",
    "archive.zip.create", "archive.zip.extract",
    "archive.gzip.compress", "archive.gzip.decompress",
    "launch.init", "launch.trust", "launch.reload",
}

# Destructive, non-idempotent tools.
_destructive_nonidempotent_tools := {
    "fs.remove", "fs.rename",
    "proc.signal",
    "subprocess.spawn", "subprocess.signal", "subprocess.cancel",
    "job.cancel",
    "launch.up", "launch.down", "launch.restart",
}

# Destructive but idempotent tools.
_destructive_idempotent_tools := {
    "fs.set_permissions",
}

# ---------------------------------------------------------------------------
# Expected annotations per tool: returns a map {field -> expected_value}.
# Returns null when the tool name is not in the matrix (unknown tool).
# ---------------------------------------------------------------------------

_expected(tool) := exp if {
    _read_only_tools[tool]
    exp := {
        "readOnlyHint":    true,
        "destructiveHint": false,
        "idempotentHint":  true,
        "openWorldHint":   false,
    }
}

_expected(tool) := exp if {
    _write_create_tools[tool]
    exp := {
        "readOnlyHint":    false,
        "destructiveHint": false,
        "idempotentHint":  false,
        "openWorldHint":   false,
    }
}

_expected(tool) := exp if {
    _destructive_nonidempotent_tools[tool]
    exp := {
        "readOnlyHint":    false,
        "destructiveHint": true,
        "idempotentHint":  false,
        "openWorldHint":   false,
    }
}

_expected(tool) := exp if {
    _destructive_idempotent_tools[tool]
    exp := {
        "readOnlyHint":    false,
        "destructiveHint": true,
        "idempotentHint":  true,
        "openWorldHint":   false,
    }
}

# ---------------------------------------------------------------------------
# deny rules — one message per annotation field divergence
# ---------------------------------------------------------------------------

deny contains msg if {
    tool := input.tool_name
    exp := _expected(tool)
    actual := input.annotations.readOnlyHint
    exp.readOnlyHint != actual
    msg := sprintf("%s: readOnlyHint must be %v, got %v", [tool, exp.readOnlyHint, actual])
}

deny contains msg if {
    tool := input.tool_name
    exp := _expected(tool)
    actual := input.annotations.destructiveHint
    exp.destructiveHint != actual
    msg := sprintf("%s: destructiveHint must be %v, got %v", [tool, exp.destructiveHint, actual])
}

deny contains msg if {
    tool := input.tool_name
    exp := _expected(tool)
    actual := input.annotations.idempotentHint
    exp.idempotentHint != actual
    msg := sprintf("%s: idempotentHint must be %v, got %v", [tool, exp.idempotentHint, actual])
}

deny contains msg if {
    tool := input.tool_name
    exp := _expected(tool)
    actual := input.annotations.openWorldHint
    exp.openWorldHint != actual
    msg := sprintf("%s: openWorldHint must be %v, got %v", [tool, exp.openWorldHint, actual])
}

deny contains msg if {
    tool := input.tool_name
    not _expected(tool)
    msg := sprintf("%s: tool name not in annotation matrix; register it or fix the name", [tool])
}

# ---------------------------------------------------------------------------
# Extended annotation matrix nuances (added 2026-05-21)
# ---------------------------------------------------------------------------
#
# Additional test vectors:
#
#   PASS — archive.hash correctly read-only (confirming matrix entry)
#   input = {"tool_name":"archive.hash","annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false},"security_policy":{"dry_run_required_for":[]}}
#
#   FAIL — archive.tar.create has destructiveHint=true but is absent from dry_run_required_for
#   input = {"tool_name":"archive.tar.create","annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false},"security_policy":{"dry_run_required_for":[]}}
#   expected deny: "archive.tar.create: destructiveHint=true but tool is absent from security_policy.dry_run_required_for"
#
#   FAIL — archive.zip.create has destructiveHint=true but is absent from dry_run_required_for
#   input = {"tool_name":"archive.zip.create","annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false},"security_policy":{"dry_run_required_for":[]}}
#   expected deny: "archive.zip.create: destructiveHint=true but tool is absent from security_policy.dry_run_required_for"
#
#   PASS — proc.signal with SIGKILL in allowlist AND listed in elicitation_required_for
#   input = {
#     "tool_name":"proc.signal",
#     "annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false},
#     "security_policy":{"dry_run_required_for":["proc.signal"],"signal_allowlist":["SIGKILL"],"elicitation_required_for":["proc.signal"]}
#   }
#
#   FAIL — proc.signal with SIGKILL in allowlist but not in elicitation_required_for
#   input = {
#     "tool_name":"proc.signal",
#     "annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false},
#     "security_policy":{"dry_run_required_for":["proc.signal"],"signal_allowlist":["SIGKILL"],"elicitation_required_for":[]}
#   }
#   expected deny: "proc.signal: SIGKILL or SIGSTOP in signal_allowlist requires proc.signal in elicitation_required_for"
#
#   FAIL — any destructiveHint=true tool absent from dry_run_required_for
#   input = {"tool_name":"fs.remove","annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false},"security_policy":{"dry_run_required_for":[]}}
#   expected deny: "fs.remove: destructiveHint=true but tool is absent from security_policy.dry_run_required_for"

# archive.hash is intentionally listed in _read_only_tools above; readOnlyHint=true
# is already enforced by the matrix. No additional rule needed — confirmed.

# archive.tar.create and archive.zip.create ship with destructiveHint=false in the
# baseline matrix (overwrite=false is the safe default). When an operator sets
# destructiveHint=true at deploy time (overwrite=true flag) the cross-check below
# catches an absent dry_run_required_for entry, preventing unsafe deployment.

# ---------------------------------------------------------------------------
# Extended rule: any tool with destructiveHint=true MUST appear in
# security_policy.dry_run_required_for (cross-policy consistency gate).
# This complements security_invariants.rego Invariant 1 at the annotation layer.
# ---------------------------------------------------------------------------

deny contains msg if {
    tool := input.tool_name
    input.annotations.destructiveHint == true
    # input.security_policy is optional; skip rule when not supplied
    sp := input.security_policy
    not _sp_dry_run_includes(sp, tool)
    msg := sprintf(
        "%s: destructiveHint=true but tool is absent from security_policy.dry_run_required_for",
        [tool],
    )
}

_sp_dry_run_includes(sp, tool) if {
    sp.dry_run_required_for[_] == tool
}

# ---------------------------------------------------------------------------
# Extended rule: if signal_allowlist contains SIGKILL or SIGSTOP,
# proc.signal MUST be listed in elicitation_required_for.
# ---------------------------------------------------------------------------

_dangerous_signals := {"SIGKILL", "SIGSTOP"}

deny contains msg if {
    input.tool_name == "proc.signal"
    sp := input.security_policy
    _allowlist_has_dangerous_signal(sp.signal_allowlist)
    not _elicitation_covers_proc_signal(sp)
    msg := "proc.signal: SIGKILL or SIGSTOP in signal_allowlist requires proc.signal in elicitation_required_for"
}

_allowlist_has_dangerous_signal(allowlist) if {
    sig := allowlist[_]
    _dangerous_signals[sig]
}

_elicitation_covers_proc_signal(sp) if {
    sp.elicitation_required_for[_] == "proc.signal"
}

# ---------------------------------------------------------------------------
# allow — true only when deny is empty
# ---------------------------------------------------------------------------

default allow := false

allow if {
    count(deny) == 0
}
