---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0018 — Logging Redaction Policy

## Context and Problem Statement

Substrate writes a structured audit log of every tool invocation, including path arguments, process identifiers, and operation outcomes. LLM-generated inputs may contain secrets (API keys, tokens, PEM blocks) embedded in file paths, archive member names, or argument strings. Unreacted log entries would expose these secrets in plaintext. Additionally, path arguments containing CRLF sequences (`\r\n`) can be used to inject synthetic log lines (log injection attack). The audit log must be trustworthy, tamper-resistant at the content level, and free of secrets.

## Decision Drivers

- Secrets embedded in tool arguments must not appear in audit log output.
- CRLF injection via path or argument strings must be neutralized before logging.
- Structured logging is required to enable downstream log parsing and alerting without fragile regex over freeform text.
- A `SecretString` type must prevent accidental logging through the standard `Debug`/`Display` trait path.
- stdout is sacred (JSON-RPC channel); all log output goes to stderr.

## Considered Options

1. Pre-emit redaction pipeline with `SecretString` type + structured logging (selected)
2. Post-hoc log scrubbing via external agent (e.g., log shipper with regex filter)
3. No redaction; rely on filesystem permissions to protect log files
4. Hash-based pseudonymization of all argument values

## Decision Outcome

Chosen option: "Pre-emit redaction pipeline with `SecretString` type + structured logging", because secrets must be eliminated before bytes reach the log sink, not after. Post-hoc scrubbing has a time window of exposure and is fragile under log shipping pipelines.

### Redaction patterns

The following patterns are matched against every loggable string field before emission. Matches are replaced with the literal `[REDACTED]`.

| Pattern family | Match examples |
|---|---|
| AWS access key | `AKIA[0-9A-Z]{16}` |
| AWS secret key | 40-char alphanumeric following `aws_secret` or positional heuristic |
| GitHub token | `ghp_[A-Za-z0-9]{36}`, `ghs_[A-Za-z0-9]{36}`, `github_pat_[A-Za-z0-9_]{82}` |
| Generic token/secret/key/credential/password | field names matching `(?i)(password|secret|token|key|credential)` with any value |
| PEM block headers | `-----BEGIN [A-Z ]+-----` through `-----END [A-Z ]+-----` |
| Bearer/Basic auth | `(?i)bearer [A-Za-z0-9._\-]+`, `(?i)basic [A-Za-z0-9+/=]+` |

Redaction is applied as a composable pipeline: each pattern is a stateless function operating on `&str` and returning `Cow<str>`. The pipeline is applied once per loggable field, in order, before the structured log event is serialized.

### CRLF stripping (audit injection prevention)

All string fields are passed through a CRLF normalizer before the redaction pipeline:

- `\r\n` (CRLF) is replaced with `\n`.
- Bare `\r` is replaced with `\n`.
- Null bytes (`\0`) are replaced with `<NUL>`.

This prevents an adversarial path argument such as `/tmp/foo\r\nINFO fake-event: ...` from injecting a synthetic log line. The normalizer runs before field serialization; the original argument value is never written to the log sink.

### SecretString type

A `SecretString` newtype wraps `String` with the following trait implementations:

- `Display` emits `[REDACTED]` unconditionally.
- `Debug` emits `SecretString([REDACTED])` unconditionally.
- `serde::Serialize` emits the string `"[REDACTED]"` unconditionally.
- Deserialization (`serde::Deserialize`) is implemented normally to allow reading config values.
- Zeroize-on-drop is applied to the inner `String`.

Fields that carry credentials (e.g., hypothetical future `fs.write` content matching a secret pattern) are typed as `SecretString` in the tool argument structs. This makes accidental logging a compile-time error rather than a runtime leak.

### Structured logging only

All audit events are emitted as JSON objects to stderr via `tracing` with the `tracing-subscriber` JSON formatter. Freeform `eprintln!` is forbidden in production code paths. Log fields are typed; the schema is stable across patch releases.

Mandatory fields per audit event:

| Field | Type | Description |
|---|---|---|
| `timestamp` | RFC 3339 | Event time (UTC) |
| `level` | string | `TRACE`/`DEBUG`/`INFO`/`WARN`/`ERROR` |
| `tool` | string | MCP tool name (e.g., `fs.remove`) |
| `session_id` | UUID | MCP session identifier |
| `outcome` | string | `ok`/`dry_run`/`denied`/`elicited`/`error` |
| `target_path` | string | Redacted, CRLF-stripped path argument |
| `duration_ms` | u64 | Wall time of tool execution |

### Consequences

#### Positive

- Secrets in LLM-generated arguments cannot reach the log sink.
- CRLF injection is neutralized structurally, not by convention.
- `SecretString` makes accidental logging of credentials a compile error.
- Structured JSON log schema enables downstream SIEM ingestion without fragile parsing.

#### Negative

- Redaction pipeline adds CPU cost per log event (acceptable for audit-frequency operations).
- Overly broad generic patterns may redact non-secret values sharing naming conventions.
- `SecretString` requires opt-in typing discipline; missed fields are not caught automatically.

## Validation

- Unit tests assert that each redaction pattern replaces known secret fixtures with `[REDACTED]`.
- CRLF injection test vectors confirm multi-line payloads produce single normalized log lines.
- `SecretString` compile-time tests confirm `format!("{:?}", secret)` emits `[REDACTED]`.
- Log output integration tests parse stderr as NDJSON and assert schema conformance.

## Cross-references

- ADR-0009 — Audit log retention and storage (where logs go, rotation policy)
- ADR-0029 — Threat model (audit log injection listed as threat T-5; CRLF stripping is the mitigation)
