package substrate.network_invariants_test

import rego.v1

import data.substrate.network_invariants

# ---------------------------------------------------------------------------
# Invariant 1 — socketEntry.protocol
# ---------------------------------------------------------------------------

# PASS — "Tcp" is a valid protocol
test_protocol_tcp_valid if {
	count(network_invariants.deny) == 0 with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "0.0.0.0",
			"local_port": 8080,
			"state": "Listen",
		},
	}
}

# PASS — "Udp" is a valid protocol
test_protocol_udp_valid if {
	count(network_invariants.deny) == 0 with input as {
		"socketEntry": {
			"protocol": "Udp",
			"family": "Inet",
			"local_addr": "0.0.0.0",
			"local_port": 53,
			"state": "Listen",
		},
	}
}

# FAIL — "SCTP" is not a valid protocol
test_protocol_sctp_invalid if {
	result := network_invariants.deny with input as {
		"socketEntry": {
			"protocol": "SCTP",
			"family": "Inet",
			"local_addr": "0.0.0.0",
			"local_port": 80,
			"state": "Listen",
		},
	}
	some msg
	result[msg]
	contains(msg, "socketEntry.protocol must be one of")
}

# ---------------------------------------------------------------------------
# Invariant 2 — socketEntry.family
# ---------------------------------------------------------------------------

# PASS — "Inet6" is a valid address family
test_family_inet6_valid if {
	count(network_invariants.deny) == 0 with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet6",
			"local_addr": "::1",
			"local_port": 443,
			"state": "Listen",
		},
	}
}

# FAIL — "AF_UNIX" is not a valid family
test_family_unix_invalid if {
	result := network_invariants.deny with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "AF_UNIX",
			"local_addr": "/tmp/sock",
			"local_port": 0,
			"state": "Listen",
		},
	}
	some msg
	result[msg]
	contains(msg, "socketEntry.family must be one of")
}

# ---------------------------------------------------------------------------
# Invariant 3 — socketEntry.state
# ---------------------------------------------------------------------------

# PASS — "Established" is a valid TCP state
test_state_established_valid if {
	count(network_invariants.deny) == 0 with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "10.0.0.1",
			"local_port": 54321,
			"remote_addr": "10.0.0.2",
			"remote_port": 443,
			"state": "Established",
		},
	}
}

# FAIL — "OPEN" is not a valid TCP state
test_state_open_invalid if {
	result := network_invariants.deny with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "127.0.0.1",
			"local_port": 9000,
			"state": "OPEN",
		},
	}
	some msg
	result[msg]
	contains(msg, "socketEntry.state must be one of the 12 TcpState variants")
}

# ---------------------------------------------------------------------------
# Invariant 4 — socketEntry.local_port
# ---------------------------------------------------------------------------

# PASS — port 0 is the lower boundary
test_local_port_zero_valid if {
	count(network_invariants.deny) == 0 with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "0.0.0.0",
			"local_port": 0,
			"state": "Closed",
		},
	}
}

# FAIL — local_port exceeds 65535
test_local_port_too_large if {
	result := network_invariants.deny with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "127.0.0.1",
			"local_port": 70000,
			"state": "Established",
		},
	}
	some msg
	result[msg]
	contains(msg, "socketEntry.local_port must be in [0, 65535]")
}

# FAIL — local_port is negative
test_local_port_negative if {
	result := network_invariants.deny with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "127.0.0.1",
			"local_port": -1,
			"state": "Listen",
		},
	}
	some msg
	result[msg]
	contains(msg, "socketEntry.local_port must be in [0, 65535]")
}

# ---------------------------------------------------------------------------
# Invariant 5 — socketEntry.remote_port
# ---------------------------------------------------------------------------

# FAIL — remote_port exceeds 65535 when present
test_remote_port_too_large if {
	result := network_invariants.deny with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "10.0.0.1",
			"local_port": 55000,
			"remote_addr": "10.0.0.2",
			"remote_port": 99999,
			"state": "Established",
		},
	}
	some msg
	result[msg]
	contains(msg, "socketEntry.remote_port must be in [0, 65535]")
}

# ---------------------------------------------------------------------------
# Invariant 6 — socketEntry.pid
# ---------------------------------------------------------------------------

# PASS — pid=1 is the minimum valid value
test_pid_one_valid if {
	count(network_invariants.deny) == 0 with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "127.0.0.1",
			"local_port": 8080,
			"state": "Listen",
			"pid": 1,
		},
	}
}

# FAIL — pid=0 is below minimum
test_pid_zero_invalid if {
	result := network_invariants.deny with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "127.0.0.1",
			"local_port": 8080,
			"state": "Listen",
			"pid": 0,
		},
	}
	some msg
	result[msg]
	contains(msg, "socketEntry.pid must be >= 1")
}

# ---------------------------------------------------------------------------
# Invariant 7 — tcpStats counters non-negative
# ---------------------------------------------------------------------------

