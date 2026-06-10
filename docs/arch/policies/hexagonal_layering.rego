# package substrate.hexagonal
#
# Enforces the hexagonal layering rules for the substrate crate stack.
# Violations indicate a dependency that crosses an architectural boundary.
#
# Crate taxonomy:
#   domain        : substrate-domain           (innermost — MUST NOT import any other substrate-* crate)
#   policy        : substrate-policy           (domain-only; MUST NOT import adapter crates)
#   infra-policy  : substrate-config           (config loader; may import domain + policy; not an adapter)
#   platform-shim : substrate-fs-index-macos-sys, substrate-signal-sys
#                   (thin OS-abstraction shims; may import domain; MUST NOT import adapter crates)
#   adapter       : substrate-fs-query, substrate-fs-mutation, substrate-fs-index,
#                   substrate-process, substrate-system-info, substrate-network-info,
#                   substrate-text, substrate-archive, substrate-jobs,
#                   substrate-subprocess
#                   (MUST NOT depend on each other; allowed: domain + policy)
#   server        : substrate-mcp-server       (only crate allowed to depend on all)
#
# Rule-5 exception (ADR-0056, documented in ADR-0022):
#   substrate-subprocess MAY activate the tokio net feature, but ONLY through its
#   own optional `outbound-net` Cargo feature, used for the active health-probe
#   path (TCP connect-checks). The exception is encoded narrowly: it fires solely
#   for crate_name == "substrate-subprocess" AND a dependency token that carries
#   the explicit `outbound-net` feature marker (e.g. "outbound-net:tokio/net").
#   A bare "tokio/net" on substrate-subprocess (net without the outbound-net gate)
#   and tokio net for EVERY other non-server adapter remain denied.
#
# Input shape:
#   {
#     "crate_name":      "substrate-fs-query",
#     "dependencies":    ["substrate-domain", "substrate-policy", "tokio", "rmcp"],
#     "allowed_external": ["tokio", "serde", "anyhow"]
#   }
#
# Test vectors (inline):
#
#   PASS — domain with no substrate-* deps
#   input = {"crate_name":"substrate-domain","dependencies":["serde","thiserror","time","serde_json"],"allowed_external":["serde","thiserror","time","serde_json"]}
#
#   PASS — policy depends on domain only
#   input = {"crate_name":"substrate-policy","dependencies":["substrate-domain","anyhow"],"allowed_external":["anyhow"]}
#
#   PASS — adapter depends on domain + policy
#   input = {"crate_name":"substrate-fs-query","dependencies":["substrate-domain","substrate-policy","tokio","serde"],"allowed_external":["tokio","serde"]}
#
#   PASS — server depends on all
#   input = {"crate_name":"substrate-mcp-server","dependencies":["substrate-domain","substrate-policy","substrate-fs-query","rmcp","tokio"],"allowed_external":["rmcp","tokio"]}
#
#   FAIL — domain imports another substrate-* crate
#   input = {"crate_name":"substrate-domain","dependencies":["substrate-policy"],"allowed_external":[]}
#   expected deny: "substrate-domain (domain layer) MUST NOT depend on substrate-policy"
#
#   FAIL — policy imports an adapter crate
#   input = {"crate_name":"substrate-policy","dependencies":["substrate-domain","substrate-fs-query"],"allowed_external":[]}
#   expected deny: "substrate-policy (policy layer) MUST NOT depend on adapter crate substrate-fs-query"
#
#   FAIL — adapter depends on another adapter
#   input = {"crate_name":"substrate-fs-query","dependencies":["substrate-domain","substrate-process"],"allowed_external":[]}
#   expected deny: "substrate-fs-query (adapter) MUST NOT depend on another adapter crate substrate-process"
#
#   FAIL — non-server crate pulls in rmcp
#   input = {"crate_name":"substrate-fs-query","dependencies":["substrate-domain","rmcp"],"allowed_external":["rmcp"]}
#   expected deny: "substrate-fs-query: only substrate-mcp-server may depend on rmcp (MCP wire layer)"
#
#   FAIL — non-server crate pulls in tokio net feature (detected by "tokio" + "net" marker)
#   input = {"crate_name":"substrate-fs-query","dependencies":["substrate-domain","tokio/net"],"allowed_external":["tokio/net"]}
#   expected deny: "substrate-fs-query: only substrate-mcp-server may activate tokio net feature"
#
#   PASS — substrate-network-info classified as adapter, depends on domain + policy
#   input = {"crate_name":"substrate-network-info","dependencies":["substrate-domain","substrate-policy","tokio"],"allowed_external":["tokio"]}
#
#   PASS — substrate-subprocess activates tokio net ONLY via its outbound-net feature (ADR-0056)
#   input = {"crate_name":"substrate-subprocess","dependencies":["substrate-domain","substrate-policy","outbound-net:tokio/net"],"allowed_external":["tokio/net"]}
#
#   FAIL — substrate-subprocess pulls tokio net WITHOUT the outbound-net gate (bare net not exempt)
#   input = {"crate_name":"substrate-subprocess","dependencies":["substrate-domain","tokio/net"],"allowed_external":["tokio/net"]}
#   expected deny: "substrate-subprocess: only substrate-mcp-server may activate tokio net feature"

package substrate.hexagonal

import rego.v1

