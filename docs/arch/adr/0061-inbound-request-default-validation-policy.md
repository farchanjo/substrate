---
status: accepted
accepted_date: 2026-05-27
date: 2026-05-27
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0061 — Inbound Request Default and Validation Policy

## Context and Problem Statement

The `subprocess.list` bug (commits `ec6c935`, `06dc285`, `530ce06`) exposed a
discipline gap at the MCP inbound layer that is independent of the domain-level
`PageSize` newtype decision in [ADR-0060](0060-page-size-value-object-at-domain-port-boundary.md).

Several MCP tool handlers use a shortcut pattern at the top of their dispatch
function:

```rust
// Shortcut: treat null or empty-object JSON as "all defaults"
let req: SomeTool​Request = if input.is_null() || input == serde_json::Value::Object(Default::default()) {
    SomeTool​Request::default()
} else {
    serde_json::from_value(input)?
};
```

This shortcut is intended to let agents omit the request body when all fields
have sensible defaults. The problem is that `#[derive(Default)]` generates a
`Default` impl that initializes every field to its Rust default — `0` for
integers — regardless of whether a `#[serde(default = "fn")]` attribute is
declared on individual fields. When the shortcut fires, `serde` is bypassed and
the per-field default functions are never called.

`SubprocessListRequest` previously fell into this category: it declared
`#[serde(default = "default_page_size")]` returning `50`, but
`#[derive(Default)]` produced `page_size = 0`, which the shortcut path
delivered to the domain without triggering serde's field-level override.

The tactical fix was a manual `impl Default for SubprocessListRequest` that
returns `page_size = 50`. This is correct but:

1. It requires remembering to write a manual `Default` impl every time a request
   struct has `#[serde(default = "fn")]` field overrides.
2. The condition "manual `Default` required when `#[serde(default = "fn")]` is
   present" is not documented, not linted, and not enforced at PR time.
3. The shortcut pattern itself (`is_null() || empty_object => Req::default()`)
   couples the handler to the `Default` impl in a non-obvious way; a reviewer
   cannot verify correctness without reading both the handler and the struct
   definition simultaneously.
4. The same gap exists in at least the following request types: `JobListRequest`,
   `FsFindRequest`, `NetworkListRequest`, `SubprocessSearchRequest`.

This ADR decides: what is the contract for inbound request validation, and how
is it enforced systematically rather than per-struct by convention?

## Decision Drivers

- **LLM-agent failure mode**: agents frequently send `{}` or omit the body
  entirely when tool descriptions say all fields are optional. The shortcut
  pattern was added to handle this; it must not silently deliver invalid
  field values.
- **`#[derive(Default)]` semantic mismatch**: `derive(Default)` produces
  `0` for `u32`, `false` for `bool`, `None` for `Option<T>`. These are
  Rust-idiomatic defaults that may not match the API contract. `serde`'s
  per-field `default = "fn"` is the correct mechanism for API-contract defaults;
  bypassing `serde` invalidates it.
- **Cross-cutting policy gap**: the shortcut pattern is replicated across
  multiple handlers. A per-handler fix is reactive; a structural rule is
  preventative.
- **Existing enforcement prior art**: the project already uses Rego policies
  (`docs/arch/policies/`) for structural invariants (hexagonal layering,
  no-subprocess, security, audit events, pagination). The same mechanism
  is appropriate here. A CI-script approach (`scripts/check-default-derives.sh`)
  would duplicate the pattern-matching already centralized in OPA.
- **Clippy custom lint**: writing a custom clippy lint requires a `dylint` or
  `cargo-dylint` integration that the project does not yet have; the benefit
  does not justify introducing a new toolchain dependency. A `cargo-deny`
  policy file targets dependency supply chain, not code structure.

## Considered Options

1. **Status quo — manual `Default` impls by convention**: document the rule
   in `CONTRIBUTING.md`; rely on code review. No automated enforcement. The
   gap reappears whenever a reviewer misses the pattern.

2. **Ban `#[derive(Default)]` on request structs (compile error)**: a custom
   clippy lint (via `dylint`) that emits a deny-level warning when a struct
   named `*Request` has both `#[derive(Default)]` and a `#[serde(default = "fn")]`
   field attribute. Strongest enforcement; requires new toolchain dependency.

3. **Remove the `is_null() || empty_object` shortcut; require explicit fields**:
   eliminate the shortcut entirely; agents that send `{}` get a serde
   deserialization error with a recovery hint. The simplest contract, but
   breaks agent ergonomics — LLM agents routinely send empty objects when
   all fields are optional.

