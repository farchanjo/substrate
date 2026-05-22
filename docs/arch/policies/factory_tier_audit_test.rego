package substrate.factory_tier_audit

import rego.v1

# ---------------------------------------------------------------------------
# Tests for Rule 1: every Port trait MUST have a PortFactory implementation
# ---------------------------------------------------------------------------

test_port_with_factory_and_audit_field_allowed if {
    count(deny) == 0 with input as {
        "domain_ports": ["HashPort"],
        "factory_implementations": ["HashPortFactory"],
        "audit_event_fields": ["hash_tier"],
    }
}

test_port_without_factory_denied if {
    deny["domain port FsIndexPort has no corresponding PortFactory implementation — per ADR-0042"] with input as {
        "domain_ports": ["FsIndexPort"],
        "factory_implementations": [],
        "audit_event_fields": ["fs_index_tier"],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 2: every Port trait MUST have an audit event entry
# ---------------------------------------------------------------------------

test_port_with_factory_but_missing_audit_field_denied if {
    deny["domain port JobRegistryPort has no entry in SUBSTRATE_CAPABILITY_TIERS_SELECTED audit event — per ADR-0042"] with input as {
        "domain_ports": ["JobRegistryPort"],
        "factory_implementations": ["JobRegistryPortFactory"],
        "audit_event_fields": [],
    }
}

test_multiple_ports_all_wired_allowed if {
    count(deny) == 0 with input as {
        "domain_ports": ["HashPort", "IndexPort"],
        "factory_implementations": ["HashPortFactory", "IndexPortFactory"],
        "audit_event_fields": ["hash_tier", "index_tier"],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 3: orphan factories (no matching domain port) are denied
# ---------------------------------------------------------------------------

test_orphan_factory_denied if {
    deny["factory DirWalkerPortFactory has no corresponding domain port DirWalkerPort — stale factory, per ADR-0042"] with input as {
        "domain_ports": [],
        "factory_implementations": ["DirWalkerPortFactory"],
        "audit_event_fields": [],
    }
}
