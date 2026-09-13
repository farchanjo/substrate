---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0025 — Bounded Context Interactions (Shared Kernel)

## Context and Problem Statement

Six bounded contexts (ADR-0002) must occasionally exchange data — for example,
a path value produced by filesystem-query may be consumed as input to
filesystem-mutation, and audit events from any context are observed by the
policy layer. Without a contract for how contexts may reference each other's
types, accidental aggregate sharing or hidden coupling will erode the context
boundaries over time.

The problem is: what is the permitted interaction pattern between bounded
contexts in substrate, and how is it enforced?

## Decision Drivers

- Context independence: each context must compile and be tested without the
  others.
- No runtime message-passing infrastructure for MVP (see ADR-0026).
- Shared types must be value objects with value semantics, not aggregates.
- The policy layer must be able to observe any context's output without knowing
  its internal model.
- Enforcement must be structural (compiler), not social (convention).

## Considered Options

- Option A: Shared kernel of pure value objects in `substrate-domain`; each
  context owns its own ports and adapters.
- Option B: Aggregate sharing — contexts import each other's types directly.
- Option C: Anti-corruption layers with explicit translation between every pair
  of contexts.

## Decision Outcome

Chosen option: "Option A — shared kernel of pure value objects in
`substrate-domain`", because it provides the minimum necessary shared surface
without coupling aggregates, and Cargo's dependency graph enforces the rule
structurally.

### Shared Kernel Contents

The following types live in `substrate-domain` and may be used by any bounded
context crate:

- `JailedPath`: a validated, canonicalized path guaranteed to be within an
  allowed root. Constructed only by the policy crate; passed by value.
- `ToolResult`: the envelope returned by every MCP tool call, carrying either
  a success payload or a structured error.
- `PageCursor`: opaque pagination token for listing operations.
- `ProgressToken`: opaque token forwarded to the MCP client for streaming
  progress notifications.
- `AuditEvent`: a value object capturing actor, tool name, arguments hash,
  outcome, and timestamp. Written by adapters; consumed by the policy layer
  for logging. It is operational telemetry, not a domain event (see ADR-0026).

### Rules

1. No bounded context crate may depend on another bounded context crate.
   Each adapter uses its own port trait even if the underlying system call
   is similar (e.g., both filesystem-query and filesystem-mutation use
   `nix::stat`, but each defines its own port independently).

2. `substrate-domain` is the only shared dependency between context crates.
   It contains no I/O, no system calls, and no async code.

3. `substrate-policy` may depend on `substrate-domain` and on
   `substrate-config`. It must not depend on any bounded context crate.

4. Data passed from one context's output to another context's input crosses
   the boundary as a shared kernel value object (typically `JailedPath` or
   a plain string), translated at the composition root in
   `substrate-mcp-server`.

5. If a type appears useful across contexts but is not yet in the shared
   kernel, it must be proposed as a shared kernel addition with an ADR update
   — not imported directly from another context crate.

### Consequences

#### Positive

- Contexts remain independently compilable and testable.
- The shared surface is small, versioned, and visible in one place.
- Policy and audit logic apply uniformly across all contexts without
  coupling to context internals.

#### Negative

- Some data structures are nominally duplicated across context ports (e.g.,
  each context defines its own error type rather than sharing one).
- Cross-context composition (e.g., grep result fed into a mutation) must be
  orchestrated at the composition root, not inside a context.

## Validation

- `cargo deny check` must report zero inter-context direct dependencies.
- `cargo test -p substrate-domain` must pass with no feature flags beyond
  `default`.
- A CI job must run `cargo check -p <each context crate>` independently,
  without building the full workspace, to confirm isolation.

## Links

- Related: [ADR-0002](0002-bounded-contexts.md)
- Related: [ADR-0022](0022-project-layout.md)
