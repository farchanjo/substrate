---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0036 — Startup Error Contract (Pre-MCP-Session Failures)

## Context and Problem Statement

Substrate fails before the MCP `initialize` handshake in several well-defined scenarios: configuration file absent or malformed, allowlist root directory missing or unreadable, mise toolchain corrupt, file-descriptor limit too low, platform unsupported. In these cases the child process exits with a non-zero code and produces no structured output. The MCP host receives a dead process and cannot distinguish "substrate crashed" from "substrate exited cleanly after config validation" or "no such binary on PATH". Operators and LLM agents have no machine-readable signal to recover from or report on.

The MCP protocol error model (JSON-RPC over stdio) is inaccessible before the session is established. A different structured channel is required.

## Decision Drivers

- MCP hosts must detect and surface startup failures without parsing free-text stderr.
- Operators must be able to grep or parse structured output in CI/CD pipelines and log aggregators.
- The envelope must be self-describing so any consumer can process it without out-of-band schema distribution.
- Exit codes must follow well-known conventions (`sysexits.h`) so process supervisors can classify failures.
- Correlation IDs must link the envelope to any server-side log lines emitted before exit.

## Considered Options

1. Exit with a non-zero code and a human-readable message on stderr — simple, not machine-readable.
2. Write a structured JSON envelope to stderr before exit — machine-readable without requiring a live MCP session.
3. Write the envelope to stdout — conflicts with the STDIO MCP transport framing (stdout carries JSON-RPC frames).
4. Expose a pre-flight HTTP endpoint — introduces a network dependency before startup, impractical for STDIO-only deployments.

## Decision Outcome

Chosen option: "Write a structured JSON envelope to stderr before exit", because stderr is the conventional out-of-band channel for diagnostics and is not used by the STDIO MCP transport.

### Startup Error Envelope

Before any non-zero exit, substrate MUST emit exactly one line to stderr with the following JSON object, terminated by a newline (`\n`). No other JSON frames appear on stderr during the startup phase.

```json
{
  "$schema": "substrate-startup-error/v1",
  "code": "<STARTUP_ERROR_CODE>",
  "message_en_us": "<human-readable description>",
  "recovery_hint": "<actionable guidance, ≤ 150 characters>",
  "correlation_id": "<UUIDv7>",
  "timestamp": "<ISO 8601 UTC, e.g. 2026-05-21T14:00:00.000Z>",
  "details": {}
}
```

#### Field Definitions

| Field | Type | Required | Notes |
|---|---|---|---|
| `$schema` | `string` | yes | Fixed value `"substrate-startup-error/v1"`. Consumers grep this to identify the envelope. |
| `code` | `string` | yes | One of the startup error codes listed below. |
| `message_en_us` | `string` | yes | Human-readable description. May include config path or other non-security-sensitive context. |
| `recovery_hint` | `string` | yes | Actionable guidance ≤ 150 characters. |
| `correlation_id` | `string` | yes | UUIDv7. Matches any log lines emitted before exit. |
| `timestamp` | `string` | yes | ISO 8601 UTC (`Z` suffix). |
| `details` | `object` | yes (may be `{}`) | Freeform key-value pairs providing machine-readable context. |

#### Startup Error Codes

| Code | Meaning | Example `details` keys |
|---|---|---|
| `SUBSTRATE_CONFIG_INVALID` | Configuration file present but fails parsing or schema validation. | `config_path`, `parse_error_line`, `parse_error_message` |
| `SUBSTRATE_CONFIG_NOT_FOUND` | No configuration file found at expected path(s). | `searched_paths` (array of strings) |
| `SUBSTRATE_ALLOWLIST_ROOT_MISSING` | A configured allowlist root directory does not exist. | `allowlist_root`, `config_path` |
| `SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE` | Allowlist root exists but substrate cannot read it (EACCES). | `allowlist_root`, `errno` |
| `SUBSTRATE_RUNTIME_INIT_FAILED` | Internal runtime initialization failed (e.g., mise toolchain corrupt, Tokio runtime build error). | `component`, `cause` |
| `SUBSTRATE_FD_LIMIT_TOO_LOW` | System or process fd limit is below the minimum required for safe operation. | `current_limit`, `required_minimum` |
| `SUBSTRATE_UNSUPPORTED_PLATFORM` | Running on an unsupported OS or architecture. | `os`, `arch`, `supported` (array) |

