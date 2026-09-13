// DDD role: ValueObject
//
// CUE schema for the network-info bounded context.
//
// Cross-references:
//   ADR-0058 — network socket introspection bounded context
//
// Dependency on shared kernel: #Pagination (subprocess.cue) for paginated list requests.
// The tool request/result envelopes live in the sibling network_tools.cue.
package schemas

// #Protocol enumerates the transport-layer protocols tracked by net.* tools.
// DDD role: ValueObject
#Protocol: "Tcp" | "Udp"

// #AddrFamily enumerates the IP address families for socket entries.
// DDD role: ValueObject
#AddrFamily: "Inet" | "Inet6"

// #TcpState enumerates the TCP connection lifecycle states per RFC 793.
// "Unknown" is returned when the kernel reports a state code outside the
// standard set (e.g., a kernel version mismatch or platform-specific extension).
// Terminal observation only — substrate does not drive state transitions.
// DDD role: ValueObject
#TcpState:
	"Closed" |
	"Listen" |
	"SynSent" |
	"SynReceived" |
	"Established" |
	"FinWait1" |
	"FinWait2" |
	"CloseWait" |
	"Closing" |
	"LastAck" |
	"TimeWait" |
	"Unknown"

// #SocketAddresses is the local/remote address pair of one socket.
// The remote half is absent when the protocol does not use a remote endpoint
// in the Listen state.
#SocketAddresses: {
	// local_addr is the textual representation of the local address.
	// IPv4: dotted-decimal notation (e.g., "127.0.0.1").
	// IPv6: RFC 5952 compressed notation (e.g., "::1").
	local_addr: #ShortText

	// local_port is the local port number bound to this socket.
	local_port: int & >=0 & <=65535

	// remote_addr is the textual remote address. Absent for Listen-state sockets
	// and for UDP sockets that are not connected.
	remote_addr?: #ShortText

	// remote_port is the remote port number. Absent under the same conditions
	// as remote_addr.
	remote_port?: int & >=0 & <=65535
}

// #SocketEntry is the value object representing a single TCP or UDP socket
// observed on the host at query time. Fields marked optional are absent when
// resolve_pid=false (pid) or when the protocol does not use a remote endpoint
// in the Listen state (remote_addr, remote_port).
// DDD role: ValueObject
#SocketEntry: {
	#SocketAddresses

	// protocol identifies whether this is a TCP or UDP socket.
	protocol: #Protocol

	// family identifies the IP address family of this socket.
	family: #AddrFamily

	// state is the TCP connection state. UDP sockets always report "Established"
	// (connected UDP) or "Listen" (unconnected UDP bound to a port).
	state: #TcpState

	// pid is the OS process ID that owns this socket. Present only when
	// resolve_pid=true was set in the request. Absent otherwise.
	pid?: int & >=1

	// inode is the kernel inode number of the socket file descriptor.
	// Present only on Linux (populated from /proc/net/tcp{,6} inode column).
	// Absent on macOS and when the kernel does not expose inode for this socket.
	inode?: int & >=0
}

// #TcpSegmentCounters groups the TCP traffic counters of #TcpStats.
#TcpSegmentCounters: {
	// segs_in is the total number of TCP segments received by the host kernel.
	segs_in: int & >=0

	// segs_out is the total number of TCP segments transmitted by the host kernel.
	segs_out: int & >=0

	// segs_retransmitted is the count of TCP segments that were retransmitted.
	// Invariant: segs_retransmitted <= segs_out.
	segs_retransmitted: int & >=0

	// rcv_packets is the total number of TCP data packets received.
	rcv_packets: int & >=0

	// snd_packets is the total number of TCP data packets sent.
	snd_packets: int & >=0
}

// #TcpConnectionCounters groups the TCP connection-lifetime and drop counters
// of #TcpStats.
#TcpConnectionCounters: {
	// connections_initiated is the count of active TCP open calls (SYN sent).
	connections_initiated: int & >=0

	// connections_accepted is the count of passive TCP opens (SYN received and
	// accepted from the backlog queue).
	connections_accepted: int & >=0

	// connections_established is the count of TCP connections that reached the
	// ESTABLISHED state.
	connections_established: int & >=0

	// connections_closed is the count of TCP connections that were closed
	// (including resets and normal four-way handshakes).
	connections_closed: int & >=0

	// persist_timer_drops is the count of connections dropped because the
	// persist timer expired with a zero receive window.
	persist_timer_drops: int & >=0

	// keepalive_drops is the count of connections dropped by the keepalive
	// mechanism (no response to keepalive probes within the configured timeout).
	keepalive_drops: int & >=0

	// bad_checksums is the count of TCP segments discarded due to checksum errors.
	bad_checksums: int & >=0
}

// #TcpStats is the aggregate value object returned by net.tcp_stats.
// All counters are monotonically increasing since kernel boot; clients compute
// deltas by calling the tool twice with a known interval.
// The captured_at timestamp anchors the snapshot in time.
// DDD role: ValueObject
#TcpStats: {
	#TcpSegmentCounters

	#TcpConnectionCounters

	// captured_at is the RFC 3339 timestamp at which the snapshot was taken.
	captured_at: #Timestamp
}

// #ConnectionCounts is the histogram value object returned by net.connection_count.
// by_state maps each observed #TcpState to its socket count; states with zero
// sockets are omitted from the map. total equals the sum of all values in by_state.
// DDD role: ValueObject
#ConnectionCounts: {
	// by_state maps each observed TCP state to the number of sockets in that state.
	// Only states with at least one socket are included as keys.
	by_state: {[#TcpState]: int & >=0}

	// total is the sum of all values in by_state. Invariant: total == sum(by_state.values()).
	total: int & >=0

	// captured_at is the RFC 3339 timestamp at which the snapshot was taken.
	captured_at: #Timestamp
}
