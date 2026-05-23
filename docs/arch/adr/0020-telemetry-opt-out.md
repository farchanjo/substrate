---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0020 — Telemetry Opt-Out

## Context and Problem Statement

Substrate operates as a local MCP server with direct access to the operator's filesystem and process table. Sending usage telemetry to external endpoints from within this trust boundary would constitute an unexpected outbound channel — one that could carry sensitive metadata (tool invocation patterns, path prefixes, process names) to third-party infrastructure without explicit operator consent. The question is whether substrate should emit any telemetry, and if so, under what conditions and to which endpoints.

## Decision Drivers

- Outbound network is OFF by default (Cargo feature `outbound-net` controls network capability; see ADR-0004).
- Operators installing substrate on sensitive machines must not have usage data leave the host without explicit configuration.
- Local audit logs already provide sufficient operational visibility for debugging and compliance.
- A future opt-in telemetry path must not require changes to the binary interface or break existing deployments.
- Privacy posture: no PII or path data should ever be included in telemetry payloads even when opt-in is active.

## Considered Options

1. No external telemetry; local audit log only; opt-in via future crate behind Cargo feature (selected)
2. Anonymous telemetry on by default with opt-out environment variable
3. Telemetry always on, no opt-out
4. Telemetry off permanently; no provision for future opt-in

## Decision Outcome

Chosen option: "No external telemetry; local audit log only; opt-in via future crate behind Cargo feature", because it respects the operator's trust boundary and aligns with the default-deny posture established by the security model.

### Current behavior (v1.x baseline)

Substrate emits zero bytes to any external endpoint. All operational observability is provided by the structured audit log written to stderr (see ADR-0018). No metrics, traces, or usage events are sent to any remote collector, crash reporter, or analytics endpoint.

This behavior is unconditional in the default binary. No environment variable, configuration flag, or runtime toggle enables outbound telemetry in the current release.

### Future opt-in path

When a telemetry subsystem is warranted (e.g., for hosted deployments where operators have explicitly consented), it will be implemented as a separate crate, `substrate-telemetry`, gated behind the `telemetry` Cargo feature:

```toml
# Cargo.toml (future)
[features]
default = []
telemetry = ["dep:substrate-telemetry"]
outbound-net = []   # existing feature; telemetry implies outbound-net
```

Enabling `telemetry`:

- Requires the operator to also set `outbound_network = true` in TOML config (no implicit network grant).
- Requires an explicit `[telemetry]` section in TOML with `endpoint`, `api_key`, and `consent = true`.
- Emits only aggregated, non-path, non-PII counters (tool invocation counts, error rates, latency percentiles).
- Telemetry payload schema will be published and versioned; operators can inspect payloads via a `--telemetry-dry-run` flag.

### Audit log as the local telemetry substitute

The structured stderr audit log (ADR-0018) satisfies local observability requirements:

- Tool invocation counts and outcomes are derivable from log events.
- Error rates and latency histograms can be computed by any log analysis tool (jq, Vector, Loki).
- No data leaves the host.

### Consequences

#### Positive

- Operators on air-gapped or sensitive machines are not surprised by outbound connections.
- Default-deny network posture (ADR-0004) is reinforced, not undermined, by this decision.
- Future opt-in design is explicit and auditable at the configuration level.

#### Negative

- Substrate maintainers have no visibility into usage patterns or error distributions in the field without operator cooperation.
- Debugging production issues on operator machines requires log access rather than centralized observability.

## Validation

- Network integration tests assert that substrate makes zero outbound TCP/UDP connections when compiled without `telemetry` and `outbound-net` features.
- `cargo audit` and dependency review gate confirms no telemetry SDK is pulled in transitively.
- CI enforces feature matrix: `default`, `outbound-net`, `telemetry` (future) are tested independently.

## Cross-References

- ADR-0019 — Privacy policy baseline (data classification, retention, operator data handling obligations)
- ADR-0011 — Configuration schema (TOML structure governing security and network features)