#### Example Envelope

```json
{
  "$schema": "substrate-startup-error/v1",
  "code": "SUBSTRATE_CONFIG_INVALID",
  "message_en_us": "Configuration file at /etc/substrate/config.toml failed validation.",
  "recovery_hint": "check parse_error_line in details and fix the TOML syntax",
  "correlation_id": "01HZABCDEF1234567890ABCDEF",
  "timestamp": "2026-05-21T14:00:00.000Z",
  "details": {
    "config_path": "/etc/substrate/config.toml",
    "parse_error_line": 12,
    "parse_error_message": "expected '=' after key at line 12, column 8"
  }
}
```

### Emit Sequence

1. Substrate detects the startup failure condition.
2. Substrate emits the JSON envelope (single line) to stderr and flushes stderr.
3. Substrate exits with the appropriate non-zero exit code (see table below).

No MCP frames are written to stdout. No partial JSON-RPC initialization begins.

### Exit Code Mapping

Codes follow `sysexits.h` conventions to maximize supervisor/shell compatibility:

| Code | Exit code | `sysexits.h` constant | Rationale |
|---|---|---|---|
| `SUBSTRATE_CONFIG_INVALID` | 78 | `EX_CONFIG` | Configuration error |
| `SUBSTRATE_CONFIG_NOT_FOUND` | 78 | `EX_CONFIG` | Configuration error |
| `SUBSTRATE_ALLOWLIST_ROOT_MISSING` | 77 | `EX_NOPERM` | Cannot access configured resource |
| `SUBSTRATE_ALLOWLIST_ROOT_UNREADABLE` | 77 | `EX_NOPERM` | Cannot access configured resource |
| `SUBSTRATE_RUNTIME_INIT_FAILED` | 70 | `EX_SOFTWARE` | Internal software error |
| `SUBSTRATE_FD_LIMIT_TOO_LOW` | 69 | `EX_UNAVAILABLE` | Required resource unavailable |
| `SUBSTRATE_UNSUPPORTED_PLATFORM` | 68 | `EX_USAGE` | Invoked in unsupported environment |

### Host-Side Detection

MCP hosts and operators detect startup failures by:

1. Monitoring the child process for unexpected exit before `initialize` completes.
2. Reading stderr and grepping for the literal string `"$schema":"substrate-startup-error/v1"`.
3. Parsing the JSON envelope to extract `code`, `recovery_hint`, and `details` for display or automated triage.

### Consequences

#### Positive

- LLM-driven MCP hosts can surface a structured failure message rather than "server disconnected unexpectedly".
- CI/CD pipelines can distinguish configuration errors (exit 78) from permission errors (exit 77) without parsing prose.
- `correlation_id` links the envelope to log lines emitted during the failed startup sequence.
- Schema string `"substrate-startup-error/v1"` allows versioned evolution without breaking existing consumers.

#### Negative

- Substrate must flush stderr before calling `process::exit`; failing to flush silently drops the envelope in some runtimes.
- Consumers must handle the case where stderr contains both the envelope and additional human-readable log lines emitted before the envelope.
- Test harnesses must capture stderr to verify envelope emission.

## Validation

- Integration tests spawn substrate with deliberately broken configurations and assert:
  - Exactly one line matching `"$schema":"substrate-startup-error/v1"` appears on stderr.
  - The `code` field matches the expected startup error code.
  - `recovery_hint` length ≤ 150 characters.
  - Exit code matches the mapping table.
- Unit tests assert the envelope serializes as a single line (no embedded newlines in field values).
- CUE schema in `docs/arch/schemas/error_catalog.cue` validates all seven startup code literals and the `recovery_hint` length constraint.

## Links

- [ADR-0004](0004-security-model.md) — security allowlist (context for ALLOWLIST_ROOT_* codes)
- [ADR-0010](0010-error-taxonomy.md) — error taxonomy (runtime error codes, correlation ID convention)
- [ADR-0011](0011-configuration-management.md) — configuration (config file location and schema)
- [ADR-0034](0034-kernel-induced-error-codes.md) — kernel-induced error codes (errno mapping for runtime errors)
