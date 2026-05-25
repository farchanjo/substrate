# package substrate.subprocess_supervisor_invariants
#
# Validates supervisor-extension fields on a #SubprocessRequest before any
# child process is spawned or its configuration is persisted.  These invariants
# gate the operator-config path (CI conftest) and the runtime adapter path
# inside substrate-subprocess.
#
# Cross-references:
#   ADR-0056 — subprocess supervisor semantics (name, restart_policy,
#               health_probe, log_rotation)
#   ADR-0052 — subprocess bounded context decision
#   ADR-0004 — security model (Layer 1–5 enforcement sequence)
#
# Input shape:
#   {
#     "subprocessRequest": {
#       "name":             "<string>",                     // optional
#       "capture_kind":     "stream" | "in_memory" | "tmp_file",  // optional
#       "restart_policy": {                                 // optional block
#         "kind":           "Never" | "OnFailure" | "Always",
#         "max_retries":    <int>,                          // OnFailure only
#         "backoff_ms":     <int>                           // OnFailure | Always
#       },
#       "health_probe": {                                   // optional block
#         "kind":           "None" | "HttpGet" | "PortOpen" | "LogPattern",
#         "url":            "<string>",                     // HttpGet only
#         "expected_status":<int>,                          // HttpGet only
#         "interval_ms":   <int>,                           // HttpGet | PortOpen
#         "startup_grace_ms":<int>,                         // HttpGet | PortOpen
#         "port":           <int>,                          // PortOpen only
#         "regex":          "<string>",                     // LogPattern only
#         "timeout_ms":     <int>                           // LogPattern only
#       },
#       "log_rotation": {                                   // optional block
#         "kind":              "None" | "BySize",
#         "max_bytes_per_file":<int>,                       // BySize only
#         "keep_files":        <int>                        // BySize only
#       }
#     }
#   }
#
# Test vectors (inline):
#
#   PASS — minimal request, no supervisor fields present
#   input = {"subprocessRequest": {}}
#
#   PASS — all supervisor fields valid
#   input = {"subprocessRequest": {
#     "name": "spring-backend",
#     "capture_kind": "tmp_file",
#     "restart_policy": {"kind": "OnFailure", "max_retries": 3, "backoff_ms": 1000},
#     "health_probe": {"kind": "HttpGet", "url": "http://localhost:8080/health",
#       "expected_status": 200, "interval_ms": 5000, "startup_grace_ms": 30000},
#     "log_rotation": {"kind": "BySize", "max_bytes_per_file": 10485760, "keep_files": 5}
#   }}
#
#   FAIL — name contains uppercase
#   input = {"subprocessRequest": {"name": "Spring-Backend"}}
#   expected deny contains: "name must match ^[a-z0-9-]{1,64}$"
#
#   FAIL — restart_policy.kind invalid
#   input = {"subprocessRequest": {"restart_policy": {"kind": "Restart"}}}
#   expected deny contains: "restart_policy.kind must be Never|OnFailure|Always"
#
#   FAIL — OnFailure max_retries out of range
#   input = {"subprocessRequest": {"restart_policy": {"kind": "OnFailure",
#     "max_retries": 0, "backoff_ms": 1000}}}
#   expected deny contains: "max_retries must be in [1, 100]"

package substrate.subprocess_supervisor_invariants

import rego.v1

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

_valid_restart_kinds := {"Never", "OnFailure", "Always"}

_valid_probe_kinds := {"None", "HttpGet", "PortOpen", "LogPattern"}

_valid_log_rotation_kinds := {"None", "BySize"}

# ---------------------------------------------------------------------------
# Invariant 1: name, when present, MUST match ^[a-z0-9-]{1,64}$
# ---------------------------------------------------------------------------

deny contains msg if {
	name := input.subprocessRequest.name
	not regex.match(`^[a-z0-9-]{1,64}$`, name)
	msg := sprintf(
		"subprocess.spawn: name must match ^[a-z0-9-]{1,64}$; got value %v",
		[name],
	)
}

# ---------------------------------------------------------------------------
# Invariant 2: restart_policy.kind MUST be Never | OnFailure | Always
# ---------------------------------------------------------------------------

deny contains msg if {
	kind := input.subprocessRequest.restart_policy.kind
	not _valid_restart_kinds[kind]
	msg := sprintf(
		"subprocess.spawn: restart_policy.kind must be Never|OnFailure|Always; got value %v",
		[kind],
	)
}

# ---------------------------------------------------------------------------
# Invariant 3a: OnFailure — max_retries MUST be in [1, 100]
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.restart_policy.kind == "OnFailure"
	retries := input.subprocessRequest.restart_policy.max_retries
	not (retries >= 1)
	msg := sprintf(
		"subprocess.spawn: max_retries must be in [1, 100] when kind=OnFailure; got value %v",
		[retries],
	)
}

