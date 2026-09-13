// DDD role: AggregateRoot
//
// CUE schema for the async job control-plane: the identifier vocabulary, the job
// state machine, and the dispatch classification.
//
// Cross-references:
//   ADR-0040 — async job control-plane (JobRegistry, buckets, idempotency)
//   ADR-0062 — tool naming convention (wire form)
//   ADR-0069 — launch.status as a polling endpoint
//
// Sibling files, same `schemas` package:
//   job_entry.cue  — the JobRegistry aggregate snapshot
//   job_event.cue  — the notifications/progress push payload
//   job_config.cue — quotas, inline thresholds, and per-tool timeouts
package schemas

// #JobId is a UUIDv7 encoded in base32 Crockford form (26 uppercase chars).
// It doubles as the MCP progressToken and the correlation_id per ADR-0040.
// DDD role: ValueObject
#JobId: string & =~"^[0-9A-HJKMNP-TV-Z]{26}$"

// #CorrelationId is an alias of #JobId.
// The triple equality (job_id == progressToken == correlation_id) eliminates
// any mapping table between MCP protocol tokens and internal identifiers per ADR-0040.
// DDD role: ValueObject
#CorrelationId: #JobId

// DDD role: ValueObject
// #JobProgressToken is the MCP progressToken value for a job submission.
// It equals the #JobId per ADR-0040 triple-equality invariant.
// Named #JobProgressToken to avoid collision with #ProgressToken in shared_kernel.cue,
// which models the incremental-progress tracking token used by streaming tools.
#JobProgressToken: #JobId

// DDD role: ValueObject
// #IdempotencyKey is a client-generated UUIDv7 (base32 Crockford, 26 chars).
// Deduplication key: (client_id, tool_name, idempotency_key, blake3_hash_of_args_json)
// per ADR-0040. Bounded to result_ttl_secs and evicted by the same GC.
#IdempotencyKey: string & =~"^[0-9A-HJKMNP-TV-Z]{26}$"

// #ClientId identifies the MCP client submitting a job.
// Cross-client visibility is forbidden; each client sees only its own jobs per ADR-0040.
// DDD role: ValueObject
#ClientId: string & =~"^[A-Za-z0-9._-]{1,64}$"

// DDD role: ValueObject
// #JobState enumerates all valid states of the async job state machine per ADR-0040.
// Terminal states (succeeded, failed, cancelled, timed_out) never regress.
#JobState: "pending" | "running" | "succeeded" | "failed" | "cancelled" | "timed_out"

// DDD role: ValueObject
// #PollingEndpoint names the control-plane tools used to poll a job per ADR-0040.
// "launch.status" added per ADR-0069 for launch-stack bring-up Task polling.
#PollingEndpoint: "job.status" | "job.result" | "launch.status"

// DDD role: ValueObject
// #JobBucket classifies every MCP tool into a dispatch bucket per ADR-0040.
// A: sync inline (snapshot-instant). B: auto-mode (inline if small, job if large).
// C: always async (job mandatory, no streaming; e.g. archive.tar.create).
// D: sync side-effect (commit fast, audit async).
// E: always async with streaming progress; introduced by the ADR-0040 2026-05-24
// amendment and assigned to subprocess.spawn and launch.up per ADR-0052/ADR-0054/ADR-0069.
#JobBucket: "A_sync_inline" | "B_auto_mode" | "C_always_async" | "D_sync_side_effect" | "E_always_async_streaming"