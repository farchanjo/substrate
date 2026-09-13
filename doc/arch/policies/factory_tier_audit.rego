# package substrate.factory_tier_audit
#
# Enforces the capability adapter factory audit discipline for substrate.
# Per ADR-0042: every port type declared in substrate-domain (trait whose name
# ends with "Port") MUST have a corresponding PortFactory implementation AND
# an entry in the SUBSTRATE_CAPABILITY_TIERS_SELECTED startup audit event.
# Violations indicate that a new port was added without the required factory
# wiring or audit event registration.
#
# This policy operates on a structured input document produced by the CI
# conftest adapter at build time (not a Rust AST parser; the input is derived
# from cargo metadata + source grep output).
#
# Input shape:
#   {
#     "domain_ports": ["<TraitName>Port", ...],
#     "factory_implementations": ["<TraitName>PortFactory", ...],
#     "audit_event_fields": ["<field_name>", ...]
#   }
#
# Where:
#   domain_ports           — list of trait names ending in "Port" discovered
#                            in substrate-domain source files (via source grep
#                            for "^pub trait .*Port" patterns).
#   factory_implementations — list of struct or impl names ending in
#                            "PortFactory" discovered in adapter crate source
#                            files (via source grep for "impl PortFactory").
#   audit_event_fields     — list of field name strings present in the
#                            SUBSTRATE_CAPABILITY_TIERS_SELECTED structured
#                            audit event schema (from CUE schema or JSON Schema).
#
# Naming convention per ADR-0042:
#   Port trait   : <Name>Port           (e.g., DirWalkerPort, HashPort)
#   Factory impl : <Name>PortFactory    (e.g., DirWalkerPortFactory)
#   Audit field  : <snake_case_name>    (e.g., dir_walker_tier, hash_tier)
#
# Test vectors (inline):
#
#   PASS — DirWalkerPort has factory and audit field
#   input = {
#     "domain_ports": ["DirWalkerPort"],
#     "factory_implementations": ["DirWalkerPortFactory"],
#     "audit_event_fields": ["dir_walker_tier"]
#   }
#
#   FAIL — FsIndexPort declared but factory missing
#   input = {
#     "domain_ports": ["FsIndexPort"],
#     "factory_implementations": [],
#     "audit_event_fields": ["fs_index_tier"]
#   }
#   expected deny: "domain port FsIndexPort has no corresponding PortFactory implementation — per ADR-0042"
#
#   FAIL — JobRegistryPort declared and has factory but missing audit field
#   input = {
#     "domain_ports": ["JobRegistryPort"],
#     "factory_implementations": ["JobRegistryPortFactory"],
#     "audit_event_fields": []
#   }
#   expected deny: "domain port JobRegistryPort has no entry in SUBSTRATE_CAPABILITY_TIERS_SELECTED audit event — per ADR-0042"

package substrate.factory_tier_audit

import rego.v1

# ---------------------------------------------------------------------------
# Helper: derive the expected factory name from a port trait name
# Convention (ADR-0042): Port trait "FooPort" => factory "FooPortFactory".
# ---------------------------------------------------------------------------

_expected_factory(port_trait) := factory_name if {
    factory_name := sprintf("%sFactory", [port_trait])
}

# ---------------------------------------------------------------------------
# Helper: derive the expected audit event field from a port trait name
# Convention: "FooBarPort" => "foo_bar_tier" (PascalCase to snake_case,
# strip "Port" suffix, append "_tier").
# Since Rego lacks a built-in PascalCase-to-snake_case converter, the CI
# adapter is expected to also populate audit_event_fields with canonical
# names. The policy checks that at least one audit field contains the stem
# of the port name in lowercase (best-effort substring match).
# ---------------------------------------------------------------------------

_port_stem(port_trait) := stem if {
    # Strip trailing "Port" (4 characters) to get the stem.
    stem := substring(port_trait, 0, count(port_trait) - 4)
}

_audit_field_matches_port(port_trait) if {
    stem := lower(_port_stem(port_trait))
    some i
    field := input.audit_event_fields[i]
    contains(field, stem)
}

# ---------------------------------------------------------------------------
# Rule 1: every Port trait MUST have a PortFactory implementation
# Per ADR-0042: PortFactory<P>.build() is the only accepted mechanism for
# selecting a tier implementation. A port without a factory cannot be wired
# into the composition root and will cause a startup failure.
# ---------------------------------------------------------------------------

_has_factory(expected) if {
    expected == input.factory_implementations[_]
}

_is_declared_port(port_name) if {
    port_name == input.domain_ports[_]
}

deny contains msg if {
    some i
    port_trait := input.domain_ports[i]
    endswith(port_trait, "Port")
    expected := _expected_factory(port_trait)
    not _has_factory(expected)
    msg := sprintf(
        "domain port %s has no corresponding PortFactory implementation — per ADR-0042",
        [port_trait],
    )
}

# ---------------------------------------------------------------------------
# Rule 2: every Port trait MUST have an entry in the
# SUBSTRATE_CAPABILITY_TIERS_SELECTED audit event
# Per ADR-0042: the audit event provides operators with a single forensic line
# showing the entire tier selection. A missing field makes the startup posture
# opaque to the audit pipeline.
# ---------------------------------------------------------------------------

deny contains msg if {
    some j
    port_trait := input.domain_ports[j]
    endswith(port_trait, "Port")
    not _audit_field_matches_port(port_trait)
    msg := sprintf(
        "domain port %s has no entry in SUBSTRATE_CAPABILITY_TIERS_SELECTED audit event — per ADR-0042",
        [port_trait],
    )
}

# ---------------------------------------------------------------------------
# Rule 3: factory_implementations MUST NOT declare factories for unknown ports
# Per ADR-0042: orphan factories (no matching domain port) indicate a stale
# factory that was not removed when its port was superseded or deleted.
# Orphan factories may shadow the correct factory for a renamed port.
# ---------------------------------------------------------------------------

deny contains msg if {
    some k
    factory_name := input.factory_implementations[k]
    endswith(factory_name, "PortFactory")
    # Derive the port name the factory is expected to serve.
    stem_len := count(factory_name) - count("Factory")
    port_name := substring(factory_name, 0, stem_len)
    not _is_declared_port(port_name)
    msg := sprintf(
        "factory %s has no corresponding domain port %s — stale factory, per ADR-0042",
        [factory_name, port_name],
    )
}

# ---------------------------------------------------------------------------
# allow — true only when deny is empty
# ---------------------------------------------------------------------------

default allow := false

allow if count(deny) == 0
