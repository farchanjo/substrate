# package substrate.security
#
# Enforces security invariants derived from ADR-0004 (Security Model) and the
# schemas in docs/arch/schemas/security_policy.cue and mcp_tool_spec.cue.
#
# These rules run at policy-evaluation time (e.g. OPA sidecar or CI gate) and
# are NOT a substitute for runtime enforcement inside the substrate process.
# They validate that the declared security_policy configuration and tool specs
# are internally consistent before deployment.
#
# Input shape:
#   {
#     "tool_spec": {
#       "name":        "fs.remove",
#       "annotations": {
#         "destructiveHint": true,
#         "openWorldHint":   false
#       },
#       "has_zip_slip_mitigation": true    // true when archive extraction tool
#                                           // references Zip Slip check in its spec
#     },
#     "security_policy": {
#       "dry_run_required_for":   ["fs.remove","fs.rename","fs.set_permissions","proc.signal","archive.tar.create","archive.tar.extract","archive.zip.create","archive.zip.extract","archive.gzip.compress","archive.gzip.decompress"],
#       "signal_allowlist":       ["SIGTERM","SIGHUP","SIGINT","SIGUSR1","SIGUSR2"],
#       "outbound_net_enabled":   false,
#       "features":               []      // list of enabled cargo feature flag strings
#     }
#   }
#
# Test vectors (inline):
#
#   PASS — fs.remove in dry_run list, no open world, non-archive
#   input = {
#     "tool_spec":{"name":"fs.remove","annotations":{"destructiveHint":true,"openWorldHint":false},"has_zip_slip_mitigation":false},
#     "security_policy":{"dry_run_required_for":["fs.remove"],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[]}
#   }
#
#   PASS — archive extract with Zip Slip mitigation reference
#   input = {
#     "tool_spec":{"name":"archive.zip.extract","annotations":{"destructiveHint":false,"openWorldHint":false},"has_zip_slip_mitigation":true},
#     "security_policy":{"dry_run_required_for":["archive.zip.extract"],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[]}
#   }
#
#   FAIL — destructive tool missing from dry_run_required_for
#   input = {
#     "tool_spec":{"name":"fs.remove","annotations":{"destructiveHint":true,"openWorldHint":false},"has_zip_slip_mitigation":false},
#     "security_policy":{"dry_run_required_for":[],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[]}
#   }
#   expected deny: "fs.remove: destructiveHint=true but tool is not listed in dry_run_required_for"
#
#   FAIL — openWorldHint=true without outbound-net feature
#   input = {
#     "tool_spec":{"name":"sys.fetch","annotations":{"destructiveHint":false,"openWorldHint":true},"has_zip_slip_mitigation":false},
#     "security_policy":{"dry_run_required_for":[],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[]}
#   }
#   expected deny: "sys.fetch: openWorldHint=true requires feature 'outbound-net' to be enabled in security_policy.features"
#
#   FAIL — proc.signal allowed but signal_allowlist is empty
#   input = {
#     "tool_spec":{"name":"proc.signal","annotations":{"destructiveHint":true,"openWorldHint":false},"has_zip_slip_mitigation":false},
#     "security_policy":{"dry_run_required_for":["proc.signal"],"signal_allowlist":[],"outbound_net_enabled":false,"features":[]}
#   }
#   expected deny: "proc.signal: tool is registered but signal_allowlist is empty; at least one signal must be explicitly allowed"
#
#   FAIL — archive extract tool missing Zip Slip mitigation reference
#   input = {
#     "tool_spec":{"name":"archive.tar.extract","annotations":{"destructiveHint":false,"openWorldHint":false},"has_zip_slip_mitigation":false},
#     "security_policy":{"dry_run_required_for":["archive.tar.extract"],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[]}
#   }
#   expected deny: "archive.tar.extract: archive extraction tool MUST reference Zip Slip mitigation in its spec (has_zip_slip_mitigation must be true)"

