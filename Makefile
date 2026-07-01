# substrate -- developer workflow targets (Makefile). See ADR-0045 amendment:
# install/uninstall/verify-install moved here from the justfile; the rest of
# the workflow is mirrored here too so `make` alone covers everything the
# justfile does. Run `make help` to list targets.

.PHONY: help build build-release test lint fmt-check spec-validate lint-mermaid \
	install uninstall verify-install _install-macos _install-linux \
	ci ci-fmt ci-lint ci-test ci-test-subprocess ci-deny ci-audit ci-semver \
	ci-coverage ci-bench ci-spec ci-mermaid ci-typos ci-shear

help:
	@echo "make build           - build everything (workspace, all targets) in dev profile"
	@echo "make build-release   - build the optimised release binary"
	@echo "make test            - run all unit + integration tests"
	@echo "make lint            - clippy gate at -D warnings"
	@echo "make fmt-check       - rustfmt check"
	@echo "make spec-validate   - spec validate --lane full"
	@echo "make lint-mermaid    - validate every Mermaid block in every .md"
	@echo "make install         - build + install to /usr/local/bin (codesign on macOS)"
	@echo "make uninstall       - remove /usr/local/bin/substrate"
	@echo "make verify-install  - inspect the installed binary"
	@echo "make ci              - run all CI gate mirrors sequentially"
	@echo "make ci-<gate>       - run a single CI gate mirror (fmt/lint/test/test-subprocess/deny/audit/semver/coverage/bench/spec/mermaid/typos/shear)"

# ---------------------------------------------------------------------------
# Dev workflow
# ---------------------------------------------------------------------------

# Build everything (workspace, all targets) in dev profile.
build:
	cargo build --workspace --all-targets

# Run all unit and integration tests (dev convenience -- single-threaded for
# output clarity). CI uses `cargo nextest run --locked` (see `make ci-test`).
test:
	cargo test --workspace --no-fail-fast -- --test-threads=1

# Clippy gate at -D warnings.
lint:
	cargo clippy --workspace --all-targets -- -D warnings

# rustfmt check.
fmt-check:
	cargo fmt --all --check

# Spec validate full lane.
spec-validate:
	spec validate --lane full

# Validate every Mermaid block in every .md by piping each to mmdc
# (mermaid-cli). Requires `mmdc` and `perl` on PATH. Per ADR-0047.
lint-mermaid:
	./scripts/lint-mermaid.sh --keep-going

# ---------------------------------------------------------------------------
# Install (ADR-0045)
# ---------------------------------------------------------------------------

# Build the optimised release binary (LTO + panic=abort + strip per ADR-0014).
# --features substrate-mcp-server/subprocess is required for the subprocess.*
# MCP tool surface; omitting it ships a binary missing subprocess_spawn/list/
# cancel/result/signal/search.
build-release:
	cargo build --workspace --release --bin substrate --features substrate-mcp-server/subprocess

# Install to /usr/local/bin with codesign on macOS, plain install on Linux.
# Signs source AND destination on macOS (ADR-0045 "Signing rationale").
# SUBSTRATE_SIGN_IDENTITY selects the codesign identity; defaults to the
# ad-hoc identity ("-"), which is local-only and not Gatekeeper-trusted.
install: build-release
	@if [ "$$(uname -s)" = "Darwin" ]; then \
		$(MAKE) _install-macos; \
	else \
		$(MAKE) _install-linux; \
	fi

_install-macos:
	codesign --options runtime --timestamp -f -s "$${SUBSTRATE_SIGN_IDENTITY:--}" target/release/substrate
	sudo install -m 0755 target/release/substrate /usr/local/bin/substrate
	sudo codesign --options runtime --timestamp -f -s "$${SUBSTRATE_SIGN_IDENTITY:--}" /usr/local/bin/substrate
	codesign --verify --strict /usr/local/bin/substrate
	@echo "installed and signed at /usr/local/bin/substrate"

_install-linux:
	sudo install -m 0755 target/release/substrate /usr/local/bin/substrate
	@echo "installed at /usr/local/bin/substrate"

# Uninstall from /usr/local/bin.
uninstall:
	sudo rm -f /usr/local/bin/substrate
	@echo "removed /usr/local/bin/substrate"

# Inspect the installed binary's signature (macOS only; falls back to
# `file` + a smoke invocation on Linux).
verify-install:
	@if [ "$$(uname -s)" = "Darwin" ]; then \
		codesign --display --verbose=4 /usr/local/bin/substrate; \
		codesign --verify --strict /usr/local/bin/substrate; \
	else \
		file /usr/local/bin/substrate; \
		/usr/local/bin/substrate --help 2>/dev/null || true; \
	fi

# ---------------------------------------------------------------------------
# CI mirror recipes -- reproduce each CI gate locally.
# All cargo invocations use --locked to match CI behaviour.
# ---------------------------------------------------------------------------

# Check formatting (mirrors CI job: fmt).
ci-fmt:
	cargo fmt --all -- --check

# Clippy lint at -D warnings (mirrors CI job: clippy).
ci-lint:
	cargo clippy --locked --workspace --all-targets -- -D warnings

# Run tests via cargo-nextest (mirrors CI job: nextest).
ci-test:
	cargo nextest run --locked --workspace --no-fail-fast

# Run tests with the subprocess feature enabled (mirrors CI job:
# nextest-subprocess). Required to exercise subprocess.* tools and their
# cucumber integration scenarios.
ci-test-subprocess:
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

# Line coverage with 80% threshold + lcov report (mirrors CI job: llvm-cov).
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
ci: ci-fmt ci-lint ci-test ci-test-subprocess ci-deny ci-audit ci-semver ci-coverage ci-bench ci-spec ci-mermaid ci-typos ci-shear
