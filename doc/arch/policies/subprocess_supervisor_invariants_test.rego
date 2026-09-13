package substrate.subprocess_supervisor_invariants_test

import rego.v1

import data.substrate.subprocess_supervisor_invariants

# ---------------------------------------------------------------------------
# Shared fixture helpers
# ---------------------------------------------------------------------------

# A fully valid supervisor request used as a base for targeted mutations.
_valid_full := {
	"name": "spring-backend",
	"capture_kind": "tmp_file",
	"restart_policy": {
		"kind": "OnFailure",
		"max_retries": 3,
		"backoff_ms": 1000,
	},
	"health_probe": {
		"kind": "HttpGet",
		"url": "http://localhost:8080/health",
		"expected_status": 200,
		"interval_ms": 5000,
		"startup_grace_ms": 30000,
	},
	"log_rotation": {
		"kind": "BySize",
		"max_bytes_per_file": 10485760,
		"keep_files": 5,
	},
}

# ---------------------------------------------------------------------------
# Invariant 1 — name format: ^[a-z0-9-]{1,64}$
# ---------------------------------------------------------------------------

# PASS — valid lowercase-alphanumeric-hyphen name
test_name_valid_lowercase_alphanumeric if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"name": "spring-backend"},
	}
}

# PASS — single-character name satisfies pattern
test_name_single_char_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"name": "a"},
	}
}

# FAIL — uppercase letters in name
test_name_invalid_uppercase if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"name": "Spring-Backend"},
	}
	some msg
	result[msg]
	contains(msg, "name must match ^[a-z0-9-]{1,64}$")
}

# FAIL — name contains underscore (not in allowed charset)
test_name_invalid_underscore if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"name": "my_service"},
	}
	some msg
	result[msg]
	contains(msg, "name must match ^[a-z0-9-]{1,64}$")
}

# FAIL — empty name fails length constraint
test_name_invalid_empty if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"name": ""},
	}
	some msg
	result[msg]
	contains(msg, "name must match ^[a-z0-9-]{1,64}$")
}

# ---------------------------------------------------------------------------
# Invariant 2 — restart_policy.kind enum: Never | OnFailure | Always
# ---------------------------------------------------------------------------

# PASS — kind=Never is valid
test_restart_kind_never_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"restart_policy": {"kind": "Never"}},
	}
}

# PASS — kind=Always is valid
test_restart_kind_always_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "Always",
			"backoff_ms": 500,
		}},
	}
}

# FAIL — arbitrary string rejected
test_restart_kind_invalid_string if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"restart_policy": {"kind": "Restart"}},
	}
	some msg
	result[msg]
	contains(msg, "restart_policy.kind must be Never|OnFailure|Always")
}

# FAIL — lowercase variant not accepted (enum is PascalCase)
test_restart_kind_invalid_lowercase if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"restart_policy": {"kind": "onfailure"}},
	}
	some msg
	result[msg]
	contains(msg, "restart_policy.kind must be Never|OnFailure|Always")
}

# ---------------------------------------------------------------------------
# Invariant 3a — OnFailure max_retries in [1, 100]
# ---------------------------------------------------------------------------

# PASS — max_retries at lower boundary
test_on_failure_max_retries_at_min if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "OnFailure",
			"max_retries": 1,
			"backoff_ms": 500,
		}},
	}
}

# PASS — max_retries at upper boundary
test_on_failure_max_retries_at_max if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "OnFailure",
			"max_retries": 100,
			"backoff_ms": 500,
		}},
	}
}

# FAIL — max_retries = 0 is below lower bound
test_on_failure_max_retries_zero_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "OnFailure",
			"max_retries": 0,
			"backoff_ms": 1000,
		}},
	}
	some msg
	result[msg]
	contains(msg, "max_retries must be in [1, 100]")
}