deny contains msg if {
	input.subprocessRequest.restart_policy.kind == "OnFailure"
	retries := input.subprocessRequest.restart_policy.max_retries
	retries > 100
	msg := sprintf(
		"subprocess.spawn: max_retries must be in [1, 100] when kind=OnFailure; got value %v",
		[retries],
	)
}

# ---------------------------------------------------------------------------
# Invariant 3b: OnFailure — backoff_ms MUST be in [100, 300000]
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.restart_policy.kind == "OnFailure"
	backoff := input.subprocessRequest.restart_policy.backoff_ms
	not (backoff >= 100)
	msg := sprintf(
		"subprocess.spawn: backoff_ms must be in [100, 300000] when kind=OnFailure; got value %v",
		[backoff],
	)
}

deny contains msg if {
	input.subprocessRequest.restart_policy.kind == "OnFailure"
	backoff := input.subprocessRequest.restart_policy.backoff_ms
	backoff > 300000
	msg := sprintf(
		"subprocess.spawn: backoff_ms must be in [100, 300000] when kind=OnFailure; got value %v",
		[backoff],
	)
}

# ---------------------------------------------------------------------------
# Invariant 4: Always — backoff_ms MUST be in [100, 300000]
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.restart_policy.kind == "Always"
	backoff := input.subprocessRequest.restart_policy.backoff_ms
	not (backoff >= 100)
	msg := sprintf(
		"subprocess.spawn: backoff_ms must be in [100, 300000] when kind=Always; got value %v",
		[backoff],
	)
}

deny contains msg if {
	input.subprocessRequest.restart_policy.kind == "Always"
	backoff := input.subprocessRequest.restart_policy.backoff_ms
	backoff > 300000
	msg := sprintf(
		"subprocess.spawn: backoff_ms must be in [100, 300000] when kind=Always; got value %v",
		[backoff],
	)
}

# ---------------------------------------------------------------------------
# Invariant 5: health_probe.kind MUST be None | HttpGet | PortOpen | LogPattern
# ---------------------------------------------------------------------------

deny contains msg if {
	kind := input.subprocessRequest.health_probe.kind
	not _valid_probe_kinds[kind]
	msg := sprintf(
		"subprocess.spawn: health_probe.kind must be None|HttpGet|PortOpen|LogPattern; got value %v",
		[kind],
	)
}

# ---------------------------------------------------------------------------
# Invariant 6: HttpGet — url MUST start with http:// or https://
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.health_probe.kind == "HttpGet"
	url := input.subprocessRequest.health_probe.url
	not regex.match(`^https?://`, url)
	msg := sprintf(
		"subprocess.spawn: health_probe.url must start with http:// or https://; got value %v",
		[url],
	)
}

# ---------------------------------------------------------------------------
# Invariant 7: HttpGet — expected_status MUST be in [100, 599]
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.health_probe.kind == "HttpGet"
	status := input.subprocessRequest.health_probe.expected_status
	not (status >= 100)
	msg := sprintf(
		"subprocess.spawn: health_probe.expected_status must be in [100, 599]; got value %v",
		[status],
	)
}

deny contains msg if {
	input.subprocessRequest.health_probe.kind == "HttpGet"
	status := input.subprocessRequest.health_probe.expected_status
	status > 599
	msg := sprintf(
		"subprocess.spawn: health_probe.expected_status must be in [100, 599]; got value %v",
		[status],
	)
}

# ---------------------------------------------------------------------------
# Invariant 8: HttpGet and PortOpen — interval_ms MUST be in [100, 60000]
# ---------------------------------------------------------------------------

deny contains msg if {
	kind := input.subprocessRequest.health_probe.kind
	kind in {"HttpGet", "PortOpen"}
	interval := input.subprocessRequest.health_probe.interval_ms
	not (interval >= 100)
	msg := sprintf(
		"subprocess.spawn: health_probe.interval_ms must be in [100, 60000]; got value %v",
		[interval],
	)
}

deny contains msg if {
	kind := input.subprocessRequest.health_probe.kind
	kind in {"HttpGet", "PortOpen"}
	interval := input.subprocessRequest.health_probe.interval_ms
	interval > 60000
	msg := sprintf(
		"subprocess.spawn: health_probe.interval_ms must be in [100, 60000]; got value %v",
		[interval],
	)
}

# ---------------------------------------------------------------------------
# Invariant 9: HttpGet and PortOpen — startup_grace_ms MUST be in [0, 600000]
# ---------------------------------------------------------------------------

deny contains msg if {
	kind := input.subprocessRequest.health_probe.kind
	kind in {"HttpGet", "PortOpen"}
	grace := input.subprocessRequest.health_probe.startup_grace_ms
	grace < 0
	msg := sprintf(
		"subprocess.spawn: health_probe.startup_grace_ms must be in [0, 600000]; got value %v",
		[grace],
	)
}

