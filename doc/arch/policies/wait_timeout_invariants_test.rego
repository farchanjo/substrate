package substrate.wait_timeout_invariants_test

import data.substrate.wait_timeout_invariants

# ---------------------------------------------------------------------------
# PASS cases
# ---------------------------------------------------------------------------

test_minimal_valid if {
	wait_timeout_invariants.allow with input as {
		"tool": "subprocess.result",
		"schema": {"properties": {"wait_ms": {
			"type": "integer",
			"minimum": 0,
			"default": 5000,
		}}},
		"config": {
			"result_max_wait_ms": 30000,
			"result_default_wait_ms": 5000,
		},
	}
}

test_with_maximum_within_cap if {
	wait_timeout_invariants.allow with input as {
		"tool": "job.result",
		"schema": {"properties": {"wait_ms": {
			"type": "integer",
			"minimum": 0,
			"default": 5000,
			"maximum": 30000,
		}}},
		"config": {
			"result_max_wait_ms": 30000,
			"result_default_wait_ms": 5000,
		},
	}
}

# ---------------------------------------------------------------------------
# FAIL cases
# ---------------------------------------------------------------------------

test_fail_default_zero if {
	result := wait_timeout_invariants.deny with input as {
		"tool": "subprocess.result",
		"schema": {"properties": {"wait_ms": {
			"type": "integer",
			"minimum": 0,
			"default": 0,
		}}},
		"config": {
			"result_max_wait_ms": 30000,
			"result_default_wait_ms": 5000,
		},
	}
	count(result) > 0
	some msg in result
	contains(msg, "wait_ms.default must be > 0")
}

test_fail_default_missing if {
	result := wait_timeout_invariants.deny with input as {
		"tool": "subprocess.result",
		"schema": {"properties": {"wait_ms": {
			"type": "integer",
			"minimum": 0,
		}}},
		"config": {
			"result_max_wait_ms": 30000,
			"result_default_wait_ms": 5000,
		},
	}
	count(result) > 0
	some msg in result
	contains(msg, "wait_ms.default is required")
}

test_fail_maximum_exceeds_cap if {
	result := wait_timeout_invariants.deny with input as {
		"tool": "subprocess.result",
		"schema": {"properties": {"wait_ms": {
			"type": "integer",
			"minimum": 0,
			"default": 5000,
			"maximum": 60000,
		}}},
		"config": {
			"result_max_wait_ms": 30000,
			"result_default_wait_ms": 5000,
		},
	}
	count(result) > 0
	some msg in result
	contains(msg, "must be <= result_max_wait_ms")
}

test_fail_default_drift if {
	result := wait_timeout_invariants.deny with input as {
		"tool": "subprocess.result",
		"schema": {"properties": {"wait_ms": {
			"type": "integer",
			"minimum": 0,
			"default": 2000,
		}}},
		"config": {
			"result_max_wait_ms": 30000,
			"result_default_wait_ms": 5000,
		},
	}
	count(result) > 0
	some msg in result
	contains(msg, "must match config.result_default_wait_ms")
}

test_fail_minimum_not_zero if {
	result := wait_timeout_invariants.deny with input as {
		"tool": "subprocess.result",
		"schema": {"properties": {"wait_ms": {
			"type": "integer",
			"minimum": 1,
			"default": 5000,
		}}},
		"config": {
			"result_max_wait_ms": 30000,
			"result_default_wait_ms": 5000,
		},
	}
	count(result) > 0
	some msg in result
	contains(msg, "wait_ms.minimum must be 0")
}
