---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0011 — Configuration Management

## Context and Problem Statement

substrate must be configurable across security policy, runtime tuning, logging verbosity, and MCP protocol parameters. Configuration must support multiple sources (file, environment, CLI flags) with a clear precedence order, and must fail loudly on invalid values rather than silently using defaults that could weaken security invariants.

## Decision Drivers

- Security settings (allowlist, jail path, dry-run flag) must never be silently ignored or defaulted to permissive values.
- Operators deploy substrate in diverse environments (macOS, Linux, containers); the configuration path must respect platform conventions.
- Hot reload is not required for MVP and introduces complexity around partial-config inconsistency and jail-path race conditions.
- CLI flags must be usable for ephemeral overrides in testing without editing the TOML file.
- The configuration schema must be self-documenting and serialisable for tooling (e.g., schema export, `--print-config`).

## Considered Options

1. `figment` with layered providers: TOML file → environment variables → CLI flags.
2. `config-rs` with a similar layered approach.
3. Hand-rolled TOML parsing with `toml` crate and manual env override logic.
4. Environment-variables-only (12-factor style).

## Decision Outcome

Chosen option: "figment with TOML → env → CLI provider stack", because figment provides a composable provider model, strong type extraction via `serde`, first-class error messages with source attribution, and is already used in the Rust ecosystem for complex configuration layering without macro magic.

### Consequences

#### Positive

- Precedence is explicit and documented: CLI flags override env vars, which override TOML file, which overrides compiled-in defaults.
- Default configuration file path follows XDG on Linux/Windows (`$XDG_CONFIG_HOME/substrate/config.toml`) and the Apple convention on macOS (`~/Library/Application Support/substrate/config.toml`). The `dirs` crate resolves the platform-correct path at startup.
- Schema is partitioned into four top-level TOML tables for separation of concerns:
  - `[security]` — allowlist path, jail root, dry-run default, elicitation policy.
  - `[runtime]` — tokio thread count, global timeout (default 30 s), per-tool timeout overrides, memory caps (see ADR-0016).
  - `[logging]` — log level filter, structured output toggle, redaction rules reference.
  - `[protocol]` — rmcp version, MCP capability advertisement, tool registration policy.
- Fail-fast on startup: if figment extraction fails (missing required field, type mismatch, constraint violation), the process logs the error to stderr and exits with code 78 (`EX_CONFIG`).
- No hot reload: configuration is loaded once at process start and held immutably in an `Arc<Config>` shared across async zones. Operators must restart the process to pick up changes.

#### Negative

- No hot reload means security policy changes require a process restart, which may interrupt in-flight MCP sessions.
- figment's error messages, while informative, use its own error type hierarchy; mapping to substrate's error codes (ADR-0010) requires a thin adapter.
- TOML table structure must remain stable across versions; breaking changes require a migration guide and a new ADR.

## Validation

- Unit tests exercise each figment provider layer in isolation (file-only, env-only, CLI-only, merged).
- Integration tests start substrate with a minimal valid config and assert `--print-config` output round-trips through deserialization.
- A test asserts that startup with an invalid `[security].jail_root` (non-existent path) exits with code 78 within 1 second.
- CI runs `cargo check --features strict-config` which enables additional compile-time constraint validation.

## Strict Config Schema

All configuration structs derived from `serde::Deserialize` MUST be annotated with `#[serde(deny_unknown_fields)]`. A TOML key that does not correspond to any known struct field (including a typo such as `alllowlist` instead of `allowlist`) causes figment extraction to fail at startup with exit code 78. The error message emitted to stderr includes the unknown field path (e.g., `[security].alllowlist`) so the operator can locate the typo without inspecting the full schema. The error code emitted to the MCP channel (if the runtime is already initialised) is `SUBSTRATE_CONFIG_INVALID` (see [ADR-0036](0036-startup-error-contract.md)).

## Allowlist Canonicalization at Startup

Every entry in `[security].roots` is resolved via `std::fs::canonicalize` at config-load time, before the MCP runtime accepts any connections. Canonicalization expands symlinks and resolves `.` and `..` components, yielding an absolute, symlink-free path.

Entries that fail canonicalization produce the following startup errors:

- `SUBSTRATE_ALLOWLIST_ROOT_MISSING` — `canonicalize` returned `ErrorKind::NotFound`; the configured path does not exist on the filesystem.
- `SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE` — `canonicalize` returned `ErrorKind::PermissionDenied`; the path exists but substrate cannot read it.

Both errors are fatal at startup (exit code 78). The canonical paths are additionally normalised to Unicode NFC form (see [ADR-0035](0035-path-safety-hardening.md)) before being stored in `Arc<Config>` to ensure consistent prefix matching regardless of the NFC/NFD normalisation of tool arguments.

## Empty Allowlist Semantics

`roots = []` under `[security]` (or an absent `[security]` table entirely) means DENY-ALL: every path-bearing tool call will immediately return `PathOutsideAllowlist` without performing any filesystem access. This is the intentional semantics for a sandboxed deployment where no filesystem access is permitted.

substrate still starts successfully in this configuration; it does not fail-fast or exit. At startup, a WARN log is emitted: `{"level":"warn","msg":"no allowlist roots configured; all path-bearing tools will reject"}`. This distinguishes a deliberate DENY-ALL configuration from a misconfiguration (which would typically include at least one root).

## Allowlist Symlink Policy

A configured root that is itself a symlink (e.g., `/home/user/project -> /data/project`) is canonicalized to its target path. The canonical form is what is stored in `Arc<Config>` and used for all subsequent prefix checks.

If any configured root canonicalizes to a path different from its literal configured value (indicating that a symlink was resolved), substrate emits a WARN log at startup listing both the configured path and the resolved canonical path. This allows operators to audit whether the symlink target is within the intended boundary, because the canonical prefix is wider than the symbolic name suggests when the symlink points outside the operator's expected subtree.

## Shutdown Drain

The `[runtime]` table accepts a `shutdown_drain_secs: u32` field (default `5`). This value specifies the maximum number of seconds that substrate waits for in-flight tool calls to complete after receiving a termination signal before forcing a hard shutdown. It is consumed by the signal handler described in [ADR-0032](0032-signal-safety.md) and interacts with the concurrency limit enforcement in [ADR-0017](0017-concurrency-limits.md).

## Cross-References

- ADR-0004 — Security model (defines the fields under `[security]`)
- ADR-0009 — Observability (log level and structured output controlled via `[logging]`)
- ADR-0017 — Timeout and cancellation model (timeout values sourced from `[runtime]`)
- [ADR-0035](0035-path-safety-hardening.md) — Path safety hardening (Unicode NFC normalization of allowlist roots)
- [ADR-0036](0036-startup-error-contract.md) — Startup error contract (`SUBSTRATE_CONFIG_INVALID`, `SUBSTRATE_ALLOWLIST_ROOT_MISSING`, `SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE` error codes)