# FAIL — max_retries = 101 exceeds upper bound
test_on_failure_max_retries_over_max_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "OnFailure",
			"max_retries": 101,
			"backoff_ms": 1000,
		}},
	}
	some msg
	result[msg]
	contains(msg, "max_retries must be in [1, 100]")
}

# ---------------------------------------------------------------------------
# Invariant 3b — OnFailure backoff_ms in [100, 300000]
# ---------------------------------------------------------------------------

# PASS — backoff_ms at lower boundary
test_on_failure_backoff_at_min if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "OnFailure",
			"max_retries": 5,
			"backoff_ms": 100,
		}},
	}
}

# PASS — backoff_ms at upper boundary
test_on_failure_backoff_at_max if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "OnFailure",
			"max_retries": 5,
			"backoff_ms": 300000,
		}},
	}
}

# FAIL — backoff_ms = 99 is below lower bound
test_on_failure_backoff_under_min_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "OnFailure",
			"max_retries": 3,
			"backoff_ms": 99,
		}},
	}
	some msg
	result[msg]
	contains(msg, "backoff_ms must be in [100, 300000]")
}

# FAIL — backoff_ms = 300001 exceeds upper bound
test_on_failure_backoff_over_max_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "OnFailure",
			"max_retries": 3,
			"backoff_ms": 300001,
		}},
	}
	some msg
	result[msg]
	contains(msg, "backoff_ms must be in [100, 300000]")
}

# ---------------------------------------------------------------------------
# Invariant 4 — Always backoff_ms in [100, 300000]
# ---------------------------------------------------------------------------

# PASS — backoff_ms = 100 at lower boundary for Always
test_always_backoff_at_min if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "Always",
			"backoff_ms": 100,
		}},
	}
}

# PASS — backoff_ms = 300000 at upper boundary for Always
test_always_backoff_at_max if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "Always",
			"backoff_ms": 300000,
		}},
	}
}

# FAIL — backoff_ms below lower bound for Always
test_always_backoff_under_min_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "Always",
			"backoff_ms": 50,
		}},
	}
	some msg
	result[msg]
	contains(msg, "backoff_ms must be in [100, 300000]")
}

# FAIL — backoff_ms exceeds upper bound for Always
test_always_backoff_over_max_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"restart_policy": {
			"kind": "Always",
			"backoff_ms": 400000,
		}},
	}
	some msg
	result[msg]
	contains(msg, "backoff_ms must be in [100, 300000]")
}

# ---------------------------------------------------------------------------
# Invariant 5 — health_probe.kind enum: None | HttpGet | PortOpen | LogPattern
# ---------------------------------------------------------------------------

# PASS — kind=None is valid
test_probe_kind_none_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {"kind": "None"}},
	}
}

# PASS — kind=PortOpen is valid (with required fields)
test_probe_kind_port_open_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "PortOpen",
			"port": 8080,
			"interval_ms": 5000,
			"startup_grace_ms": 30000,
		}},
	}
}

# FAIL — unrecognized probe kind
test_probe_kind_invalid_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {"kind": "TCPConnect"}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.kind must be None|HttpGet|PortOpen|LogPattern")
}

# FAIL — empty string is not a valid probe kind
test_probe_kind_empty_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {"kind": ""}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.kind must be None|HttpGet|PortOpen|LogPattern")
}

# ---------------------------------------------------------------------------
# Invariant 6 — HttpGet url must start with http:// or https://
# ---------------------------------------------------------------------------

# PASS — http:// prefix is valid
test_httpget_url_http_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "http://localhost:8080/health",
			"expected_status": 200,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
}

# PASS — https:// prefix is valid
test_httpget_url_https_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "https://api.example.com/health",
			"expected_status": 200,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
}

# FAIL — ftp:// scheme rejected
test_httpget_url_ftp_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "ftp://files.example.com/health",
			"expected_status": 200,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.url must start with http:// or https://")
}

# FAIL — bare hostname without scheme rejected
test_httpget_url_no_scheme_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "localhost:8080/health",
			"expected_status": 200,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.url must start with http:// or https://")
}

