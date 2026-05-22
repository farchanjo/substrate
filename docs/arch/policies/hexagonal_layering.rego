# package substrate.hexagonal
#
# Enforces the hexagonal layering rules for the substrate crate stack.
# Violations indicate a dependency that crosses an architectural boundary.
#
# Crate taxonomy:
#   domain   : substrate-domain          (innermost — MUST NOT import any other substrate-* crate)
#   policy   : substrate-policy          (domain-only; MUST NOT import adapter crates)
#   adapter  : substrate-fs, substrate-proc, substrate-sys, substrate-text, substrate-archive
#              (MUST NOT depend on each other; allowed: domain + policy)
#   server   : substrate-mcp-server      (only crate allowed to depend on all)
#
# Input shape:
#   {
#     "crate_name":      "substrate-fs",
#     "dependencies":    ["substrate-domain", "substrate-policy", "tokio", "rmcp"],
#     "allowed_external": ["tokio", "serde", "anyhow"]
#   }
#
# Test vectors (inline):
#
#   PASS — domain with no substrate-* deps
#   input = {"crate_name":"substrate-domain","dependencies":["serde","thiserror"],"allowed_external":["serde","thiserror"]}
#
#   PASS — policy depends on domain only
#   input = {"crate_name":"substrate-policy","dependencies":["substrate-domain","anyhow"],"allowed_external":["anyhow"]}
#
#   PASS — adapter depends on domain + policy
#   input = {"crate_name":"substrate-fs","dependencies":["substrate-domain","substrate-policy","tokio","serde"],"allowed_external":["tokio","serde"]}
#
#   PASS — server depends on all
#   input = {"crate_name":"substrate-mcp-server","dependencies":["substrate-domain","substrate-policy","substrate-fs","rmcp","tokio"],"allowed_external":["rmcp","tokio"]}
#
#   FAIL — domain imports another substrate-* crate
#   input = {"crate_name":"substrate-domain","dependencies":["substrate-policy"],"allowed_external":[]}
#   expected deny: "substrate-domain (domain layer) MUST NOT depend on substrate-policy"
#
#   FAIL — policy imports an adapter crate
#   input = {"crate_name":"substrate-policy","dependencies":["substrate-domain","substrate-fs"],"allowed_external":[]}
#   expected deny: "substrate-policy (policy layer) MUST NOT depend on adapter crate substrate-fs"
#
#   FAIL — adapter depends on another adapter
#   input = {"crate_name":"substrate-fs","dependencies":["substrate-domain","substrate-proc"],"allowed_external":[]}
#   expected deny: "substrate-fs (adapter) MUST NOT depend on another adapter crate substrate-proc"
#
#   FAIL — non-server crate pulls in rmcp
#   input = {"crate_name":"substrate-fs","dependencies":["substrate-domain","rmcp"],"allowed_external":["rmcp"]}
#   expected deny: "substrate-fs: only substrate-mcp-server may depend on rmcp (MCP wire layer)"
#
#   FAIL — non-server crate pulls in tokio net feature (detected by "tokio" + "net" marker)
#   input = {"crate_name":"substrate-fs","dependencies":["substrate-domain","tokio/net"],"allowed_external":["tokio/net"]}
#   expected deny: "substrate-fs: only substrate-mcp-server may activate tokio net feature"

package substrate.hexagonal

import rego.v1

# ---------------------------------------------------------------------------
# Crate classification helpers
# ---------------------------------------------------------------------------

_is_domain(name) if name == "substrate-domain"

_is_policy(name) if name == "substrate-policy"

_adapter_crates := {
    "substrate-fs",
    "substrate-proc",
    "substrate-sys",
    "substrate-text",
    "substrate-archive",
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
# ---------------------------------------------------------------------------

deny contains msg if {
    not _is_server(input.crate_name)
    dep := input.dependencies[_]
    contains(dep, "tokio")
    contains(dep, "net")
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
