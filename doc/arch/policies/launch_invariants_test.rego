package substrate.launch_invariants

import rego.v1

# ---------------------------------------------------------------------------
# Rule 1: dependency-graph acyclicity (ADR-0065)
# ---------------------------------------------------------------------------

test_acyclic_graph_allowed if {
	count(deny) == 0 with input as {
		"profile": {"services": {
			"db": {"depends_on": []},
			"api": {"depends_on": [{"service": "db", "required": true}]},
			"web": {"depends_on": [{"service": "api", "required": true}]},
		}},
		"trust_records": [{"dev": 66, "ino": 1234}],
	}
}

test_two_node_cycle_denied if {
	deny["dependency cycle through service 'a' — depends_on must form a DAG (ADR-0065)"] with input as {
		"profile": {"services": {
			"a": {"depends_on": [{"service": "b", "required": true}]},
			"b": {"depends_on": [{"service": "a", "required": true}]},
		}},
		"trust_records": [],
	}
}

test_self_loop_denied if {
	deny["dependency cycle through service 'a' — depends_on must form a DAG (ADR-0065)"] with input as {
		"profile": {"services": {"a": {"depends_on": [{"service": "a", "required": true}]}}},
		"trust_records": [],
	}
}

# ---------------------------------------------------------------------------
# Rule 2: trust-record identity floor (ADR-0064)
# ---------------------------------------------------------------------------

test_valid_trust_record_allowed if {
	count(deny) == 0 with input as {
		"profile": {"services": {}},
		"trust_records": [{"dev": 66, "ino": 1234}],
	}
}

test_trust_record_zero_ino_denied if {
	deny["trust record 0 has ino 0 — a valid inode is >= 1 (ADR-0064)"] with input as {
		"profile": {"services": {}},
		"trust_records": [{"dev": 66, "ino": 0}],
	}
}

test_trust_record_zero_dev_denied if {
	deny["trust record 0 has dev 0 — a valid device id is >= 1 (ADR-0064)"] with input as {
		"profile": {"services": {}},
		"trust_records": [{"dev": 0, "ino": 12}],
	}
}

# ---------------------------------------------------------------------------
# Combined: empty profile and empty trust store are vacuously valid
# ---------------------------------------------------------------------------

test_empty_input_allowed if {
	count(deny) == 0 with input as {
		"profile": {"services": {}},
		"trust_records": [],
	}
}
