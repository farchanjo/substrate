<!--
Thanks for contributing.

Title format (per ADR-0024): <type>(<scope>): <subject>
  type: feat | fix | docs | refactor | test | build | ci | chore | perf | style | security
  scope: a crate name (fs-query, process, mcp-server, ...) or "adr"
-->

## Summary

<!-- One paragraph: what changes and why. Reference the issue if any (Closes #N). -->

## Spec / ADR references

<!--
List the ADR(s) and Gherkin feature(s) this PR implements or amends.
  - ADR-NNNN — short title
  - features/<bc>/<file>.feature
If this PR amends a locked architectural decision, link the superseding ADR.
-->

## Type of change

- [ ] Bug fix (no API change)
- [ ] New feature (backwards compatible)
- [ ] Breaking change (bumps major or pre-1.0 minor)
- [ ] Documentation / spec only
- [ ] CI / build / tooling
- [ ] Refactor (no behavior change)
- [ ] Performance (include benchmark numbers)
- [ ] Security

## Test plan

<!--
List the validation steps a reviewer can reproduce locally.
  - [ ] cargo fmt --all -- --check
  - [ ] cargo clippy --workspace --all-targets -- -D warnings
  - [ ] cargo nextest run --workspace --no-fail-fast
  - [ ] spec validate --lane fast (or --lane full for spec/ADR changes)
  - [ ] Manual: ...
-->

## Checklist

- [ ] Commits follow the Angular format and are signed off with `Signed-off-by:` (DCO).
- [ ] Commits are small and contextual (no bulk "various changes").
- [ ] No edits to `clippy.toml`, `Cargo.toml [lints.*]`, `rust-toolchain.toml`, or `#![forbid/deny(...)]` without explicit ADR justification.
- [ ] No `println!` / `print!` in `crates/*/src/` (stdout is the JSON-RPC channel).
- [ ] `docs/` is in sync with the code change.
- [ ] If a new tool is added: ADR + Gherkin + CUE schema + audit-event entry are in place.
- [ ] If a new error code is added: it follows the `SUBSTRATE_<UPPER_SNAKE>` form and updates [ADR-0010](../docs/arch/adr/0010-error-taxonomy.md).
- [ ] No use of `std::process::Command` or `tokio::process::Command` in shipping crates (see [ADR-0044](../docs/arch/adr/0044-no-subprocess-policy.md)).

## Risk and rollout

<!--
- Reversibility (easy / moderate / hard)
- Blast radius (one tool / one bounded context / cross-cutting)
- Migration steps for downstream operators, if any
-->

## Additional context

<!-- Screenshots, benchmark numbers, links to discussion threads, design sketches. -->
