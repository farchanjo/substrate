package substrate.hexagonal

import rego.v1

# ---------------------------------------------------------------------------
# Tests for Rule 1: domain MUST NOT import any other substrate-* crate
# ---------------------------------------------------------------------------

test_domain_with_no_substrate_deps_allowed if {
    count(deny) == 0 with input as {
        "crate_name": "substrate-domain",
        "dependencies": ["serde", "thiserror", "tracing"],
        "allowed_external": ["serde", "thiserror", "tracing"],
    }
}

test_domain_importing_substrate_policy_denied if {
    deny["substrate-domain (domain layer) MUST NOT depend on substrate-policy"] with input as {
        "crate_name": "substrate-domain",
        "dependencies": ["substrate-policy"],
        "allowed_external": [],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 2: policy MUST NOT import adapter crates
# ---------------------------------------------------------------------------

test_policy_depending_on_domain_only_allowed if {
    count(deny) == 0 with input as {
        "crate_name": "substrate-policy",
        "dependencies": ["substrate-domain", "anyhow"],
        "allowed_external": ["anyhow"],
    }
}

test_policy_importing_adapter_denied if {
    deny["substrate-policy (policy layer) MUST NOT depend on adapter crate substrate-fs-query"] with input as {
        "crate_name": "substrate-policy",
        "dependencies": ["substrate-domain", "substrate-fs-query"],
        "allowed_external": [],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 3: adapter MUST NOT depend on another adapter
# ---------------------------------------------------------------------------

test_adapter_depending_on_domain_and_policy_allowed if {
    count(deny) == 0 with input as {
        "crate_name": "substrate-fs-query",
        "dependencies": ["substrate-domain", "substrate-policy", "tokio", "serde"],
        "allowed_external": ["tokio", "serde"],
    }
}

test_adapter_depending_on_another_adapter_denied if {
    deny["substrate-fs-query (adapter) MUST NOT depend on another adapter crate substrate-process"] with input as {
        "crate_name": "substrate-fs-query",
        "dependencies": ["substrate-domain", "substrate-process"],
        "allowed_external": [],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 4: only substrate-mcp-server may depend on rmcp
# ---------------------------------------------------------------------------

test_server_depending_on_rmcp_allowed if {
    count(deny) == 0 with input as {
        "crate_name": "substrate-mcp-server",
        "dependencies": ["substrate-domain", "substrate-fs-query", "rmcp", "tokio"],
        "allowed_external": ["rmcp", "tokio"],
    }
}

test_adapter_depending_on_rmcp_denied if {
    deny["substrate-fs-query: only substrate-mcp-server may depend on rmcp (MCP wire layer)"] with input as {
        "crate_name": "substrate-fs-query",
        "dependencies": ["substrate-domain", "rmcp"],
        "allowed_external": ["rmcp"],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 5: only substrate-mcp-server may activate tokio net feature
# ---------------------------------------------------------------------------

test_adapter_activating_tokio_net_denied if {
    deny["substrate-fs-query: only substrate-mcp-server may activate tokio net feature"] with input as {
        "crate_name": "substrate-fs-query",
        "dependencies": ["substrate-domain", "tokio/net"],
        "allowed_external": ["tokio/net"],
    }
}