# ---------------------------------------------------------------------------
# Crate classification helpers
# ---------------------------------------------------------------------------

_is_domain(name) if name == "substrate-domain"

_is_policy(name) if name == "substrate-policy"

# substrate-config is an infra-policy crate: may import domain + policy,
# but MUST NOT import adapter crates (same rule as policy layer).
_is_infra_policy(name) if name == "substrate-config"

# Platform shims are thin OS-abstraction crates: may import domain,
# MUST NOT import adapter crates or pull in rmcp / tokio-net.
_platform_shim_crates := {
    "substrate-fs-index-macos-sys",
    "substrate-signal-sys",
}

_is_platform_shim(name) if _platform_shim_crates[name]

_adapter_crates := {
    "substrate-fs-query",
    "substrate-fs-mutation",
    "substrate-fs-index",
    "substrate-process",
    "substrate-system-info",
    "substrate-network-info",
    "substrate-text",
    "substrate-archive",
    "substrate-jobs",
    "substrate-subprocess",
}

_is_adapter(name) if _adapter_crates[name]

_is_server(name) if name == "substrate-mcp-server"

_is_substrate(dep) if startswith(dep, "substrate-")

# ---------------------------------------------------------------------------
# Rule 1: domain MUST NOT import any other substrate-* crate
# ---------------------------------------------------------------------------

deny contains msg if {
    _is_domain(input.crate_name)
    dep := input.dependencies[_]
    _is_substrate(dep)
    msg := sprintf(
        "%s (domain layer) MUST NOT depend on %s",
        [input.crate_name, dep],
    )
}

# ---------------------------------------------------------------------------
# Rule 2: policy MUST NOT import adapter crates
# ---------------------------------------------------------------------------

deny contains msg if {
    _is_policy(input.crate_name)
    dep := input.dependencies[_]
    _is_adapter(dep)
    msg := sprintf(
        "%s (policy layer) MUST NOT depend on adapter crate %s",
        [input.crate_name, dep],
    )
}

# ---------------------------------------------------------------------------
# Rule 2b: infra-policy (substrate-config) MUST NOT import adapter crates
# ---------------------------------------------------------------------------

deny contains msg if {
    _is_infra_policy(input.crate_name)
    dep := input.dependencies[_]
    _is_adapter(dep)
    msg := sprintf(
        "%s (infra-policy layer) MUST NOT depend on adapter crate %s",
        [input.crate_name, dep],
    )
}

# ---------------------------------------------------------------------------
# Rule 2c: platform shims MUST NOT import adapter crates
# ---------------------------------------------------------------------------

deny contains msg if {
    _is_platform_shim(input.crate_name)
    dep := input.dependencies[_]
    _is_adapter(dep)
    msg := sprintf(
        "%s (platform shim) MUST NOT depend on adapter crate %s",
        [input.crate_name, dep],
    )
}

# ---------------------------------------------------------------------------
# Rule 3: adapter MUST NOT depend on another adapter
# ---------------------------------------------------------------------------

deny contains msg if {
    _is_adapter(input.crate_name)
    dep := input.dependencies[_]
    _is_adapter(dep)
    msg := sprintf(
        "%s (adapter) MUST NOT depend on another adapter crate %s",
        [input.crate_name, dep],
    )
}

# ---------------------------------------------------------------------------
# Rule 4: only substrate-mcp-server may depend on rmcp (MCP wire layer)
# ---------------------------------------------------------------------------

deny contains msg if {
    not _is_server(input.crate_name)
    dep := input.dependencies[_]
    dep == "rmcp"
    msg := sprintf(
        "%s: only substrate-mcp-server may depend on rmcp (MCP wire layer)",
        [input.crate_name],
    )
}

# ---------------------------------------------------------------------------
# Rule 5: only substrate-mcp-server may activate the tokio net feature
# Detected via the "tokio/net" feature-path notation in the dependency list.
#
# Documented exception (ADR-0056, recorded in ADR-0022): substrate-subprocess
# MAY activate tokio net, but ONLY through its optional `outbound-net` Cargo
# feature (active TCP connect health-probes). The exception is encoded narrowly
# by `_is_outbound_net_exception`: it fires solely for substrate-subprocess and
# only for a dependency token carrying the explicit "outbound-net:" marker.
# Bare "tokio/net" on substrate-subprocess, and tokio net on any other
# non-server adapter, are still denied.
# ---------------------------------------------------------------------------

# True when this net activation is the documented ADR-0056 exception: the crate
# is substrate-subprocess AND the dependency token is gated behind the
# `outbound-net` feature (encoded as the "outbound-net:" prefix marker).
_is_outbound_net_exception(crate_name, dep) if {
    crate_name == "substrate-subprocess"
    contains(dep, "outbound-net:")
}

deny contains msg if {
    not _is_server(input.crate_name)
    dep := input.dependencies[_]
    contains(dep, "tokio")
    contains(dep, "net")
    not _is_outbound_net_exception(input.crate_name, dep)
    msg := sprintf(
        "%s: only substrate-mcp-server may activate tokio net feature",
        [input.crate_name],
    )
}

# ---------------------------------------------------------------------------
# allow — true only when deny is empty
# ---------------------------------------------------------------------------

default allow := false

allow if {
    count(deny) == 0
}