deny contains msg if {
	kind := input.subprocessRequest.health_probe.kind
	kind in {"HttpGet", "PortOpen"}
	grace := input.subprocessRequest.health_probe.startup_grace_ms
	grace > 600000
	msg := sprintf(
		"subprocess.spawn: health_probe.startup_grace_ms must be in [0, 600000]; got value %v",
		[grace],
	)
}

# ---------------------------------------------------------------------------
# Invariant 10: PortOpen — port MUST be in [1, 65535]
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.health_probe.kind == "PortOpen"
	port := input.subprocessRequest.health_probe.port
	not (port >= 1)
	msg := sprintf(
		"subprocess.spawn: health_probe.port must be in [1, 65535]; got value %v",
		[port],
	)
}

deny contains msg if {
	input.subprocessRequest.health_probe.kind == "PortOpen"
	port := input.subprocessRequest.health_probe.port
	port > 65535
	msg := sprintf(
		"subprocess.spawn: health_probe.port must be in [1, 65535]; got value %v",
		[port],
	)
}

# ---------------------------------------------------------------------------
# Invariant 11: LogPattern — timeout_ms MUST be in [1000, 600000]
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.health_probe.kind == "LogPattern"
	timeout := input.subprocessRequest.health_probe.timeout_ms
	not (timeout >= 1000)
	msg := sprintf(
		"subprocess.spawn: health_probe.timeout_ms must be in [1000, 600000] for LogPattern; got value %v",
		[timeout],
	)
}

deny contains msg if {
	input.subprocessRequest.health_probe.kind == "LogPattern"
	timeout := input.subprocessRequest.health_probe.timeout_ms
	timeout > 600000
	msg := sprintf(
		"subprocess.spawn: health_probe.timeout_ms must be in [1000, 600000] for LogPattern; got value %v",
		[timeout],
	)
}

# ---------------------------------------------------------------------------
# Invariant 12: LogPattern — regex MUST be non-empty
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.health_probe.kind == "LogPattern"
	regex_val := input.subprocessRequest.health_probe.regex
	count(regex_val) == 0
	msg := sprintf(
		"subprocess.spawn: health_probe.regex must be non-empty for LogPattern; got value %v",
		[regex_val],
	)
}

# ---------------------------------------------------------------------------
# Invariant 13: log_rotation.kind MUST be None | BySize
# ---------------------------------------------------------------------------

deny contains msg if {
	kind := input.subprocessRequest.log_rotation.kind
	not _valid_log_rotation_kinds[kind]
	msg := sprintf(
		"subprocess.spawn: log_rotation.kind must be None|BySize; got value %v",
		[kind],
	)
}

# ---------------------------------------------------------------------------
# Invariant 14a: BySize — max_bytes_per_file MUST be in [1048576, 1073741824]
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.log_rotation.kind == "BySize"
	bytes := input.subprocessRequest.log_rotation.max_bytes_per_file
	not (bytes >= 1048576)
	msg := sprintf(
		"subprocess.spawn: log_rotation.max_bytes_per_file must be in [1048576, 1073741824] (1 MiB..1 GiB); got value %v",
		[bytes],
	)
}

deny contains msg if {
	input.subprocessRequest.log_rotation.kind == "BySize"
	bytes := input.subprocessRequest.log_rotation.max_bytes_per_file
	bytes > 1073741824
	msg := sprintf(
		"subprocess.spawn: log_rotation.max_bytes_per_file must be in [1048576, 1073741824] (1 MiB..1 GiB); got value %v",
		[bytes],
	)
}

# ---------------------------------------------------------------------------
# Invariant 14b: BySize — keep_files MUST be in [1, 20]
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.log_rotation.kind == "BySize"
	keep := input.subprocessRequest.log_rotation.keep_files
	not (keep >= 1)
	msg := sprintf(
		"subprocess.spawn: log_rotation.keep_files must be in [1, 20]; got value %v",
		[keep],
	)
}

deny contains msg if {
	input.subprocessRequest.log_rotation.kind == "BySize"
	keep := input.subprocessRequest.log_rotation.keep_files
	keep > 20
	msg := sprintf(
		"subprocess.spawn: log_rotation.keep_files must be in [1, 20]; got value %v",
		[keep],
	)
}

# ---------------------------------------------------------------------------
# Invariant 15: BySize log_rotation requires capture_kind=tmp_file
# ---------------------------------------------------------------------------

deny contains msg if {
	input.subprocessRequest.log_rotation.kind == "BySize"
	capture := input.subprocessRequest.capture_kind
	capture != "tmp_file"
	msg := sprintf(
		"subprocess.spawn: log_rotation BySize requires capture_kind=tmp_file; got value %v",
		[capture],
	)
}

# ---------------------------------------------------------------------------
# allow — true only when all deny rules produce no messages
# ---------------------------------------------------------------------------

default allow := false

allow if count(deny) == 0
