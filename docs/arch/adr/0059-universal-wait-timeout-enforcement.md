---
status: accepted
accepted_date: 2026-05-26
date: 2026-05-26
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0059 — Universal Wait/Timeout Enforcement (no unbounded waits)

## Context and Problem Statement

[ADR-0040](0040-async-job-control-plane.md) introduced `job.result(wait_ms)`
and [ADR-0052](0052-subprocess-execution-architecture.md) added
`subprocess.result(wait_ms)`. Both tools declared a default of `wait_ms = 0`,
meaning a caller that omits the field receives an immediate "still in
progress" response. The intent was to preserve sub-millisecond latency for
status-check style calls.

In practice this default is the root cause of an observable failure mode:
LLM agents that omit `wait_ms` fall back to a tight polling loop on
`job_status` (or `subprocess_status`), consuming context tokens, never opening
the push channel (`notifications/progress`), and remaining unaware of process
crashes that occur between polls. An incident on 2026-05-26 produced a
48m26s agent run that consumed 12 kB of tokens before bottoming out in a
`subprocess_cancel` cascade against three concurrently-spawned children that
had already failed boot-guard validation.

The same failure shape can occur in any future tool whose contract is
"submit work, then come back later for the result". Without a uniform policy,
every new long-running tool re-introduces the same anti-pattern.

This ADR decides: how substrate enforces an upper bound on every async wait,
how the long-poll defaults are tightened to discourage polling, and how the
configuration layer surfaces the bound to operators.

## Decision Drivers

- LLM agents default to the most permissive shape declared in the JSON schema.
  When the schema says `default: 0`, agents that omit the field believe
  zero-wait is the intended behavior.
- The push channel (`notifications/progress`) is already correct and
  cancellation-safe per [ADR-0040](0040-async-job-control-plane.md) §Push.
  The defect is not in the protocol; it is in the pull-channel ergonomics.
- The server has authoritative knowledge of the wait cap
  (`jobs.quotas.result_max_wait_ms`); clients do not. Encoding the cap in
  the schema default is the cheapest way to surface it.
- Per-tool execution timeouts already exist for jobs
  ([ADR-0040](0040-async-job-control-plane.md) §Job Timeouts) and for
  Bucket A/D tools ([ADR-0006](0006-tokio-runtime-timeout-cancellation.md)).
  No new timeout primitive is needed; only consistent enforcement.

## Considered Options

1. Reject `wait_ms = 0` server-side with `SUBSTRATE_INVALID_ARGUMENT` —
   rejected. Breaks every existing caller; some legitimate status-poll
   patterns (e.g. composite-tool implementations) depend on zero-wait.
2. Silently rewrite `wait_ms = 0` to the server default — rejected. Violates
   least-surprise; an explicit `0` should mean what it says.
3. Tighten the schema default from `0` to a non-zero long-poll value;
   accept explicit `0` for opt-out; cap honored unchanged — accepted.
   Callers who omit `wait_ms` now long-poll by default; callers who pass
   `wait_ms = 0` get the same fast-return semantics they had before.

## Decision Outcome

Chosen option: "Tighten schema default to non-zero; honor explicit zero".

The MCP JSON schema published for every tool that accepts `wait_ms`
declares:

```json
{
  "wait_ms": {
    "type": "integer",
    "minimum": 0,
    "default": 5000,
    "description": "Long-poll timeout in milliseconds. 0 = no wait (return immediately). Capped server-side."
  }
}
```

The Rust handler-side deserialization preserves the `Option<u64>` shape, but
when the field is absent the handler substitutes the configured default
(`jobs.quotas.result_default_wait_ms`, see §Configuration) instead of
treating absence as `wait_ms = 0`. An explicit `wait_ms = 0` in the request
payload is honored as before.

### Affected Tools

The new schema default and handler-side defaulting apply to every tool that
exposes a `wait_ms`-style long-poll parameter:

```text
job.result            wait_ms          ADR-0040
subprocess.result     wait_ms          ADR-0052
subprocess.search     wait_ms          ADR-0057
```

Tools that do NOT expose `wait_ms` are unaffected:

```text
job.status, job.cancel, job.list                     instant control-plane reads
fs.*, sys.*, proc.*, text.* (Bucket A/D)             per-tool timeout ADR-0006
archive.* / fs.find / text.search (Bucket B/C)       submit-then-poll via job.result
```

### Configuration

A new field is added to `#JobQuotas`:

```cue
// result_default_wait_ms is the wait_ms substituted by the handler when the
// caller omits the field. Must satisfy 0 < default <= result_max_wait_ms.
result_default_wait_ms: int & >0 | *5000
```

Constraint: `result_default_wait_ms <= result_max_wait_ms`. CUE enforces this
via an in-schema cross-field reference.

The default of 5000 ms is chosen because:

- It is shorter than typical MCP client request timeouts (30 s in most
  clients) so that even if the call ultimately times out client-side, the
  pull-channel deadline fires first and surfaces a structured error.
- It is long enough for the common case (small archive extract, port-probe
  on local services, fast subprocess exit) to deliver an inline result on
  the first call, eliminating one round-trip.
- It is short enough that a misbehaving worker doesn't block the agent for
  the full `result_max_wait_ms` (30 s) cap.

