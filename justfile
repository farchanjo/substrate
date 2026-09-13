# substrate -- developer workflow targets per ADR-0045.
#
# Run `just --list` to enumerate targets.

# Default: list available recipes.
default:
    @just --list

# Build the optimised release binary (LTO + panic=abort + strip per ADR-0014).
# `--features subprocess` is required for the subprocess.* MCP tool surface;
# omitting it ships a binary missing subprocess_spawn/list/cancel/result/signal/search.
build-release:
    cargo build --workspace --release --bin substrate --features substrate-mcp-server/subprocess

# Build everything (workspace, all targets) in the dev profile. This is the
# day-to-day path: unoptimized, incremental, sccache-wrapped. `build-release`
# is for deploy artifacts only.
build:
    cargo build --workspace --all-targets

# Fast test loop. nextest parallelizes across binaries; the cucumber harness is
# excluded because its clap CLI rejects the `--list` probe nextest uses to
# enumerate tests, so it runs on cargo's harness instead. Doctests follow for the
# same reason: nextest does not run them.
test:
    cargo nextest run --workspace -E 'not binary(cucumber)' --no-fail-fast
    cargo test -p substrate-mcp-server --test cucumber
    cargo test --workspace --doc

# Serial fallback for when interleaved output matters more than speed.
test-serial:
    cargo test --workspace --no-fail-fast -- --test-threads=1

# Clippy gate at -D warnings.
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# rustfmt + ruff-style format check.
fmt-check:
    cargo fmt --all --check

# Spec validate, deep lane (native + external validators).
spec-validate:
    speckit validate --deep

# Validate every Mermaid block in every .md by piping each to mmdc (mermaid-cli).
# Requires `mmdc` and `perl` on PATH. Per ADR-0047.
lint-mermaid:
    ./scripts/lint-mermaid.sh --keep-going

# Install / uninstall / verify-install moved to the Makefile (`make install`,
# `make uninstall`, `make verify-install`) -- see ADR-0045 amendment.

# ---------------------------------------------------------------------------
# CI mirror recipes — reproduce each CI gate locally.
# All cargo invocations use --locked to match CI behaviour.
# ---------------------------------------------------------------------------

# Check formatting (mirrors CI job: fmt).
ci-fmt:
    cargo fmt --all -- --check

# Clippy lint at -D warnings (mirrors CI job: clippy).
ci-clippy:
    cargo clippy --locked --workspace --all-targets -- -D warnings

# Run tests via cargo-nextest (mirrors CI job: nextest). The cucumber harness is
# excluded from the nextest pass — see `test` for why — and run on its own.
ci-nextest:
    cargo nextest run --locked --workspace -E 'not binary(cucumber)' --no-fail-fast
    cargo test --locked -p substrate-mcp-server --test cucumber
    cargo test --locked --workspace --doc

# Run tests with the subprocess feature enabled (mirrors CI job: nextest-subprocess).
# Required to exercise subprocess.* tools and their cucumber integration scenarios.
ci-nextest-subprocess:
    cargo nextest run --locked -p substrate-mcp-server --features subprocess --no-fail-fast

# Dependency advisories + license + source check (mirrors CI job: deny).
ci-deny:
    cargo deny --locked check

# Vulnerability advisory scan (mirrors CI job: audit).
ci-audit:
    cargo audit --deny warnings

# Public API semver regression check (mirrors CI job: semver-checks).
ci-semver:
    cargo semver-checks check-release --locked --workspace || true

# Line coverage with 80 % threshold + lcov report (mirrors CI job: llvm-cov).
ci-coverage:
    cargo llvm-cov --locked --workspace --fail-under-lines 80 --lcov --output-path lcov.info

# Compile benchmarks without running them (mirrors CI job: bench).
ci-bench:
    cargo bench --locked --workspace --no-run

# Spec deep-lane validation (mirrors CI job: spec-validate).
ci-spec:
    speckit validate --deep

# Mermaid diagram lint via mmdc (alias for lint-mermaid).
ci-mermaid:
    ./scripts/lint-mermaid.sh --keep-going

# Spell-check source and docs (mirrors CI job: typos).
ci-typos:
    typos

# Detect unused workspace dependencies (mirrors CI job: cargo-shear).
ci-shear:
    cargo shear

# Run all CI gates sequentially (full local CI run).
ci: ci-fmt ci-clippy ci-nextest ci-nextest-subprocess ci-deny ci-audit ci-semver ci-coverage ci-bench ci-spec ci-mermaid ci-typos ci-shear

# ---------------------------------------------------------------------------
