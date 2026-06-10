---
status: accepted
accepted_date: 2026-05-27
date: 2026-05-27
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0060 — PageSize Value Object at Domain Port Boundaries

## Context and Problem Statement

[ADR-0057](0057-subprocess-output-pagination-and-search.md) declares
`page_size: u32 — 1..=10000` as a valid range for the `Pagination` value
object. A bug in `subprocess.list` (commits `ec6c935`, `06dc285`, `530ce06`)
showed that `SubprocessPort::list` accepts `page_size: u32` as a raw primitive
at the domain port boundary. A caller supplying `page_size = 0` triggers
`iter().take(0).collect()`, silently returning an empty list — the correct range
is declared only in prose, not enforced at any compile-time or runtime boundary
inside the domain.

The same gap exists in every other paginated port:

- `JobPort::list` (`job.list`, ADR-0040)
- `NetworkInfoPort::list_tcp` / `list_udp` (`net.tcp_list`, `net.udp_list`,
  ADR-0058)
- `FsQueryPort::find` (`fs.find`, ADR-0041)
- Future paginated ports follow the same raw-`u32` pattern by convention

A raw `u32` crossing a port boundary cannot carry the invariant `>= 1` without
a runtime assertion or a guard at every call site. Neither the Rust type system
nor `clippy` surfaces the gap until a test fails. The `debug_assert!` added as a
tactical fix is elided in release builds.

The question is: should `page_size` be promoted to a typed `PageSize` newtype
that encodes `NonZeroU32` + clamped maximum at the domain port level, making the
constraint impossible to violate at compile time across all paginated ports?

## Decision Drivers

- **Compile-time vs. runtime contract**: a newtype on `NonZeroU32` makes
  `page_size = 0` a type error; the bug cannot be reintroduced without a
  deliberate `unsafe` bypass.
- **Cross-cutting port surface**: seven ports currently accept raw `page_size`;
  fixing each call site individually is linear cost with no systemic guarantee
  against future regressions.
- **LLM-agent failure mode**: agents that omit pagination fields trigger
  `Default` paths; an invalid zero silently narrows results rather than
  surfacing an error. Silent truncation is harder to diagnose than a
  `SUBSTRATE_INVALID_ARGUMENT` error.
- **Clamped maximum**: wrapping in a newtype allows `PageSize::try_from` to
  reject values above 10000 with a structured error rather than silently
  clamping, which would mask misconfigured agents.
- **Bounded refactor cost**: the newtype lives in `substrate-domain`; all
  callers are within the same workspace and compile together. A one-shot
  structural change is lower risk than incremental call-site patches.

## Considered Options

1. **Status quo — raw `u32` with `debug_assert!`**: keep `page_size: u32`
   at the port boundary; rely on `debug_assert!(page_size > 0)` in each
   registry implementation for debug builds. No structural change; the
   invariant remains prose-only in release builds.

2. **Runtime guard at every call site**: add `if page_size == 0 { return
   Err(SUBSTRATE_INVALID_ARGUMENT) }` in each handler and adapter. Prevents
   silent empty results but requires N guard clauses, is not compile-verified,
   and does not enforce the upper bound.

3. **`PageSize` newtype wrapping `NonZeroU32` with `MAX = 10000`** (chosen):
   define `PageSize` in `substrate-domain/src/value_objects/pagination.rs`;
   implement `TryFrom<u32>` returning `SubstrateError::InvalidArgument` for
   `0` or `> 10000`; replace `u32` at every paginated port boundary; convert
   at the outermost inbound layer (MCP handler) before the domain is reached.

4. **`Validate` trait on request structs**: define a `Validate` trait with a
   `fn validate(&self) -> Result<(), SubstrateError>` method; call it in every
   handler after deserialization. Catches the bug but requires N trait impls,
   does not give compile-time guarantees, and keeps the invalid value in scope
   until `validate()` is called.

## Decision Outcome

Chosen option: "Option 3 — `PageSize` newtype wrapping `NonZeroU32` with
`MAX = 10_000`".