package substrate.security

import rego.v1

# ---------------------------------------------------------------------------
# Helper sets
# ---------------------------------------------------------------------------

# Canonical set of tools that must appear in dry_run_required_for when
# declared with destructiveHint=true or belonging to the archive extract family.
_destructive_annotation(tool) if {
    input.tool_spec.name == tool
    input.tool_spec.annotations.destructiveHint == true
}

_archive_extract_tools := {
    "archive.tar.extract",
    "archive.zip.extract",
    "archive.gzip.decompress",
}

_is_archive_extract(name) if _archive_extract_tools[name]

_is_proc_signal(name) if name == "proc.signal"

# ---------------------------------------------------------------------------
# Invariant 1: destructiveHint=true tools MUST appear in dry_run_required_for
# ---------------------------------------------------------------------------

deny contains msg if {
    input.tool_spec.annotations.destructiveHint == true
    tool := input.tool_spec.name
    not _in_dry_run_list(tool)
    msg := sprintf(
        "%s: destructiveHint=true but tool is not listed in dry_run_required_for",
        [tool],
    )
}

_in_dry_run_list(tool) if {
    input.security_policy.dry_run_required_for[_] == tool
}

# ---------------------------------------------------------------------------
# Invariant 2: openWorldHint=true requires feature 'outbound-net' to be enabled
# ---------------------------------------------------------------------------

deny contains msg if {
    input.tool_spec.annotations.openWorldHint == true
    tool := input.tool_spec.name
    not _outbound_net_enabled
    msg := sprintf(
        "%s: openWorldHint=true requires feature 'outbound-net' to be enabled in security_policy.features",
        [tool],
    )
}

_outbound_net_enabled if {
    input.security_policy.features[_] == "outbound-net"
}

_outbound_net_enabled if {
    input.security_policy.outbound_net_enabled == true
}

# ---------------------------------------------------------------------------
# Invariant 3: proc.signal registered but signal_allowlist is empty
# ---------------------------------------------------------------------------

deny contains msg if {
    _is_proc_signal(input.tool_spec.name)
    count(input.security_policy.signal_allowlist) == 0
    msg := sprintf(
        "%s: tool is registered but signal_allowlist is empty; at least one signal must be explicitly allowed",
        [input.tool_spec.name],
    )
}

# ---------------------------------------------------------------------------
# Invariant 4: archive extraction tools MUST reference Zip Slip mitigation
# ---------------------------------------------------------------------------

deny contains msg if {
    _is_archive_extract(input.tool_spec.name)
    input.tool_spec.has_zip_slip_mitigation != true
    msg := sprintf(
        "%s: archive extraction tool MUST reference Zip Slip mitigation in its spec (has_zip_slip_mitigation must be true)",
        [input.tool_spec.name],
    )
}

# ---------------------------------------------------------------------------
# Invariant 5: outbound_net_enabled=true without feature gate is inconsistent
# This catches the case where the runtime flag is set but the cargo feature
# that actually compiles the TCP stack is absent from the feature list.
# ---------------------------------------------------------------------------

deny contains msg if {
    input.security_policy.outbound_net_enabled == true
    not _feature_outbound_net_declared
    msg := "security_policy.outbound_net_enabled=true but cargo feature 'outbound-net' is absent from features list; they must be aligned"
}

_feature_outbound_net_declared if {
    input.security_policy.features[_] == "outbound-net"
}

