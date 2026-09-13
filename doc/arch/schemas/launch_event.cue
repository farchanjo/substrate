// DDD role: ValueObject
package schemas

// Event stream of the launch bounded context: the event classification, one
// durable event-log entry, and the messaging-fabric bounds. Per ADR-0066 and
// ADR-0067.

// #LaunchEventKind enumerates the typed lifecycle plane plus the SEMANTIC marker for the
// heuristic plane. Lifecycle events are authoritative; SEMANTIC events are advisory.
// Per ADR-0066.
#LaunchEventKind: "STARTED" | "READY" | "EXITED" | "CRASHED" | "RESTARTING" | "ORPHAN_REAPED" | "ORPHAN_ADOPTED" | "STACK_TTL_EXPIRED" | "SEMANTIC"

// #LaunchEventOrigin is the correlation half of a #LaunchEvent: which Stack, which
// Service (absent for Stack-level events), and where in the log the entry sits.
// Embedded by #LaunchEvent.
#LaunchEventOrigin: {
	// stack_id correlates the event with its Stack.
	stack_id: #StackId

	// service is the originating Service; absent for Stack-level events.
	service?: #ServiceName

	// seq is the zero-based monotonic sequence number within the Stack event-log.
	seq: int & >=0

	// cursor is the opaque pagination cursor addressing this position in the log.
	cursor: string & !=""
}

// #LaunchEvent is one entry in the durable per-Stack event-log (events.ndjson) and the
// unit delivered over the events resource and replay. The cursor is the opaque ?since
// value (ADR-0008) a client passes to read the delta. Per ADR-0066.
#LaunchEvent: {
	// Which Stack and Service the event belongs to, and its log position.
	#LaunchEventOrigin

	// kind is the event classification.
	kind: #LaunchEventKind

	// stream is present only for SEMANTIC events distilled from a child output channel.
	stream?: #Stream

	// message is the redacted, human-oriented event text (already passed the denylist).
	message: #ShortText

	// exit_code is present only for EXITED / CRASHED events.
	exit_code?: #ExitCode

	// timestamp is the RFC 3339 time the event was recorded.
	timestamp: #Timestamp
}

// #LaunchChannelBounds carries the configurable bounds for the lock-free messaging
// fabric, all with defaults. Per ADR-0067 (channel capacities) and ADR-0066 (rate caps)
// and ADR-0065 (orchestrated-restart rate limit).
#LaunchChannelBounds: {
	// stdout_mpsc_capacity bounds the per-Service stdout/stderr reader channel; overflow
	// is dropped with a count, never awaited (the pipe is never blocked).
	stdout_mpsc_capacity: int & >=1 | *1024

	// event_broadcast_capacity bounds the per-Stack broadcast bus; a lagging consumer
	// receives Lagged(n) drop-with-count backpressure.
	event_broadcast_capacity: int & >=1 | *256

	// notify_rate_per_sec caps semantic-event emission per Service per second.
	notify_rate_per_sec: int & >=1 | *5

	// orchestrated_restart_per_min caps reconciler/cascade restarts per Service per minute.
	orchestrated_restart_per_min: int & >=1 | *60
}