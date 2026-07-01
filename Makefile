# substrate -- local install target per ADR-0045 (Makefile port; see amendment).
#
# Run `make help` to list targets. Everything else (build/test/clippy/ci
# mirrors) still lives in the justfile -- this Makefile only owns install.

.PHONY: help build-release install uninstall verify-install

help:
	@echo "make build-release  - build the optimised release binary"
	@echo "make install        - build + install to /usr/local/bin (codesign on macOS)"
	@echo "make uninstall      - remove /usr/local/bin/substrate"
	@echo "make verify-install - inspect the installed binary"

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
