# substrate -- developer workflow targets per ADR-0045.
#
# Run `just --list` to enumerate targets.

# Default: list available recipes.
default:
    @just --list

# Build the optimised release binary (LTO + panic=abort + strip per ADR-0014).
build-release:
    cargo build --workspace --release --bin substrate

# Build everything (workspace, all targets) in dev profile.
build:
    cargo build --workspace --all-targets

# Run all unit and integration tests.
test:
    cargo test --workspace --no-fail-fast -- --test-threads=1

# Clippy gate at -D warnings.
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# rustfmt + ruff-style format check.
fmt-check:
    cargo fmt --all --check

# Spec validate full lane.
spec-validate:
    spec validate --lane full

# Validate every Mermaid block in every .md by piping each to mmdc (mermaid-cli).
# Requires `mmdc` and `perl` on PATH. Per ADR-0047.
lint-mermaid:
    ./scripts/lint-mermaid.sh --keep-going

# Install to /usr/local/bin with codesign on macOS, plain install on Linux.
# Per ADR-0045 -- signs source AND destination on macOS. Set
# SUBSTRATE_SIGN_IDENTITY to a Developer ID for Gatekeeper-trusted builds;
# defaults to ad-hoc ("-") which is local-only.
install: build-release
    @if [ "$(uname -s)" = "Darwin" ]; then just _install-macos; else just _install-linux; fi

_install-macos: build-release
    codesign --options runtime --timestamp -f -s "${SUBSTRATE_SIGN_IDENTITY:--}" target/release/substrate
    sudo install -m 0755 target/release/substrate /usr/local/bin/substrate
    sudo codesign --options runtime --timestamp -f -s "${SUBSTRATE_SIGN_IDENTITY:--}" /usr/local/bin/substrate
    codesign --verify --strict /usr/local/bin/substrate
    @echo "installed and signed at /usr/local/bin/substrate"

_install-linux: build-release
    sudo install -m 0755 target/release/substrate /usr/local/bin/substrate
    @echo "installed at /usr/local/bin/substrate"

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

# Run tests via cargo-nextest (mirrors CI job: nextest).
ci-nextest:
    cargo nextest run --locked --workspace --no-fail-fast

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

# Spec full-lane validation (mirrors CI job: spec-validate).
ci-spec:
    spec validate --lane full

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
ci: ci-fmt ci-clippy ci-nextest ci-deny ci-audit ci-semver ci-coverage ci-bench ci-spec ci-mermaid ci-typos ci-shear

# ---------------------------------------------------------------------------

# Uninstall from /usr/local/bin.
uninstall:
    sudo rm -f /usr/local/bin/substrate
    @echo "removed /usr/local/bin/substrate"

# Inspect the installed binary's signature (macOS only).
verify-install:
    @if [ "$(uname -s)" = "Darwin" ]; then \
        codesign --display --verbose=4 /usr/local/bin/substrate; \
        codesign --verify --strict /usr/local/bin/substrate; \
    else \
        file /usr/local/bin/substrate; \
        /usr/local/bin/substrate --help 2>/dev/null || true; \
    fi
