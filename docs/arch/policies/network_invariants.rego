# package substrate.network_invariants
#
# Validates network-info bounded context input shapes before any kernel API
# is invoked.  These invariants are evaluated at policy-evaluation time
# (CI conftest gate) and at runtime inside the substrate-network adapter.
#
# Cross-references:
#   ADR-0058 — network socket introspection bounded context decision
#   ADR-0004 — security model (Layer 1–5 enforcement sequence)
#
# Input shapes accepted:
#
#   socketEntry — a single #SocketEntry value to validate
#   {
#     "socketEntry": {
#       "protocol":    "Tcp" | "Udp",
#       "family":      "Inet" | "Inet6",
#       "local_addr":  "<string>",
#       "local_port":  <int>,
#       "remote_addr": "<string>",       // optional
#       "remote_port": <int>,             // optional
#       "state":       "<TcpState>",
#       "pid":         <int>,             // optional, >= 1
#       "inode":       <int>              // optional, >= 0
#     }
#   }
#
#   tcpStats — a #TcpStats snapshot to validate
#   {
#     "tcpStats": {
#       "segs_in":                 <int>,
#       "segs_out":                <int>,
#       "segs_retransmitted":      <int>,
#       "rcv_packets":             <int>,
#       "snd_packets":             <int>,
#       "connections_initiated":   <int>,
#       "connections_accepted":    <int>,
#       "connections_established": <int>,
#       "connections_closed":      <int>,
#       "persist_timer_drops":     <int>,
#       "keepalive_drops":         <int>,
#       "bad_checksums":           <int>,
#       "captured_at":             "<string>"
#     }
#   }
#
#   connectionCounts — a #ConnectionCounts histogram to validate
#   {
#     "connectionCounts": {
#       "by_state": { "<TcpState>": <int>, ... },
#       "total":    <int>,
#       "captured_at": "<string>"
#     }
#   }
#
#   networkTcpListRequest — a #NetworkTcpListRequest to validate
#   {
#     "networkTcpListRequest": {
#       "state_filter": ["<TcpState>", ...],  // optional
#       "resolve_pid":  <bool>                // optional
#     }
#   }
#
# Test vectors (inline):
#
#   PASS — minimal valid socket entry (TCP/Inet/Listen)
#   input = {"socketEntry": {"protocol": "Tcp", "family": "Inet",
#     "local_addr": "0.0.0.0", "local_port": 8080, "state": "Listen"}}
#
#   FAIL — invalid protocol
#   input = {"socketEntry": {"protocol": "SCTP", "family": "Inet",
#     "local_addr": "0.0.0.0", "local_port": 80, "state": "Listen"}}
#   expected deny contains: "socketEntry.protocol must be one of"
#
#   FAIL — local_port out of range
#   input = {"socketEntry": {"protocol": "Tcp", "family": "Inet",
#     "local_addr": "127.0.0.1", "local_port": 70000, "state": "Established"}}
#   expected deny contains: "socketEntry.local_port must be in [0, 65535]"
#
#   FAIL — negative counter in tcpStats
#   input = {"tcpStats": {"segs_in": -1, "segs_out": 0, "segs_retransmitted": 0,
#     "rcv_packets": 0, "snd_packets": 0, "connections_initiated": 0,
#     "connections_accepted": 0, "connections_established": 0,
#     "connections_closed": 0, "persist_timer_drops": 0, "keepalive_drops": 0,
#     "bad_checksums": 0, "captured_at": "2026-05-24T00:00:00Z"}}
#   expected deny contains: "tcpStats.segs_in must be >= 0"
#
#   FAIL — connectionCounts.total mismatch
#   input = {"connectionCounts": {"by_state": {"Listen": 3, "Established": 5},
#     "total": 10, "captured_at": "2026-05-24T00:00:00Z"}}
#   expected deny contains: "connectionCounts.total must equal the sum of by_state values"

package substrate.network_invariants

import rego.v1

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

_valid_protocols := {"Tcp", "Udp"}

_valid_families := {"Inet", "Inet6"}

_valid_tcp_states := {
	"Closed",
	"Listen",
	"SynSent",
	"SynReceived",
	"Established",
	"FinWait1",
	"FinWait2",
	"CloseWait",
	"Closing",
	"LastAck",
	"TimeWait",
	"Unknown",
}