# PASS — all counters at zero is a valid (zeroed) snapshot
test_tcp_stats_all_zero_valid if {
	count(network_invariants.deny) == 0 with input as {
		"tcpStats": {
			"segs_in": 0,
			"segs_out": 0,
			"segs_retransmitted": 0,
			"rcv_packets": 0,
			"snd_packets": 0,
			"connections_initiated": 0,
			"connections_accepted": 0,
			"connections_established": 0,
			"connections_closed": 0,
			"persist_timer_drops": 0,
			"keepalive_drops": 0,
			"bad_checksums": 0,
			"captured_at": "2026-05-24T00:00:00Z",
		},
	}
}

# FAIL — segs_in is negative
test_tcp_stats_segs_in_negative if {
	result := network_invariants.deny with input as {
		"tcpStats": {
			"segs_in": -1,
			"segs_out": 0,
			"segs_retransmitted": 0,
			"rcv_packets": 0,
			"snd_packets": 0,
			"connections_initiated": 0,
			"connections_accepted": 0,
			"connections_established": 0,
			"connections_closed": 0,
			"persist_timer_drops": 0,
			"keepalive_drops": 0,
			"bad_checksums": 0,
			"captured_at": "2026-05-24T00:00:00Z",
		},
	}
	some msg
	result[msg]
	contains(msg, "tcpStats.segs_in must be >= 0")
}

# FAIL — bad_checksums is negative
test_tcp_stats_bad_checksums_negative if {
	result := network_invariants.deny with input as {
		"tcpStats": {
			"segs_in": 100,
			"segs_out": 90,
			"segs_retransmitted": 2,
			"rcv_packets": 100,
			"snd_packets": 90,
			"connections_initiated": 5,
			"connections_accepted": 3,
			"connections_established": 8,
			"connections_closed": 4,
			"persist_timer_drops": 0,
			"keepalive_drops": 0,
			"bad_checksums": -5,
			"captured_at": "2026-05-24T00:00:00Z",
		},
	}
	some msg
	result[msg]
	contains(msg, "tcpStats.bad_checksums must be >= 0")
}

# ---------------------------------------------------------------------------
# Invariant 8 — connectionCounts.total must equal sum(by_state)
# ---------------------------------------------------------------------------

# PASS — total matches by_state sum
test_connection_counts_total_matches_sum if {
	count(network_invariants.deny) == 0 with input as {
		"connectionCounts": {
			"by_state": {"Listen": 3, "Established": 5, "TimeWait": 2},
			"total": 10,
			"captured_at": "2026-05-24T00:00:00Z",
		},
	}
}

# FAIL — total does not match by_state sum
test_connection_counts_total_mismatch if {
	result := network_invariants.deny with input as {
		"connectionCounts": {
			"by_state": {"Listen": 3, "Established": 5},
			"total": 10,
			"captured_at": "2026-05-24T00:00:00Z",
		},
	}
	some msg
	result[msg]
	contains(msg, "connectionCounts.total must equal the sum of by_state values")
}

# ---------------------------------------------------------------------------
# Invariant 9 — networkTcpListRequest.state_filter elements
# ---------------------------------------------------------------------------

# PASS — all state_filter values are valid TcpState variants
test_tcp_list_request_state_filter_valid if {
	count(network_invariants.deny) == 0 with input as {
		"networkTcpListRequest": {"state_filter": ["Listen", "Established", "TimeWait"]},
	}
}

# FAIL — state_filter contains an unknown state
test_tcp_list_request_state_filter_invalid if {
	result := network_invariants.deny with input as {
		"networkTcpListRequest": {"state_filter": ["Listen", "BOGUS"]},
	}
	some msg
	result[msg]
	contains(msg, "networkTcpListRequest.state_filter element must be a valid TcpState variant")
}

# PASS — empty state_filter is valid (no filter applied)
test_tcp_list_request_state_filter_empty if {
	count(network_invariants.deny) == 0 with input as {
		"networkTcpListRequest": {"state_filter": []},
	}
}

# ---------------------------------------------------------------------------
# Invariant 10 — Listen entries must have local_port > 0
# ---------------------------------------------------------------------------

# PASS — Listen entry with local_port=22 is valid
test_listen_entry_nonzero_port_valid if {
	count(network_invariants.deny) == 0 with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "0.0.0.0",
			"local_port": 22,
			"state": "Listen",
		},
	}
}

# FAIL — Listen entry with local_port=0 indicates adapter layout bug
test_listen_entry_zero_port_invalid if {
	result := network_invariants.deny with input as {
		"socketEntry": {
			"protocol": "Tcp",
			"family": "Inet",
			"local_addr": "0.0.0.0",
			"local_port": 0,
			"state": "Listen",
		},
	}
	some msg
	result[msg]
	contains(msg, "socketEntry.local_port must be > 0 for Listen entries")
}
