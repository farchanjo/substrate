---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0026 — Domain Events Policy (Not Used for MVP)

## Context and Problem Statement

Domain-driven design supports bounded context integration via domain events:
a context publishes a fact about something that happened, and other contexts
react asynchronously. Substrate has a cross-cutting audit concern — every tool
invocation and its outcome should be observable — that could be modeled as a
stream of domain events.

The problem is: should substrate adopt a domain event bus for MVP, or should
the audit trail be handled by a simpler operational mechanism?

## Decision Drivers

- MVP scope must be minimal: substrate is a stdio-transport MCP server with
  no external network services and no persistent storage.
- An in-process event bus adds async coordination complexity (channels,
  backpressure, subscriber lifecycle) without a proportional benefit at MVP
  scale.
- Audit data is operational telemetry consumed by the process itself (log
  emission to stderr) and by a future observability layer, not by other
  bounded contexts reacting to business facts.
- The shared kernel already defines `AuditEvent` as a value object. Adding
  a publish/subscribe mechanism would require infrastructure that violates the
  zero-infra constraint on `substrate-domain`.

## Considered Options

- Option A: No domain events for MVP; audit is a direct function call from
  each adapter into a logging sink.
- Option B: In-process tokio broadcast channel as a lightweight event bus.
- Option C: Structured domain events with a pluggable dispatcher, reserved
  for post-MVP.

## Decision Outcome

Chosen option: "Option A — no domain events for MVP; audit as direct logging",
because the operational cost of an event bus exceeds its benefit at current
scale, and the audit concern does not require bounded context decoupling — it
is infrastructure, not domain logic.

### What Audit Is and Is Not

`AuditEvent` is a value object in the shared kernel (ADR-0025). It is
constructed by each adapter immediately before or after a tool call and passed
to a logging function that writes structured JSON to stderr. This is
operational telemetry, equivalent to a structured log line.

It is explicitly **not** a domain event in the DDD sense. It does not represent
a business fact that other bounded contexts need to react to. No subscriber,
no channel, no event loop.

### Future Reservation

If a future version of substrate requires cross-context reactions (e.g.,
a filesystem-mutation event triggering a cache invalidation in a query cache),
domain events should be introduced at that point. The recommended path is:

1. Define a `DomainEvent` trait in `substrate-domain`.
2. Add an `EventDispatcher` port in `substrate-domain`.
3. Implement a tokio-broadcast-based adapter in a new `substrate-events` crate.
4. Wire the dispatcher at the composition root.

This path is reserved and not implemented. Any implementation before it is
required is premature.

### Consequences

#### Positive

- Zero async coordination overhead for MVP.
- Audit path is a direct, traceable function call — easy to test and debug.
- No event schema versioning concern at MVP.

#### Negative

- If cross-context reactions are needed before a planned v2, refactoring to
  an event bus will touch every adapter.
- The audit log is write-only from the process perspective; there is no
  in-process query interface over past events.

## Validation

- `grep -r "broadcast\|EventBus\|DomainEvent" crates/` must return no results
  in the MVP codebase.
- Each adapter's audit call must be a synchronous function call to a logging
  sink, verifiable by reading the adapter source.
- The future reservation path must be documented in `docs/arch/` before any
  implementation begins in v2.

## Links

- Related: [ADR-0002](0002-bounded-contexts.md)
- Related: [ADR-0009](0009-observability.md)
