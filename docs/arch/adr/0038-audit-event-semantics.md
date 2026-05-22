---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0038 — Audit Event Semantics (Ordering, Correlation, Sequence)

## Context and Problem Statement

ADR-0009 mandates audit events for every tool invocation but does not specify: (a) whether events are emitted before or after tool execution, (b) how the `correlation_id` UUIDv7 crosses `tokio::spawn` boundaries including `spawn_blocking`, or (c) how events are ordered when the system clock jumps backward (NTP correction, VM migration, leap-second smear). Without these specifications, implementations diverge silently, forensic reconstruction becomes unreliable, and the SLI measurement contracts in ADR-0039 lack a stable measurement source.

## Decision Drivers

- Forensic integrity: a crash between the decision to mutate and the mutation itself must leave a detectable trace in the audit log (ADR-0029).
- Correlation across async boundaries: `tokio::spawn` does not automatically propagate `tracing` span context; without explicit `.instrument()` calls, spawned tasks emit orphaned log events.
- Clock jump immunity: SIEM systems that sort events by `timestamp` can mis-order events after an NTP step; a monotonic counter independent of wall time is required.
- Concurrency visibility: operators need to know how many tool calls were in-flight when a given call began, to reconstruct resource contention during incidents.

## Considered Options

1. Single post-execution event for all tools; sort by UUIDv7 (which encodes timestamp).
2. Pre/post events for mutating tools only; monotonic `seq` counter; `active_requests_at_start` snapshot.
3. Pre/post events for all tools; reduces post-hoc ambiguity at the cost of double event volume.
4. External distributed trace (OpenTelemetry OTLP); deferred from MVP scope.

## Decision Outcome

Chosen option: **2 — pre/post for mutating tools, post-only for read-only tools, with monotonic `seq` counter and `active_requests_at_start` snapshot**, because it minimises event volume for high-frequency read operations while providing complete forensic evidence for all state-modifying operations, and adds clock-immune ordering without external dependencies.

### Consequences

#### Positive

**Emission ordering**

Read-only tools (`fs.find`, `sys.info`, `text.search`, `archive.hash`, `proc.list`, and all other tools whose execution produces no side-effects observable outside the process) emit ONE audit event AFTER the tool returns. The `outcome` field carries one of `{success, error, cancelled, timeout}`.

Mutating tools (`fs.write`, `fs.remove`, `fs.rename`, `fs.set_permissions`, `proc.signal`, `archive.create`, `archive.extract`) emit TWO audit events:

- **Event 1 — before execution**: `outcome = "attempted"`. Includes the full argument set and the `elicitation_confirmed: bool` flag (true when the operator approved via elicitation, false for dry-run bypass or default-permitted operations). Emitted synchronously before the first mutating syscall.
- **Event 2 — after execution**: `outcome ∈ {success, error, cancelled, timeout}`. Emitted after the operation completes or is aborted.

Rationale: if the substrate process crashes after Event 1 is written but before the mutating syscall completes, the audit log shows `attempted` without a corresponding terminal outcome. A SIEM or forensic reviewer can unambiguously identify incomplete operations, distinguishing them from operations that never started or completed successfully.

`AuditOutcome` enum is extended to `{attempted, success, error, cancelled, timeout}`. Prior values `{success, error, cancelled}` remain valid; `attempted` and `timeout` are additive.

**Correlation ID propagation**

One UUIDv7 is generated per MCP tool call at the adapter entry point (the outermost MCP request handler), before any async work begins.

The ID is stored in the active `tracing::Span` via `info_span!("mcp_tool", correlation_id = %id)`. Every `tokio::spawn` inside a tool implementation MUST chain `.instrument(Span::current())` to inherit the span context. This is enforced via a code review checklist item added to the PR template.

Inside `spawn_blocking` closures, `tracing` span context is NOT automatically propagated because `spawn_blocking` may execute on a thread pool thread that does not inherit the async runtime's span stack. Tool implementations MUST explicitly pass `correlation_id` as a function parameter to the closure and emit log events using `info!(correlation_id = %id, "...")` with the explicit field. This pattern is documented in the substrate coding guide.

For nested tool invocations within a prompt workflow (for example, a workflow prompt that calls `fs.find` then `text.search`), each tool call receives its own `correlation_id`. The workflow-level invocation additionally attaches a `workflow_id` field (also UUIDv7) to all child tool spans, enabling downstream correlation of the full workflow without conflating individual tool SLIs.

**Monotonic sequence counter**

A `seq: u64` field is added to `AuditEvent`. The value is atomically incremented via `AtomicU64::fetch_add(1, Ordering::SeqCst)` from a process-global static counter initialised to 0 at startup.

SIEM pipelines and compliance consumers MUST sort audit events by `seq` for ordering purposes rather than `timestamp`. This makes ordering immune to NTP clock jumps, VM live-migration wall-clock discontinuities, and leap-second smear. The `timestamp` field is retained for human readability and approximate absolute time placement.