# ---------------------------------------------------------------------------
# Invariant 7 — HttpGet expected_status in [100, 599]
# ---------------------------------------------------------------------------

# PASS — expected_status at lower boundary
test_httpget_status_at_min if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "http://localhost/health",
			"expected_status": 100,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
}

# PASS — expected_status at upper boundary
test_httpget_status_at_max if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "http://localhost/health",
			"expected_status": 599,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
}

# FAIL — expected_status = 99 below HTTP range
test_httpget_status_below_100_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "http://localhost/health",
			"expected_status": 99,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.expected_status must be in [100, 599]")
}

# FAIL — expected_status = 600 above HTTP range
test_httpget_status_above_599_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "http://localhost/health",
			"expected_status": 600,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.expected_status must be in [100, 599]")
}

# ---------------------------------------------------------------------------
# Invariant 8 — HttpGet / PortOpen interval_ms in [100, 60000]
# ---------------------------------------------------------------------------

# PASS — HttpGet interval_ms at lower boundary
test_httpget_interval_at_min if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "http://localhost/health",
			"expected_status": 200,
			"interval_ms": 100,
			"startup_grace_ms": 0,
		}},
	}
}

# PASS — PortOpen interval_ms at upper boundary
test_portopen_interval_at_max if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "PortOpen",
			"port": 8080,
			"interval_ms": 60000,
			"startup_grace_ms": 0,
		}},
	}
}

# FAIL — HttpGet interval_ms = 99 below lower bound
test_httpget_interval_below_min_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "http://localhost/health",
			"expected_status": 200,
			"interval_ms": 99,
			"startup_grace_ms": 0,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.interval_ms must be in [100, 60000]")
}

# FAIL — PortOpen interval_ms = 60001 above upper bound
test_portopen_interval_above_max_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "PortOpen",
			"port": 8080,
			"interval_ms": 60001,
			"startup_grace_ms": 0,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.interval_ms must be in [100, 60000]")
}

# ---------------------------------------------------------------------------
# Invariant 9 — HttpGet / PortOpen startup_grace_ms in [0, 600000]
# ---------------------------------------------------------------------------

# PASS — startup_grace_ms = 0 (lower boundary, allowed)
test_httpget_grace_at_zero if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "http://localhost/health",
			"expected_status": 200,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
}

# PASS — startup_grace_ms at upper boundary
test_portopen_grace_at_max if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "PortOpen",
			"port": 8080,
			"interval_ms": 5000,
			"startup_grace_ms": 600000,
		}},
	}
}

# FAIL — startup_grace_ms negative
test_httpget_grace_negative_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "HttpGet",
			"url": "http://localhost/health",
			"expected_status": 200,
			"interval_ms": 5000,
			"startup_grace_ms": -1,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.startup_grace_ms must be in [0, 600000]")
}

# FAIL — startup_grace_ms above 600000
test_portopen_grace_over_max_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "PortOpen",
			"port": 8080,
			"interval_ms": 5000,
			"startup_grace_ms": 600001,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.startup_grace_ms must be in [0, 600000]")
}

# ---------------------------------------------------------------------------
# Invariant 10 — PortOpen port in [1, 65535]
# ---------------------------------------------------------------------------

# PASS — port at lower boundary
test_portopen_port_at_min if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "PortOpen",
			"port": 1,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
}

# PASS — port at upper boundary
test_portopen_port_at_max if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "PortOpen",
			"port": 65535,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
}

# FAIL — port = 0 is below lower bound (reserved / invalid)
test_portopen_port_zero_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "PortOpen",
			"port": 0,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.port must be in [1, 65535]")
}

# FAIL — port = 65536 is above upper bound
test_portopen_port_above_max_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "PortOpen",
			"port": 65536,
			"interval_ms": 5000,
			"startup_grace_ms": 0,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.port must be in [1, 65535]")
}