4. **Rego policy: `#[serde(default = "fn")]` implies no `#[derive(Default)]`**
   (chosen): add a Rego policy
   `docs/arch/policies/request_default_invariants.rego` that operates on
   AST-extracted struct metadata (present in the existing spec-mode CUE schema
   for request types). The policy denies any request struct that has both
   `derive_default: true` and any field with `serde_default_fn: true`. Wired
   into the `spec lint:opa` CI gate that already runs `hexagonal_layering.rego`
   and `security_invariants.rego`. Does not require a new toolchain dependency.

5. **`Validate` trait with mandatory call in every handler**: define
   `trait Validate { fn validate(&self) -> Result<(), SubstrateError>; }`; add a
   `#[must_use]` wrapper that panics in debug if `.validate()` is not called
   after construction. Enforces post-construction validity; does not prevent
   the underlying derive-vs-serde mismatch.

## Decision Outcome

Chosen option: "Option 4 — Rego policy enforcing that request structs with
`#[serde(default = "fn")]` fields must not also `#[derive(Default)]`".

The Rego toolchain is already present in the project (OPA + conftest, wired via
`spec lint:opa`); adding a new `.rego` file follows the established pattern with
zero new toolchain dependencies. The policy makes the rule machine-checkable at
PR time without requiring a custom clippy lint.

### Rego Policy Specification

New file: `docs/arch/policies/request_default_invariants.rego`

Package: `substrate.request_default_invariants`

Input shape: a list of structs extracted from the CUE schema or from a
compile-time code-gen step:

```json
{
  "structs": [
    {
      "name": "SubprocessListRequest",
      "derive_default": true,
      "fields": [
        { "name": "page_size", "serde_default_fn": "default_page_size" }
      ]
    }
  ]
}
```

Core invariant:

```rego
deny contains msg if {
    s := input.structs[_]
    s.derive_default == true
    f := s.fields[_]
    f.serde_default_fn != null
    msg := sprintf(
        "%s: field '%s' declares #[serde(default = \"%s\")] but the struct also derives Default; write a manual Default impl that matches the serde defaults, or remove #[derive(Default)]",
        [s.name, f.name, f.serde_default_fn]
    )
}
```

Companion invariant: the handler shortcut (`is_null() || empty_object`) MUST
NOT be used with a struct that `#[derive(Default)]` (same input shape, field
`has_null_shortcut: true`):

```rego
deny contains msg if {
    s := input.structs[_]
    s.derive_default == true
    s.has_null_shortcut == true
    msg := sprintf(
        "%s: null/empty-object shortcut used with #[derive(Default)]; replace with a manual Default impl that honors all #[serde(default = \"fn\")] overrides",
        [s.name]
    )
}
```

### Handler Shortcut Contract

The handler shortcut pattern is retained (removing it would break agent
ergonomics) with the following amended contract:

> A handler MAY use the `is_null() || empty_object => Req::default()` shortcut
> only when `Req` has a manual `impl Default` (not `#[derive(Default)]`) that
> explicitly initializes every field to the same value that
> `#[serde(default = "fn")]` would produce.

This contract is enforced by the Rego policy above and documented in the
handler module header via a module-level doc comment:

```rust
//! # Request Default Contract (ADR-0061)
//! Every request struct in this module that participates in the
//! `is_null() || empty_object` shortcut MUST implement `Default` manually
//! (not via `#[derive(Default)]`). The manual impl MUST match every
//! `#[serde(default = "fn")]` field override.
//! Enforced by: docs/arch/policies/request_default_invariants.rego
```

### Affected Request Structs (immediate)

The following structs have `#[serde(default = "fn")]` fields and currently
use `#[derive(Default)]` or have no `Default` impl at all:

| Struct | Affected field | Serde default | Rust default (`derive`) |
|---|---|---|---|
| `SubprocessListRequest` | `page_size` | `50` | `0` (fixed by tactical patch) |
| `JobListRequest` | `page_size` | `50` | `0` |
| `FsFindRequest` | `max_depth` | `16` | `0` |
| `NetworkListRequest` | `page_size` | `50` | `0` |
| `SubprocessSearchRequest` | `page_size` (in `Pagination`) | `100` | `0` |