The zero-page bug is a class of failure, not a one-off: any raw primitive at
a port boundary can carry an invalid value across layers. Promoting `page_size`
to a newtype moves the enforcement from a runtime assertion that is elided in
release builds to a type-system invariant that is checked at construction time
and cannot be bypassed without an explicit `unsafe` block.

### Value Object Definition

```rust
// substrate-domain/src/value_objects/pagination.rs

use std::num::NonZeroU32;
use crate::error::{SubstrateError, ErrorCode};

/// Valid page size: 1..=10_000, as declared by ADR-0057.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PageSize(NonZeroU32);

impl PageSize {
    pub const MIN: u32 = 1;
    pub const MAX: u32 = 10_000;
    pub const DEFAULT: PageSize = // safety: 50 != 0 and <= MAX
        PageSize(unsafe { NonZeroU32::new_unchecked(50) });

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl TryFrom<u32> for PageSize {
    type Error = SubstrateError;

    fn try_from(n: u32) -> Result<Self, Self::Error> {
        if n == 0 || n > Self::MAX {
            Err(SubstrateError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "page_size must be in [1, {}]; got {}",
                    Self::MAX, n
                ),
            ))
        } else {
            // SAFETY: n > 0 is enforced by the branch above
            Ok(PageSize(NonZeroU32::new(n).expect("n > 0")))
        }
    }
}

impl Default for PageSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}
```

`PageSize` is a pure value object: no I/O, no async, no external dependencies.
It lives in the `domain` layer and is imported by the `ports` layer via
`substrate-domain`.

### Port Boundary Change

Every paginated port method replaces `page_size: u32` with `page_size: PageSize`:

```rust
// Before (substrate-domain/src/ports/subprocess.rs)
pub trait SubprocessPort: Send + Sync {
    async fn list(&self, page_size: u32, ...) -> Result<Vec<SubprocessHandle>>;
}

// After
use crate::value_objects::pagination::PageSize;

pub trait SubprocessPort: Send + Sync {
    async fn list(&self, page_size: PageSize, ...) -> Result<Vec<SubprocessHandle>>;
}
```

Affected ports:

| Port | Method | Crate |
|---|---|---|
| `SubprocessPort` | `list` | `substrate-subprocess` |
| `JobPort` | `list` | `substrate-jobs` |
| `NetworkInfoPort` | `list_tcp`, `list_udp` | `substrate-net` |
| `FsQueryPort` | `find` | `substrate-fs` |

### Inbound Conversion

The MCP handler deserializes `page_size` as `Option<u32>` (from JSON) and
converts at the handler boundary before calling the port:

```rust
// In the MCP handler (substrate-mcp-server)
let page_size = match req.page_size {
    Some(n) => PageSize::try_from(n)?,
    None => PageSize::default(),
};
```

`?` propagates `SubstrateError::InvalidArgument` as `SUBSTRATE_INVALID_ARGUMENT`
to the MCP caller. The domain never receives an invalid value.

### Consequences

**Positive:**

- `page_size = 0` is a type error at the domain port boundary; the
  `debug_assert!` tactical fix can be removed.
- All paginated ports enforce the 1..=10000 range declared by ADR-0057 without
  per-call-site guards.
- `PageSize::default()` returns 50 (the `SubprocessListRequest::default()`
  value from the tactical fix), ensuring the `Default` path and the newtype
  agree without manual synchronization.
- A structured `SUBSTRATE_INVALID_ARGUMENT` error with a human-readable message
  replaces a silent empty result.

**Negative:**

- All four ports change their signatures simultaneously. Any crate that
  implements a port against a mock or a test double must be updated.
- Migration must be done as a single wave (the trait signature changes are
  breaking); a phased rollout is possible only if ports are versioned, which
  they currently are not.

**Migration order:**