# ---------------------------------------------------------------------------
# Invariant 11 — LogPattern timeout_ms in [1000, 600000]
# ---------------------------------------------------------------------------

# PASS — timeout_ms at lower boundary
test_logpattern_timeout_at_min if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "LogPattern",
			"regex": "started",
			"timeout_ms": 1000,
		}},
	}
}

# PASS — timeout_ms at upper boundary
test_logpattern_timeout_at_max if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "LogPattern",
			"regex": "ready",
			"timeout_ms": 600000,
		}},
	}
}

# FAIL — timeout_ms = 999 below lower bound
test_logpattern_timeout_below_min_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "LogPattern",
			"regex": "started",
			"timeout_ms": 999,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.timeout_ms must be in [1000, 600000]")
}

# FAIL — timeout_ms = 600001 above upper bound
test_logpattern_timeout_above_max_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "LogPattern",
			"regex": "started",
			"timeout_ms": 600001,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.timeout_ms must be in [1000, 600000]")
}

# ---------------------------------------------------------------------------
# Invariant 12 — LogPattern regex must be non-empty
# ---------------------------------------------------------------------------

# PASS — non-empty regex
test_logpattern_regex_non_empty_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "LogPattern",
			"regex": "Application started",
			"timeout_ms": 30000,
		}},
	}
}

# PASS — regex with special chars is non-empty
test_logpattern_regex_with_special_chars_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "LogPattern",
			"regex": `\bready\b`,
			"timeout_ms": 30000,
		}},
	}
}

# FAIL — empty regex string
test_logpattern_regex_empty_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"health_probe": {
			"kind": "LogPattern",
			"regex": "",
			"timeout_ms": 30000,
		}},
	}
	some msg
	result[msg]
	contains(msg, "health_probe.regex must be non-empty")
}

# ---------------------------------------------------------------------------
# Invariant 13 — log_rotation.kind enum: None | BySize
# ---------------------------------------------------------------------------

# PASS — kind=None is valid
test_log_rotation_kind_none_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {"log_rotation": {"kind": "None"}},
	}
}

# PASS — kind=BySize with required fields and correct capture_kind
test_log_rotation_kind_bysize_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {
			"capture_kind": "tmp_file",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 10485760,
				"keep_files": 5,
			},
		},
	}
}

# FAIL — invalid rotation kind
test_log_rotation_kind_invalid_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"log_rotation": {"kind": "ByTime"}},
	}
	some msg
	result[msg]
	contains(msg, "log_rotation.kind must be None|BySize")
}

# FAIL — lowercase kind rejected
test_log_rotation_kind_lowercase_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {"log_rotation": {"kind": "bysize"}},
	}
	some msg
	result[msg]
	contains(msg, "log_rotation.kind must be None|BySize")
}

# ---------------------------------------------------------------------------
# Invariant 14a — BySize max_bytes_per_file in [1048576, 1073741824]
# ---------------------------------------------------------------------------

# PASS — max_bytes_per_file at lower boundary (1 MiB)
test_bysize_max_bytes_at_min if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {
			"capture_kind": "tmp_file",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 1048576,
				"keep_files": 3,
			},
		},
	}
}

# PASS — max_bytes_per_file at upper boundary (1 GiB)
test_bysize_max_bytes_at_max if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {
			"capture_kind": "tmp_file",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 1073741824,
				"keep_files": 1,
			},
		},
	}
}

# FAIL — max_bytes_per_file below 1 MiB
test_bysize_max_bytes_below_min_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {
			"capture_kind": "tmp_file",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 1048575,
				"keep_files": 5,
			},
		},
	}
	some msg
	result[msg]
	contains(msg, "log_rotation.max_bytes_per_file must be in [1048576, 1073741824]")
}

# FAIL — max_bytes_per_file above 1 GiB
test_bysize_max_bytes_above_max_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {
			"capture_kind": "tmp_file",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 1073741825,
				"keep_files": 5,
			},
		},
	}
	some msg
	result[msg]
	contains(msg, "log_rotation.max_bytes_per_file must be in [1048576, 1073741824]")
}