# ---------------------------------------------------------------------------
# Extended invariants (added 2026-05-21)
# ---------------------------------------------------------------------------
#
# Additional test vectors:
#
#   PASS — archive.tar.extract with openat2/O_NOFOLLOW_ANY reference in spec
#   input = {
#     "tool_spec":{"name":"archive.tar.extract","annotations":{"destructiveHint":false,"openWorldHint":false},"has_zip_slip_mitigation":true,"references_openat2_path_safety":true,"stat_schema_includes_nlink":false},
#     "security_policy":{"dry_run_required_for":["archive.tar.extract"],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[],"reject_hardlinks":false,"archive_allow_symlinks":false},
#     "startup_error_schema":"substrate-startup-error/v1"
#   }
#
#   FAIL — archive extract tool missing openat2/O_NOFOLLOW_ANY reference
#   input = {
#     "tool_spec":{"name":"archive.tar.extract","annotations":{"destructiveHint":false,"openWorldHint":false},"has_zip_slip_mitigation":true,"references_openat2_path_safety":false},
#     "security_policy":{"dry_run_required_for":["archive.tar.extract"],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[],"reject_hardlinks":false,"archive_allow_symlinks":false},
#     "startup_error_schema":"substrate-startup-error/v1"
#   }
#   expected deny contains: "archive.tar.extract: archive extract tool spec MUST reference openat2/O_NOFOLLOW_ANY path safety hardening (ADR-0035)"
#
#   FAIL — reject_hardlinks=true but stat_schema_includes_nlink=false
#   input = {
#     "tool_spec":{"name":"fs.stat","annotations":{"destructiveHint":false,"openWorldHint":false},"has_zip_slip_mitigation":false,"stat_schema_includes_nlink":false},
#     "security_policy":{"dry_run_required_for":[],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[],"reject_hardlinks":true,"archive_allow_symlinks":false},
#     "startup_error_schema":"substrate-startup-error/v1"
#   }
#   expected deny contains: "fs.stat: reject_hardlinks=true requires stat output schema to include 'nlink' field"
#
#   FAIL — archive_allow_symlinks=false but extract tool missing symlink rejection declaration
#   input = {
#     "tool_spec":{"name":"archive.zip.extract","annotations":{"destructiveHint":false,"openWorldHint":false},"has_zip_slip_mitigation":true,"references_openat2_path_safety":true,"rejects_symlink_members":false},
#     "security_policy":{"dry_run_required_for":["archive.zip.extract"],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[],"reject_hardlinks":false,"archive_allow_symlinks":false},
#     "startup_error_schema":"substrate-startup-error/v1"
#   }
#   expected deny contains: "archive.zip.extract: archive_allow_symlinks=false but tool spec does not declare that symlink/hardlink members are rejected"
#
#   FAIL — startup_error_schema missing or not conforming to substrate-startup-error/v1
#   input = {
#     "tool_spec":{"name":"fs.read","annotations":{"destructiveHint":false,"openWorldHint":false},"has_zip_slip_mitigation":false},
#     "security_policy":{"dry_run_required_for":[],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[],"reject_hardlinks":false,"archive_allow_symlinks":false},
#     "startup_error_schema":"custom-error/v2"
#   }
#   expected deny contains: "startup errors MUST conform to substrate-startup-error/v1 schema"
#
#   FAIL — substrate-defined JSON-RPC code collides with a standard code
#   input = {
#     "tool_spec":{"name":"fs.read","annotations":{"destructiveHint":false,"openWorldHint":false},"has_zip_slip_mitigation":false},
#     "security_policy":{"dry_run_required_for":[],"signal_allowlist":["SIGTERM"],"outbound_net_enabled":false,"features":[],"reject_hardlinks":false,"archive_allow_symlinks":false,"jsonrpc_application_codes":[-32700]},
#     "startup_error_schema":"substrate-startup-error/v1"
#   }
#   expected deny contains: "security_policy.jsonrpc_application_codes must not include standard JSON-RPC code -32700"

# ---------------------------------------------------------------------------
# Invariant 6: archive extract tools MUST reference openat2/O_NOFOLLOW_ANY
# path safety hardening (ADR-0035 requirement).
# Field: tool_spec.references_openat2_path_safety (bool)
# ---------------------------------------------------------------------------

