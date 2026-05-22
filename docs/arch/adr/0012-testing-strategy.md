---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0012 — Testing Strategy

## Context and Problem Statement

substrate exposes filesystem, archive, and process operations to LLM agents. Incorrect behaviour — especially path jail escapes or silent data loss — represents a high-severity failure mode. The testing strategy must provide confidence in correctness, security invariants, and protocol contract stability without imposing excessive CI runtime overhead.

## Decision Drivers

- Path jail invariants (ADR-0004) must be verified by property-based tests that generate adversarial inputs, not just hand-crafted cases.
- MCP tool output schemas are consumed directly by LLM agents; schema drift is a breaking change and must be caught before release.
- Parser inputs (archive manifests, TOML snippets, glob patterns) are attacker-controlled surfaces that require fuzz coverage.
- Domain logic must be testable without filesystem, network, or async runtime dependencies.
- Coverage targets must be enforceable in CI without manual developer tracking.

## Considered Options

1. Unit tests per crate + proptest + cargo-fuzz + cucumber-rs + schemars contract validation + cargo-tarpaulin coverage.
2. Unit tests only, with no property-based or fuzz testing.
3. Integration-test-heavy approach with minimal unit coverage.
4. External test suite (Python/pytest driving the MCP STDIO interface).

## Decision Outcome

Chosen option: "layered test pyramid with proptest, cargo-fuzz, cucumber-rs, schemars, and cargo-tarpaulin", because each layer targets a distinct failure class that the others cannot catch, and the Rust toolchain supports all layers natively without external test runners.

### Consequences

#### Positive

- **Unit tests** live in each crate's `src/` module tree (`#[cfg(test)]`). The `domain` crate has zero external dependencies, so unit tests run without filesystem or tokio. All domain logic is pure-function testable.
- **Integration tests** live in `crates/substrate-mcp-server/tests/`. They start a substrate process over STDIO, exchange MCP JSON-RPC messages, and assert on tool responses. Each test is isolated via a temporary directory jail.
- **proptest** targets path jail invariants and error code stability. Strategies generate: symlink chains, `..` traversal sequences, Unicode filenames, deeply nested paths, and concurrent tool call orderings. Every generated input must either succeed within the jail or return `ERR_PATH_ESCAPE` — no panics, no silent truncation.
- **cargo-fuzz** (libFuzzer) runs on parser entry points: archive manifest parsers, glob pattern compilers, TOML config fragments, and MCP JSON-RPC frame deserialisers. Fuzz targets are defined under `fuzz/src/`.
- **schemars** derives `JsonSchema` on all tool output types. A contract test serialises the derived schema and compares it against a committed golden file (`tests/schemas/`). Schema changes require an explicit golden update, making drift visible in PRs.
- **cucumber-rs** runs Gherkin feature files under `tests/features/`. Each `.feature` file covers a user-facing capability (e.g., `file_read.feature`, `archive_extract.feature`). This ensures the tool surface behaves as specified from an agent's perspective.
- **cargo-tarpaulin** enforces coverage gates: 80% line coverage for the `domain` crate, 70% for adapter crates. CI fails if either gate is not met. Coverage is reported as a CI artifact.

#### Negative

- cargo-fuzz requires a nightly toolchain for the fuzz runner; CI must maintain a separate nightly job alongside the stable build.
- cucumber-rs step definitions can become verbose; maintainers must discipline themselves to keep feature files readable by non-Rust contributors.
- cargo-tarpaulin is slower than standard `cargo test`; coverage jobs run in a separate CI stage to avoid blocking the primary test pipeline.
- Golden schema files add a maintenance burden when intentional output type changes are made.

## Validation

- CI pipeline: `cargo test --workspace` must pass on stable Rust.
- CI pipeline: `cargo tarpaulin --workspace` must meet domain 80% / adapter 70% gates.
- CI pipeline: `cargo fuzz run <target> -- -max_total_time=60` runs on each fuzz target for 60 seconds in CI (extended runs scheduled weekly).
- CI pipeline: `cargo test --test cucumber` runs all Gherkin scenarios.
- Schema golden comparison runs as part of `cargo test --test contract`.

## Cross-References

- ADR-0023 — CI/CD pipeline (test stage ordering and artifact publication)
- ADR-0029 — Tool error codes (error code stability is a proptest invariant)