# ---------------------------------------------------------------------------
# Invariant 14b — BySize keep_files in [1, 20]
# ---------------------------------------------------------------------------

# PASS — keep_files at lower boundary
test_bysize_keep_files_at_min if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {
			"capture_kind": "tmp_file",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 10485760,
				"keep_files": 1,
			},
		},
	}
}

# PASS — keep_files at upper boundary
test_bysize_keep_files_at_max if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {
			"capture_kind": "tmp_file",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 10485760,
				"keep_files": 20,
			},
		},
	}
}

# FAIL — keep_files = 0 is below lower bound
test_bysize_keep_files_zero_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {
			"capture_kind": "tmp_file",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 10485760,
				"keep_files": 0,
			},
		},
	}
	some msg
	result[msg]
	contains(msg, "log_rotation.keep_files must be in [1, 20]")
}

# FAIL — keep_files = 21 above upper bound
test_bysize_keep_files_above_max_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {
			"capture_kind": "tmp_file",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 10485760,
				"keep_files": 21,
			},
		},
	}
	some msg
	result[msg]
	contains(msg, "log_rotation.keep_files must be in [1, 20]")
}

# ---------------------------------------------------------------------------
# Invariant 15 — BySize log_rotation requires capture_kind=tmp_file
# ---------------------------------------------------------------------------

# PASS — capture_kind=tmp_file with BySize
test_bysize_requires_tmp_file_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {
			"capture_kind": "tmp_file",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 10485760,
				"keep_files": 5,
			},
		},
	}
}

# FAIL — capture_kind=stream with BySize
test_bysize_capture_kind_stream_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {
			"capture_kind": "stream",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 10485760,
				"keep_files": 5,
			},
		},
	}
	some msg
	result[msg]
	contains(msg, "log_rotation BySize requires capture_kind=tmp_file")
}

# FAIL — capture_kind=in_memory with BySize
test_bysize_capture_kind_in_memory_denied if {
	result := subprocess_supervisor_invariants.deny with input as {
		"subprocessRequest": {
			"capture_kind": "in_memory",
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 10485760,
				"keep_files": 5,
			},
		},
	}
	some msg
	result[msg]
	contains(msg, "log_rotation BySize requires capture_kind=tmp_file")
}

# ---------------------------------------------------------------------------
# Happy-path integration — all supervisor fields valid, deny empty, allow=true
# ---------------------------------------------------------------------------

# PASS — empty subprocessRequest has no supervisor fields to validate
test_happy_path_empty_request if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {},
	}
}

# PASS — full valid supervisor config with all fields at mid-range values
test_happy_path_full_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": _valid_full,
	}
}

# PASS — allow rule is true when all fields are valid
test_happy_path_allow_rule_true if {
	subprocess_supervisor_invariants.allow with input as {
		"subprocessRequest": _valid_full,
	}
}

# PASS — PortOpen full valid config
test_happy_path_portopen_full_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {
			"name": "redis",
			"capture_kind": "tmp_file",
			"restart_policy": {"kind": "Always", "backoff_ms": 2000},
			"health_probe": {
				"kind": "PortOpen",
				"port": 6379,
				"interval_ms": 3000,
				"startup_grace_ms": 10000,
			},
			"log_rotation": {
				"kind": "BySize",
				"max_bytes_per_file": 52428800,
				"keep_files": 10,
			},
		},
	}
}

# PASS — LogPattern full valid config
test_happy_path_logpattern_full_valid if {
	count(subprocess_supervisor_invariants.deny) == 0 with input as {
		"subprocessRequest": {
			"name": "kafka-broker",
			"restart_policy": {"kind": "Never"},
			"health_probe": {
				"kind": "LogPattern",
				"regex": "\\[KafkaServer\\] started",
				"timeout_ms": 60000,
			},
		},
	}
}