deny contains msg if {
    _is_archive_extract(input.tool_spec.name)
    input.tool_spec.references_openat2_path_safety != true
    msg := sprintf(
        "%s: archive extract tool spec MUST reference openat2/O_NOFOLLOW_ANY path safety hardening (ADR-0035)",
        [input.tool_spec.name],
    )
}

# ---------------------------------------------------------------------------
# Invariant 7: when reject_hardlinks=true, fs.stat output schema MUST include
# the 'nlink' field so callers can verify link counts.
# Field: tool_spec.stat_schema_includes_nlink (bool), only checked for fs.stat.
# ---------------------------------------------------------------------------

deny contains msg if {
    input.security_policy.reject_hardlinks == true
    input.tool_spec.name == "fs.stat"
    input.tool_spec.stat_schema_includes_nlink != true
    msg := "fs.stat: reject_hardlinks=true requires stat output schema to include 'nlink' field"
}

# ---------------------------------------------------------------------------
# Invariant 8: when archive_allow_symlinks=false (default), every archive
# extract tool MUST declare that it rejects symlink and hardlink members.
# Field: tool_spec.rejects_symlink_members (bool)
# ---------------------------------------------------------------------------

deny contains msg if {
    _is_archive_extract(input.tool_spec.name)
    input.security_policy.archive_allow_symlinks == false
    input.tool_spec.rejects_symlink_members != true
    msg := sprintf(
        "%s: archive_allow_symlinks=false but tool spec does not declare that symlink/hardlink members are rejected",
        [input.tool_spec.name],
    )
}

# ---------------------------------------------------------------------------
# Invariant 9: startup error envelopes MUST conform to substrate-startup-error/v1
# schema (fields: code, message_en_us, recovery_hint, correlation_id, timestamp,
# details). The schema identifier must be exactly "substrate-startup-error/v1".
# Field: input.startup_error_schema (string, optional — omit to skip check)
# ---------------------------------------------------------------------------

deny contains msg if {
    schema := input.startup_error_schema
    schema != "substrate-startup-error/v1"
    msg := sprintf(
        "startup errors MUST conform to substrate-startup-error/v1 schema; got '%s'",
        [schema],
    )
}

# ---------------------------------------------------------------------------
# Invariant 10: substrate-defined JSON-RPC application error codes MUST be in
# the range -32001..-32099; they MUST NOT override JSON-RPC 2.0 standard codes
# (-32700, -32600, -32601, -32602, -32603).
# Field: security_policy.jsonrpc_application_codes (list of ints, optional)
# ---------------------------------------------------------------------------

# Standard JSON-RPC 2.0 codes that substrate must never claim.
_jsonrpc_std_codes := {-32700, -32600, -32601, -32602, -32603}

# Substrate application-error range: -32001..-32099 (excludes -32000 which is
# the JSON-RPC "Server error" boundary shared with application codes in some
# implementations; conservative exclusion prevents ambiguity).
_jsonrpc_app_min := -32099
_jsonrpc_app_max := -32001

deny contains msg if {
    code := input.security_policy.jsonrpc_application_codes[_]
    _jsonrpc_std_codes[code]
    msg := sprintf(
        "security_policy.jsonrpc_application_codes must not include standard JSON-RPC code %d",
        [code],
    )
}

deny contains msg if {
    code := input.security_policy.jsonrpc_application_codes[_]
    not _jsonrpc_std_codes[code]
    not _in_substrate_jsonrpc_range(code)
    msg := sprintf(
        "security_policy.jsonrpc_application_codes: code %d is outside the substrate-reserved range -32099..-32001",
        [code],
    )
}

_in_substrate_jsonrpc_range(code) if {
    code >= _jsonrpc_app_min
    code <= _jsonrpc_app_max
}

# ---------------------------------------------------------------------------
# allow — true only when deny is empty
# ---------------------------------------------------------------------------

default allow := false

allow if {
    count(deny) == 0
}