**Active concurrency snapshot**

Each audit event includes `active_requests_at_start: u32`, which records the number of concurrent active tool-call executions at the moment the current tool entered its execution phase. This value is sourced from the same atomic counter used by ADR-0017's concurrency limit enforcement. It allows operators to reconstruct resource contention timelines from audit logs alone.

#### Negative

- Mutating tools now emit two events; high-frequency batch operations will double audit log volume for those tool categories. Operators may need to increase log rotation limits (see ADR-0016).
- The requirement to explicitly pass `correlation_id` into every `spawn_blocking` closure is a manual discipline that cannot be enforced by the compiler. Code review is the only enforcement mechanism until a proc-macro or lint rule is developed.
- Consumers that sorted by `timestamp` prior to this ADR must be migrated to sort by `seq`. The two fields will coexist indefinitely.

## Validation

- Unit tests assert that a successful `fs.write` call produces exactly two audit events with outcomes `attempted` and `success` in that `seq` order.
- Unit tests assert that a panicking `fs.write` (simulated by injecting a fault before the terminal event) leaves only an `attempted` event in the emitted stream.
- Unit tests assert that `seq` values are strictly monotonic across 1000 concurrent audit emissions using `tokio::join!`.
- Integration tests verify that `spawn_blocking` closures within tool implementations carry the correct `correlation_id` in their log output by inspecting stderr JSON Lines.
- CI lint rule (via custom clippy lint or grep) asserts that every `tokio::spawn` inside a `src/tools/` module is followed by `.instrument(Span::current())` on the same call chain.

## Cross-References

- [ADR-0009](0009-observability.md) — Observability (parent; defines the tracing framework this ADR extends)
- [ADR-0017](0017-concurrency-limits.md) — Concurrency limits (source of `active_requests_at_start` counter)
- [ADR-0010](0010-error-taxonomy.md) — Tool error codes (forensic integrity requirement for incomplete mutations)
- [ADR-0039](0039-sli-definitions.md) — SLI definitions (consumes `duration_ms` and `outcome` from AuditEvent)

## Amendments

### 2026-05-21 — Extended by ADR-0040 async-job-control-plane

The async job control plane introduces long-running jobs with lifecycle transitions and progress streams. Each job lifecycle step generates audit events that extend the `AuditEvent` structure with job-specific fields. All new fields are optional in the base schema unless noted; they appear only when the event is associated with the relevant feature.

**Additions:**

- `client_id` — string field on `AuditEvent`; derived from the MCP `initialize` request `clientInfo.name` field. REQUIRED on all events emitted after session establishment, absent on pre-session startup events.
- `job_id` — string (UUIDv7 base32); present on every event associated with a job lifecycle transition (submitted, started, progress, completed, cancelled, timed-out).
- `idempotency_key` — string (UUIDv7); present when the originating tool call carried an `idempotency_key` parameter, enabling deduplication audit trails.
- `sequence_number` — integer; present on job progress events, identifying position within the progress stream for a given `job_id`. Distinct from the process-global `seq` counter defined in the base ADR.
- `progress_events_dropped` — integer counter; present in the terminal-state audit event for a job (outcome `success`, `error`, `cancelled`, or `timed-out`). Records how many progress events were dropped by the mpsc backpressure mechanism during the job's lifetime. A non-zero value indicates consumer lag.

### 2026-05-21 — Extended by ADR-0042 capability-adapter-factory

The capability adapter factory emits startup audit events that document the tier selections made for each port. These events enable operators to verify that the expected kernel-level capabilities are active without inspecting process state directly.

**Additions:**

- `simd_tier` — string field on `AuditEvent`; one of `avx512`, `avx2`, `sse42`, `sse2`, `neon`, or `portable`. Present on the startup `SUBSTRATE_SIMD_TIER_DETECTED` event and optionally on per-tool events when SIMD was on the critical path for that invocation.
- `walker_tier`, `watcher_tier`, `jail_tier`, `hash_tier`, `stat_tier` — string fields present on the startup `SUBSTRATE_CAPABILITY_TIERS_SELECTED` event, recording the selected implementation tier for each capability port.
- New audit event code `SUBSTRATE_CAPABILITY_TIERS_SELECTED` — emitted once at startup after all capability probes and factory builds complete. Payload includes the full tier map (all five `*_tier` fields).
- New audit event code `SUBSTRATE_SIMD_TIER_DETECTED` — emitted once at startup. Payload: `simd_tier` string.
- New audit event code `SUBSTRATE_JAIL_DEGRADED` — emitted at startup if PathJail falls back to the userspace tier (see ADR-0035 amendment in this same wave). Severity: WARN. Always paired with a `tracing::warn!` log line.
- New audit event code `SUBSTRATE_SUBPROCESS_POLICY_VERIFIED` — emitted once at startup, confirming that the no-subprocess Rego policy gate passed at build time. Payload may include `binary_hash` (blake3 hex of the substrate-mcp-server binary) for supply-chain audit purposes.