Each must receive a manual `impl Default` before [ADR-0060](0060-page-size-value-object-at-domain-port-boundary.md)
migration begins; ADR-0060 migration then replaces the raw `u32` with `PageSize`
and the `Default` impl returns `PageSize::default()` (50).

### Enforcement in CI

The `spec lint:opa` stage in CI already runs `conftest test` against every
`.rego` file under `docs/arch/policies/` with companion `_test.rego` vectors.
Adding `request_default_invariants.rego` and
`request_default_invariants_test.rego` requires no CI pipeline changes.

The input data for the new policy (struct metadata with `derive_default` and
`serde_default_fn` fields) is generated by a lightweight `cargo metadata` +
`syn` parse step in `scripts/extract_request_structs.py` (new script; outputs
JSON to stdout for `conftest`). This keeps the pipeline as a pure OPA
evaluation without a custom clippy lint.

## Consequences

### Positive

- The `derive(Default)` / `serde(default = "fn")` mismatch is caught at PR
  time by the CI Rego gate before it reaches production.
- The handler shortcut pattern is retained with a machine-verified contract
  instead of an honor-system convention.
- No new toolchain dependency; the OPA + conftest path is already in place.
- The policy and its test vectors serve as executable documentation of the rule
  for future contributors.

### Negative

- `scripts/extract_request_structs.py` is a new maintenance surface: it must
  be updated when request struct naming conventions change.
- The Rego policy operates on extracted metadata, not on raw Rust source.
  A mismatch between the extractor and the actual source could produce false
  negatives. Mitigation: the extractor is unit-tested independently; the policy
  test vectors cover both pass and fail cases.
- The policy does not prevent the shortcut from being added to new handlers;
  it only catches the case where the shortcut is combined with a mismatched
  `Default` impl. A handler that adds the shortcut against a newly introduced
  struct without a manual `Default` is caught only on the next CI run.

### Migration order

1. Author `docs/arch/policies/request_default_invariants.rego` and companion
   `_test.rego`.
2. Author `scripts/extract_request_structs.py` and add it to the `spec lint:opa`
   CI step.
3. Run the policy against the current codebase; confirm the five structs in the
   table above are flagged.
4. Write manual `impl Default` for each flagged struct.
5. Remove `#[derive(Default)]` from each flagged struct.
6. Re-run CI to confirm zero Rego denials.
7. [ADR-0060](0060-page-size-value-object-at-domain-port-boundary.md) migration
   begins; manual `Default` impls updated to return `PageSize::default()`.

**Tests required:**

- Rego test: a struct with `derive_default: true` and a field with
  `serde_default_fn` set → deny.
- Rego test: a struct with `derive_default: false` and manual impl, same
  field → allow.
- Rego test: a struct with `has_null_shortcut: true` and `derive_default: true`
  → deny.
- Python extractor test: a Rust source file with `#[derive(Default)]` and a
  `#[serde(default = "default_page_size")]` field produces correct JSON metadata.
- Integration test: send `{}` to `subprocess.list`; assert `page_size = 50`
  is applied (not `0`), confirming the manual `Default` impl is honored by
  the shortcut path.

## Related ADRs

- [ADR-0057](0057-subprocess-output-pagination-and-search.md) — declares the
  pagination value contract violated by the trigger bug.
- [ADR-0052](0052-subprocess-execution-architecture.md) — subprocess bounded
  context; defines `SubprocessListRequest`.
- [ADR-0059](0059-universal-wait-timeout-enforcement.md) — established the
  pattern of enforcing a `#[serde(default = "fn")]` requirement via Rego policy
  (the `wait_timeout_invariants.rego` policy's third rule mirrors the concern
  addressed here).
- [ADR-0060](0060-page-size-value-object-at-domain-port-boundary.md) — companion
  ADR: promotes `page_size` to a `PageSize` newtype at the domain port boundary.
  ADR-0061 migration must complete before ADR-0060 migration begins so that
  all manual `Default` impls are in place when `PageSize::default()` replaces
  raw integers.

## References

- Trigger commits: `ec6c935`, `06dc285`, `530ce06` (subprocess_list page_size bug fix).
- `docs/arch/policies/subprocess_pagination_invariants.rego` — existing Rego policy
  that already enforces `page_size in [1, 10000]` at the conftest level, but only
  for pagination objects received over the wire; does not cover the `Req::default()`
  shortcut path.
- `docs/arch/policies/wait_timeout_invariants.rego` — prior art: Rego enforcement
  of a serde default requirement (the third rule in that policy).
