# package substrate.launch_invariants
#
# Structural invariants for the launch bounded context (ADR-0063..ADR-0069).
# These are spec-level invariants checked against a serialized Profile + trust
# store snapshot, NOT a code-shape lint (that is hexagonal_layering / no_subprocess).
#
# Rule 1 — dependency-graph acyclicity (ADR-0065): the depends_on edges of a
#   Profile MUST form a directed acyclic graph. A service that is reachable from
#   its own successors participates in a cycle and is denied (self-loops and
#   multi-node cycles both caught via graph.reachable).
#
# Rule 2 — trust-record identity floor (ADR-0064): every TrustRecord pins a
#   (dev, ino, ...) tuple from fstat. A valid inode number is >= 1 and a valid
#   device id is >= 1; a record carrying ino < 1 or dev < 1 is malformed and is
#   denied (it would let a zeroed/forged tuple match a never-stat'd file).
#
# Input shape (provided by the CI conftest adapter):
#   {
#     "profile": {
#       "services": {
#         "<name>": { "depends_on": [ { "service": "<name>", "required": <bool> }, ... ] },
#         ...
#       }
#     },
#     "trust_records": [ { "dev": <int>, "ino": <int> }, ... ]
#   }
#
# Test vectors (inline):
#
#   PASS — acyclic graph + valid trust records
#   input = {"profile":{"services":{"db":{"depends_on":[]},"api":{"depends_on":[{"service":"db","required":true}]}}},"trust_records":[{"dev":66,"ino":1234}]}
#
#   FAIL — two-node cycle a<->b
#   input = {"profile":{"services":{"a":{"depends_on":[{"service":"b","required":true}]},"b":{"depends_on":[{"service":"a","required":true}]}}},"trust_records":[]}
#   expected deny: "dependency cycle through service 'a' — depends_on must form a DAG (ADR-0065)"
#
#   FAIL — self-loop a->a
#   input = {"profile":{"services":{"a":{"depends_on":[{"service":"a","required":true}]}}},"trust_records":[]}
#   expected deny: "dependency cycle through service 'a' — depends_on must form a DAG (ADR-0065)"
#
#   FAIL — trust record with ino < 1
#   input = {"profile":{"services":{}},"trust_records":[{"dev":66,"ino":0}]}
#   expected deny: "trust record 0 has ino 0 — a valid inode is >= 1 (ADR-0064)"
#
#   FAIL — trust record with dev < 1
#   input = {"profile":{"services":{}},"trust_records":[{"dev":0,"ino":12}]}
#   expected deny: "trust record 0 has dev 0 — a valid device id is >= 1 (ADR-0064)"

package substrate.launch_invariants

import rego.v1

# ---------------------------------------------------------------------------
# Dependency adjacency: every service name maps to the set of services it
# depends on. object.get defaults depends_on to [] so a service with no edges
# is still a graph node (graph.reachable treats a missing key as no out-edges).
# ---------------------------------------------------------------------------

_dep_graph[name] := deps if {
	some name, svc in input.profile.services
	deps := {e.service | some e in object.get(svc, "depends_on", [])}
}

# ---------------------------------------------------------------------------
# Rule 1: depends_on MUST form a DAG (ADR-0065).
# A service participates in a cycle when it is reachable from its own direct
# successors. graph.reachable follows edges transitively, so this catches
# self-loops and multi-node cycles alike.
# ---------------------------------------------------------------------------

deny contains msg if {
	some name
	input.profile.services[name]
	reachable := graph.reachable(_dep_graph, _dep_graph[name])
	reachable[name]
	msg := sprintf(
		"dependency cycle through service '%s' — depends_on must form a DAG (ADR-0065)",
		[name],
	)
}

# ---------------------------------------------------------------------------
# Rule 2: TrustRecord identity floor (ADR-0064).
# A valid inode is >= 1 and a valid device id is >= 1.
# ---------------------------------------------------------------------------

deny contains msg if {
	some i, rec in input.trust_records
	rec.ino < 1
	msg := sprintf(
		"trust record %d has ino %d — a valid inode is >= 1 (ADR-0064)",
		[i, rec.ino],
	)
}

deny contains msg if {
	some i, rec in input.trust_records
	rec.dev < 1
	msg := sprintf(
		"trust record %d has dev %d — a valid device id is >= 1 (ADR-0064)",
		[i, rec.dev],
	)
}

# ---------------------------------------------------------------------------
# allow — true only when deny is empty
# ---------------------------------------------------------------------------

default allow := false

allow if count(deny) == 0
