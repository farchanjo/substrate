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
