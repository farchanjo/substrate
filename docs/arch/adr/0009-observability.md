---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0009 — Observability (Tracing, Spans, Audit)

## Context and Problem Statement

substrate is an MCP server executed as a child process by LLM agents over STDIO. Because stdout is the MCP wire channel (sagrado — must not be contaminated), all human-readable diagnostic output must flow exclusively to stderr. The server must produce structured, correlated telemetry to support debugging, audit compliance, and post-incident analysis without coupling to an external observability backend in the MVP.

## Decision Drivers

- STDIO transport reserves stdout for MCP framing; telemetry must use stderr only.
- LLM agents issue concurrent tool calls; tracing must support correlation across overlapping requests.
- Security model (ADR-0004) requires an audit trail for every destructive or privileged operation.
- Minimise operational dependencies for the MVP; defer metrics and distributed tracing ingestion to a later phase.
- Structured logs must be machine-parseable for future pipeline integration.

## Considered Options

1. `tracing` crate with `tracing-subscriber` fmt layer writing to stderr (structured JSON).
2. `log` crate with a custom stderr sink.
3. OpenTelemetry SDK with OTLP exporter.
4. Inline `eprintln!` statements with no structured framework.

## Decision Outcome

Chosen option: "tracing crate with tracing-subscriber writing structured JSON to stderr", because it integrates natively with the async tokio ecosystem, supports hierarchical spans, composes with future OTLP exporters without code changes, and has zero risk of contaminating stdout.

### Consequences

#### Positive

- `tracing` spans map one-to-one to the MCP call hierarchy: MCP-request → tool-call → fs-operation.
- Correlation IDs (UUIDv7) are injected at MCP-request span creation and propagated automatically via `tracing::Span::current()` into all child spans and structured log events.
- Log levels TRACE / DEBUG / INFO / WARN / ERROR follow standard severity semantics; TRACE is suppressed in release builds by default via compile-time `max_level_trace` feature gate.
- Audit log events are emitted at INFO level with a structured `audit=true` field, enabling downstream grep/filter without a separate sink.
- Sensitive fields (file contents, resolved paths under jail, command arguments) are redacted at the tracing instrumentation site per ADR-0018 policy before the event reaches any subscriber.
- Metrics (counters, histograms) are explicitly out of scope for MVP; they can be added later as a `tracing-opentelemetry` bridge layer without touching business logic.

#### Negative

- JSON log lines can be verbose; operators must pipe through `jq` or a log viewer for human-readable output during local development.
- No distributed trace propagation in MVP; correlation is limited to within a single server process lifetime.
- `tracing-subscriber` `EnvFilter` must be configured carefully to avoid leaking redacted data at TRACE level.

## Validation

- All `#[instrument]` spans on public tool entry points must carry a `correlation_id` field sourced from UUIDv7.
- CI enforces that no `println!` macro appears in non-test crates (lint rule via `clippy::print_stdout`).
- Integration tests assert that stderr output parses as valid JSON Lines for representative tool invocations.
- Audit events for destructive operations (delete, overwrite, chmod) are verified present in test output with `audit=true` and without raw path values when path is inside jail.

## Span Status Policy

Each `tracing::Span` wrapping an MCP tool call is closed with an explicit status that maps to the `outcome` field of the terminal audit event (see [ADR-0038](0038-audit-event-semantics.md)):

- `outcome = timeout` → span status **cancelled** (the call exceeded its deadline; the server did not produce a result).
- `outcome = error` → span status **error** (a server-caused or tool-caused error occurred).
- `outcome = success` → span status **success**.
- `outcome = cancelled` → span status **cancelled** (client sent a cancellation notification).
- `outcome = attempted` → span remains open until the terminal event closes it; the pre-execution span status is not set until that point.

These status values are set via `tracing::Span::current().record("otel.status_code", ...)` using the OpenTelemetry status vocabulary so that a future OTLP bridge layer requires no code changes.

## Panic Hook

At process startup, substrate installs a custom panic hook via `std::panic::set_hook` before the tokio runtime is constructed. The hook emits a single JSON line to stderr with the following fields:

```json
{"level":"error","panic":true,"location":"file:line:col","backtrace":"...","message":"..."}
```

The hook fires before the standard panic handler (which writes to stderr in non-JSON format) so that the JSON line is always the first output for the panic event. After the custom hook returns, the default handler is invoked to produce the human-readable backtrace.

`RUST_BACKTRACE=full` is set in the process environment at startup (before spawning any threads) to ensure that backtraces captured in the hook contain full symbol information. If the environment already contains `RUST_BACKTRACE`, the existing value is preserved.

## Non-Blocking Tracing Writer

The `tracing-subscriber` fmt layer uses `tracing_appender::non_blocking(std::io::stderr())` as its writer, with `flush_on_drop = true` to ensure all buffered events are flushed when the guard is dropped at shutdown.

The internal ring buffer is bounded; when the background writer thread cannot drain fast enough, new log events are dropped rather than blocking the async runtime. A `lost_lines: u64` counter is maintained via `AtomicU64` and incremented for each dropped event. On graceful shutdown, if `lost_lines > 0`, substrate emits a single WARN log line: `{"level":"warn","lost_lines":<n>,"msg":"tracing writer dropped events"}`. Operators MUST monitor this field; a non-zero value indicates that the logging back-pressure budget was insufficient and that audit events may be missing from the log.

## WARN vs ERROR Level Policy

The log level for `ToolError` variants follows a user-caused vs server-caused distinction:

**WARN** (user-caused; the client sent an invalid or unauthorized request):

- `PathOutsideAllowlist` — the requested path falls outside all configured roots.
- `PathTraversalBlocked` — a `..` component or symlink escape was detected.
- `SymlinkEscape` — the resolved symlink target leaves the allowlist boundary.
- `PermissionDenied` — OS returned `EACCES` or `EPERM` for an otherwise valid path.
- `NotFound` — the target path does not exist.
- `InvalidArgument` — a tool argument failed schema or semantic validation.
- `DryRunRequired` — the operation requires `--dry-run` confirmation which was not supplied.
- `ConfirmationRequired` — elicitation was required but not confirmed.

**ERROR** (server-caused; indicates a substrate defect or infrastructure failure):

- `InternalError` — an unexpected condition that substrate cannot attribute to the client.
- `IOError` — an unexpected OS I/O error not covered by the user-caused variants above.
- `Timeout` — the tool exceeded its configured deadline (see ADR-0017).

**INFO**:

- `Cancelled` — the client sent an MCP cancellation notification; no server-side fault.

This policy aligns with the principle that alerts should fire on ERROR events, while WARN events are surfaced to operators for trend analysis without paging.

## Audit Event Semantics

Ordering, correlation propagation across `tokio::spawn` and `spawn_blocking` boundaries, the monotonic `seq` counter, and the `active_requests_at_start` concurrency snapshot are specified in [ADR-0038](0038-audit-event-semantics.md). This ADR does not duplicate that specification.

## SLI Definitions

Per-budget measurement specs — numerator/denominator expressions, measurement windows, and CI-only SLIs for cold-start latency and memory usage — are specified in [ADR-0039](0039-sli-definitions.md).

## Cross-References

- ADR-0010 — Error handling strategy (error events feed into tracing spans)
- ADR-0018 — Data redaction policy (defines what fields must be redacted before logging)
- ADR-0020 — Async zone model (span propagation across tokio task boundaries)
- [ADR-0038](0038-audit-event-semantics.md) — Audit event semantics (ordering, correlation, seq counter)
- [ADR-0039](0039-sli-definitions.md) — SLI definitions (per-budget measurement specs)