_tcp_stats_counters := {
	"segs_in",
	"segs_out",
	"segs_retransmitted",
	"rcv_packets",
	"snd_packets",
	"connections_initiated",
	"connections_accepted",
	"connections_established",
	"connections_closed",
	"persist_timer_drops",
	"keepalive_drops",
	"bad_checksums",
}

# ---------------------------------------------------------------------------
# Invariant 1 — socketEntry.protocol must be "Tcp" or "Udp"
# ---------------------------------------------------------------------------

deny contains msg if {
	proto := input.socketEntry.protocol
	not _valid_protocols[proto]
	msg := sprintf(
		"socketEntry.protocol must be one of %v; got '%s'",
		[_valid_protocols, proto],
	)
}

# ---------------------------------------------------------------------------
# Invariant 2 — socketEntry.family must be "Inet" or "Inet6"
# ---------------------------------------------------------------------------

deny contains msg if {
	fam := input.socketEntry.family
	not _valid_families[fam]
	msg := sprintf(
		"socketEntry.family must be one of %v; got '%s'",
		[_valid_families, fam],
	)
}

# ---------------------------------------------------------------------------
# Invariant 3 — socketEntry.state must be a valid TcpState
# ---------------------------------------------------------------------------

deny contains msg if {
	st := input.socketEntry.state
	not _valid_tcp_states[st]
	msg := sprintf(
		"socketEntry.state must be one of the 12 TcpState variants; got '%s'",
		[st],
	)
}

# ---------------------------------------------------------------------------
# Invariant 4 — socketEntry.local_port must be in [0, 65535]
# ---------------------------------------------------------------------------

deny contains msg if {
	port := input.socketEntry.local_port
	port < 0
	msg := sprintf(
		"socketEntry.local_port must be in [0, 65535]; got %d",
		[port],
	)
}

deny contains msg if {
	port := input.socketEntry.local_port
	port > 65535
	msg := sprintf(
		"socketEntry.local_port must be in [0, 65535]; got %d",
		[port],
	)
}

# ---------------------------------------------------------------------------
# Invariant 5 — socketEntry.remote_port must be in [0, 65535] when present
# ---------------------------------------------------------------------------

deny contains msg if {
	port := input.socketEntry.remote_port
	port < 0
	msg := sprintf(
		"socketEntry.remote_port must be in [0, 65535] when present; got %d",
		[port],
	)
}

deny contains msg if {
	port := input.socketEntry.remote_port
	port > 65535
	msg := sprintf(
		"socketEntry.remote_port must be in [0, 65535] when present; got %d",
		[port],
	)
}

# ---------------------------------------------------------------------------
# Invariant 6 — socketEntry.pid must be >= 1 when present
# ---------------------------------------------------------------------------

deny contains msg if {
	pid := input.socketEntry.pid
	pid < 1
	msg := sprintf(
		"socketEntry.pid must be >= 1 when present; got %d",
		[pid],
	)
}

# ---------------------------------------------------------------------------
# Invariant 7 — tcpStats counters must all be >= 0
# ---------------------------------------------------------------------------

deny contains msg if {
	some counter in _tcp_stats_counters
	val := input.tcpStats[counter]
	val < 0
	msg := sprintf(
		"tcpStats.%s must be >= 0; got %d",
		[counter, val],
	)
}

# ---------------------------------------------------------------------------
# Invariant 8 — connectionCounts.total must equal sum(by_state values)
# ---------------------------------------------------------------------------

deny contains msg if {
	counts := input.connectionCounts
	computed := sum([v | v := counts.by_state[_]])
	counts.total != computed
	msg := sprintf(
		"connectionCounts.total must equal the sum of by_state values; total=%d but sum(by_state)=%d",
		[counts.total, computed],
	)
}

# ---------------------------------------------------------------------------
# Invariant 9 — networkTcpListRequest.state_filter elements must be valid TcpState
# ---------------------------------------------------------------------------

deny contains msg if {
	some st in input.networkTcpListRequest.state_filter
	not _valid_tcp_states[st]
	msg := sprintf(
		"networkTcpListRequest.state_filter element must be a valid TcpState variant; got '%s'",
		[st],
	)
}