1. Define `PageSize` in `substrate-domain/src/value_objects/pagination.rs`.
2. Export it from `substrate-domain/src/value_objects/mod.rs`.
3. Update each port trait signature (one commit per bounded context).
4. Update each adapter `impl` and each MCP handler conversion point.
5. Remove `debug_assert!(page_size > 0)` from `registry.rs:985`.
6. Add unit tests: `PageSize::try_from(0)` → `Err`, `try_from(10001)` → `Err`,
   `try_from(1)` → `Ok`, `try_from(10000)` → `Ok`, `Default::default()` → 50.
7. Update the Rego policy `docs/arch/policies/subprocess_pagination_invariants.rego`
   to reference this ADR in its header cross-references.

**Tests required:**

- `PageSize` unit tests (see migration step 6).
- Each adapter's list method: call with `PageSize::try_from(1)` and assert
  non-empty return; call with `PageSize::try_from(10000)` and assert no panic.
- MCP handler integration test: send `{"page_size": 0}` and assert
  `SUBSTRATE_INVALID_ARGUMENT` is returned.
- MCP handler integration test: send `{}` (page_size absent) and assert
  default of 50 is applied.
- Extend the existing Gherkin feature
  `subprocess-list-empty-args-returns-handles.feature` with a scenario for
  `page_size = 0` returning an error rather than an empty list.

## Related ADRs

- [ADR-0057](0057-subprocess-output-pagination-and-search.md) — declares
  `page_size: 1..=10000` in prose; this ADR promotes that constraint to
  a compile-time type invariant.
- [ADR-0052](0052-subprocess-execution-architecture.md) — subprocess bounded
  context; defines `SubprocessPort` and `subprocess.list`.
- [ADR-0040](0040-async-job-control-plane.md) — `JobPort.list` is a peer
  paginated port affected by this change.
- [ADR-0059](0059-universal-wait-timeout-enforcement.md) — established the
  precedent of tightening a numeric parameter contract at the schema and handler
  boundary; this ADR applies the same principle to `page_size`.
- [ADR-0061](0061-inbound-request-default-validation-policy.md) — companion
  ADR: governs the handler-level `Default` / `is_null()` shortcut policy that
  allowed the zero-page bug to survive the inbound layer.
- [ADR-0008](0008-mcp-features-map.md) — declares the handler/protocol-layer
  pagination clamp (`max_page_size` default 500). The `PageSize` value object
  defined here owns the outer domain bound (`1..=10000`, reject-if-outside);
  the per-tool handler cap in ADR-0008 is the inner clamp applied after this
  value object validates. The 2026-06-10 amendment of ADR-0008 documents the
  layering reconciliation in full.

## Amendments

### 2026-06-10 — Relationship to the ADR-0008 handler-layer pagination cap

This value object enforces the domain-port bound `MIN = 1`, `MAX = 10_000`,
`DEFAULT = 50`. It does NOT set the maximum page size a paginated tool will
actually return. [ADR-0008](0008-mcp-features-map.md) defines a separate
handler/protocol-layer clamp that runs AFTER `PageSize::try_from` succeeds:
`fs.find`, `proc.list`, and `text.search` clamp the validated value down to
500 via `.get().min(cap)`, and `fs.read_dir` clamps to 5_000. The TOML
`[protocol] max_page_size` (default 500) configures this clamp.

The two layers are complementary, not contradictory: a request with
`page_size` in `501..=10_000` is accepted by this value object and then
silently clamped down to the per-tool ceiling at the handler; only
`page_size = 0` or `page_size > 10_000` is rejected with
`SUBSTRATE_INVALID_ARGUMENT`. A second associated default,
`DEFAULT_PAGINATION = 100`, is provided for line- and record-oriented
operations (subprocess result, search, network TCP/UDP list).

## References

- Trigger commits: `ec6c935`, `06dc285`, `530ce06` (subprocess_list page_size bug fix).
- `docs/arch/policies/subprocess_pagination_invariants.rego` — existing Rego policy
  enforcing `page_size in [1, 10000]` at CI/conftest level.
- `docs/arch/specs/features/subprocess/subprocess-list-empty-args-returns-handles.feature` —
  Gherkin coverage added by the tactical fix.