Operators may override the default to any value in `1..=result_max_wait_ms`
via the `[jobs.quotas]` section of `substrate.toml`.

### Per-Tool Schema Default Override

Future tools MAY declare a per-tool default that differs from
`result_default_wait_ms` if their typical completion time justifies a
different value. The override is declared in the tool's JSON schema
`default` field and overrides the global config default at handler dispatch.

### Validation Layer (Rego)

A new policy `policies/wait_timeout_invariants.rego` enforces:

- Every CUE schema and JSON schema that declares a `wait_ms` property MUST
  also declare a non-zero `default`.
- No `wait_ms` schema may declare an upper bound (`maximum`) greater than
  the configured `jobs.quotas.result_max_wait_ms`.
- The handler-side default substitution MUST be visible in the request
  type (e.g. `#[serde(default = "default_wait_ms")]` not bare
  `#[serde(default)]`).

The third rule is enforced by a clippy-driven AST check in CI; the policy
file documents the requirement for human reviewers.

### Backward Compatibility

- Callers that previously sent `wait_ms = 0` continue to get fast-return
  behavior. No change.
- Callers that previously omitted `wait_ms` and polled in a loop now get
  a 5 s long-poll on the first call. In the common case the result is
  delivered inline and the loop exits after one iteration. In the
  long-running case the loop still works but each iteration is bounded
  by `result_default_wait_ms` and naturally backs off.
- The hints map (`ADR-0040 §Hints`) is extended with a new key:

  ```text
  next_poll_after_ms   integer   suggested delay before next poll;
                                 equals max(remaining_runtime_estimate_ms,
                                            result_default_wait_ms)
  ```

  Agents that respect this hint converge on the push-channel pattern even
  without explicitly opening a subscription.

## Consequences

### Positive

- LLM agents that omit `wait_ms` automatically get long-poll semantics,
  collapsing the 48-minute polling failure mode to at most one polling
  iteration per `result_default_wait_ms` window.
- The schema default doubles as inline documentation: any agent reading the
  schema discovers that long-poll is the intended pull-channel shape.
- No new infrastructure: reuses existing `tokio::time::timeout` wrapper in
  [substrate-jobs/src/registry.rs](../../crates/substrate-jobs/src/registry.rs)
  and existing `result_max_wait_ms` cap.
- Push-channel semantics ([ADR-0040](0040-async-job-control-plane.md) §Push)
  unchanged. Agents that open `notifications/progress` continue to receive
  events at the configured throttle rate and bypass the long-poll path
  entirely.

### Negative

- A misconfigured client that legitimately wants fast-return now must pass
  `wait_ms = 0` explicitly. This is a deliberate trade-off: the failure
  mode of accidental polling is much costlier than the inconvenience of
  one extra field.
- Bucket A / Bucket D tools remain bounded only by
  [ADR-0006](0006-tokio-runtime-timeout-cancellation.md) `global_default_seconds`.
  This ADR does not change that boundary.
- The CUE constraint
  `result_default_wait_ms <= result_max_wait_ms` requires CUE 0.7+ for the
  cross-field reference syntax. The project pins CUE via mise; no impact.

## Validation

- Unit test: deserialize a `JobResultRequest` with `wait_ms` absent; assert
  the resulting `Option<u64>` is `None`; assert the handler substitutes
  `result_default_wait_ms`.
- Unit test: deserialize a `JobResultRequest` with explicit `wait_ms = 0`;
  assert the resulting `Option<u64>` is `Some(0)`; assert the handler
  passes `Duration::from_millis(0)` to the registry (fast-return path).
- Integration test: submit a Bucket C job that completes in 200 ms; call
  `job.result` with no `wait_ms`; assert the result is delivered inline
  in the first call.
- Integration test: submit a Bucket C job that runs for 60 s; call
  `job.result` with no `wait_ms`; assert each call blocks ~5 s and
  returns `state=running` until the job completes; assert at most
  `ceil(60 / 5) = 12` calls suffice to retrieve the result.
- CUE validation: a config that sets `result_default_wait_ms` above
  `result_max_wait_ms` fails `spec lint:cue`.
- Rego policy: a future tool schema with `wait_ms` and no non-zero default
  fails `spec lint:opa`.

## More Information

Related ADRs:

- [ADR-0006](0006-tokio-runtime-timeout-cancellation.md) — tool-level
  timeout primitive; this ADR extends the principle to async waits.
- [ADR-0007](0007-tool-card-narrative-arc.md) — hints map; extended here
  with `next_poll_after_ms`.
- [ADR-0040](0040-async-job-control-plane.md) — pull-channel
  `wait_ms` parameter; this ADR amends the default value.
- [ADR-0052](0052-subprocess-execution-architecture.md) — subprocess
  control-plane inheriting the `wait_ms` shape.
- [ADR-0057](0057-subprocess-output-pagination-and-search.md) —
  `subprocess.search` long-poll path.

## Amendments

### 2026-05-26 — Initial acceptance

ADR-0059 accepted at v1 with the schema-default tightening and
`result_default_wait_ms` configuration field. No code is shipped under
this ADR alone; see commit `feat(jobs): ADR-0059 universal wait timeout
enforcement` for the implementation wave.
