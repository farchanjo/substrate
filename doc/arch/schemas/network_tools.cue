// DDD role: ValueObject
//
// CUE schema for the net.* tool request and result envelopes.
//
// Split out of network.cue (one-definition-per-file) because a bounded context
// with one envelope per tool exceeds the per-file definition budget. Every
// definition here belongs to the network-info bounded context per ADR-0058.
//
// Cross-references:
//   ADR-0058 — network socket introspection bounded context
//
// Dependency on shared kernel: #Pagination (subprocess.cue) for paginated list requests.
package schemas

// #TcpStateFilter limits a net.tcp_list response to the listed TCP states.
// An empty list is equivalent to absent (no filter).
#TcpStateFilter: [...#TcpState]

// #SocketEntryPage is one page of #SocketEntry values.
#SocketEntryPage: [...#SocketEntry]

// #NetworkTcpListRequest is the value object submitted by an MCP client to
// invoke net.tcp_list. All fields are optional; absent means no filter applied.
// DDD role: ValueObject
#NetworkTcpListRequest: {
	// state_filter, when present, limits the response to sockets in the listed
	// TCP states. An empty list is equivalent to absent (no filter).
	state_filter?: #TcpStateFilter

	// resolve_pid, when true, instructs the adapter to resolve the owning PID
	// for each socket via platform-specific APIs (proc_pidfdinfo on macOS,
	// /proc/<pid>/fd/* scan on Linux). Incurs additional latency. Default false.
	resolve_pid?: bool | *false

	// pagination, when present, enables cursor-based paged retrieval of results.
	// Reuses the #Pagination value object from ADR-0057.
	pagination?: #Pagination
}

// #NetworkTcpListResult is the value object returned by net.tcp_list.
// DDD role: ValueObject
#NetworkTcpListResult: {
	// entries is the current page of #SocketEntry values matching the request filter.
	entries: #SocketEntryPage

	// total is the count of all matching sockets before pagination was applied.
	total: int & >=0

	// next_offset, when present, is the pagination offset for the next page.
	// Absent when this is the last (or only) page of results.
	next_offset?: int & >=0
}

// #NetworkUdpListRequest is the value object submitted by an MCP client to
// invoke net.udp_list.
// DDD role: ValueObject
#NetworkUdpListRequest: {
	// resolve_pid, when true, instructs the adapter to resolve the owning PID
	// for each socket. Default false.
	resolve_pid?: bool | *false

	// pagination, when present, enables cursor-based paged retrieval of results.
	pagination?: #Pagination
}

// #NetworkUdpListResult is the value object returned by net.udp_list.
// DDD role: ValueObject
#NetworkUdpListResult: {
	// entries is the current page of #SocketEntry values.
	entries: #SocketEntryPage

	// total is the count of all UDP sockets before pagination was applied.
	total: int & >=0

	// next_offset, when present, is the pagination offset for the next page.
	next_offset?: int & >=0
}

// #NetworkTcpStatsRequest is the (empty) value object submitted by an MCP
// client to invoke net.tcp_stats. No parameters are required; the tool always
// returns a full snapshot of the kernel TCP MIB counters.
// DDD role: ValueObject
#NetworkTcpStatsRequest: {}

// #NetworkConnectionCountRequest is the (empty) value object submitted by an
// MCP client to invoke net.connection_count. No parameters are required; the
// tool always returns a full histogram across all current TCP sockets.
// DDD role: ValueObject
#NetworkConnectionCountRequest: {}
