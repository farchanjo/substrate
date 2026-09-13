---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0039 — SLI Definitions (Per-Budget Measurement Spec)

## Context and Problem Statement

ADR-0030 defines SLO latency targets for substrate tools (e.g., `fs.find` p95 ≤ 2000 ms) but does not specify the Service Level Indicator — the precise numerator/denominator expression over a measurement window that yields a compliance ratio. Without an SLI definition, SLO compliance cannot be evaluated programmatically, burn rates cannot be computed, and error budget exhaustion events cannot trigger automated policy responses. Additionally, ADR-0030 does not address memory SLOs for which the measurement source differs from the audit log.

## Decision Drivers

- SLIs must be computable from the existing audit log JSON Lines stream (ADR-0038) without requiring a separate metrics collection agent in the MVP.
- Cancelled and timeout outcomes must be excluded from the latency SLI denominator to avoid penalising the service for client-initiated cancellations.
- Archive operations have a lower call rate than filesystem operations; their measurement window must be wider to accumulate a statistically meaningful sample.
- Memory and cold-start SLIs cannot be measured from the audit log at runtime; they require CI harness integration.
- The SLI definitions must be unambiguous enough for a script to implement without further specification.

## Considered Options

1. Define SLIs purely in terms of Prometheus histogram buckets (requires a metrics agent; deferred from MVP).
2. Define SLIs over the audit log JSON Lines stream with explicit numerator/denominator filters and rolling windows.
3. Embed SLI computation in the substrate binary itself and expose via a `/metrics` endpoint.
4. Defer SLI definitions until the observability stack is chosen post-MVP.

## Decision Outcome

Chosen option: **2 — audit-log-derived SLIs with explicit filter expressions and rolling windows**, because the audit log is the only reliable measurement source available in the MVP without additional infrastructure, and the `duration_ms` and `outcome` fields added by ADR-0038 provide all required data. CI-only SLIs use criterion benchmarks and cargo-tarpaulin perf hooks.

### Consequences

#### Positive

**Production-path latency SLIs**

All latency SLIs share the same expression shape:

```
SLI(tool, threshold_ms) =
  count(audit events where tool_name = <tool>
                       AND duration_ms <= <threshold>
                       AND outcome IN {success, error})
  /
  count(audit events where tool_name = <tool>
                       AND outcome IN {success, error})
```

Measurement window: rolling, reset on each evaluation. Events with `outcome IN {cancelled, timeout}` are excluded from both numerator and denominator. The SLO target from ADR-0030 applies to the ratio over the window.

| Tool | Threshold (ms) | Window |
|---|---|---|
| `fs.find` | 2000 | 5 min |
| `proc.list` | 200 | 5 min |
| `sys.info` | 50 | 5 min |
| `text.search` | 500 | 5 min |
| `archive.tar.create` | 30000 | 10 min |
| `archive.zip.create` | 30000 | 10 min |
| `archive.gzip.compress` | 5000 | 5 min |

Archive operations use a 10-minute window because their expected call rate at typical workloads is too low to produce a statistically reliable 5-minute sample.

**CI-only SLIs**

These SLIs cannot be measured from the runtime audit log. They are evaluated exclusively in CI using the test harness.

*Cold start p99*: SLI = `count(process launches where duration from exec() to first MCP response ≤ 100 ms) / count(process launches)`. Measured via an integration test that spawns the binary 100 times and records the latency of the first JSON-RPC response on stdout. Pass threshold: p99 ≤ 100 ms.

*Idle RSS*: SLI = `count(30-second RSS samples below 30 MiB during idle period) / count(total idle samples)`. Idle is defined as no active tool calls for at least 10 seconds. Measured by sampling `/proc/self/status` (Linux) or `task_info` (macOS) every 30 seconds during a 5-minute idle integration test. Pass threshold: SLI ≥ 0.99 (i.e., ≤ 1 sample in 100 exceeds 30 MiB).

*10-concurrent RSS peak*: SLI is a pass/fail metric, not a ratio. Pass condition: peak RSS during a burst of 10 concurrent tool calls does not exceed 200 MiB. Measured by the CI concurrency stress test harness. No rolling window; single measurement per CI run.

**Measurement source**

Production-path SLIs source `duration_ms` and `outcome` from `AuditEvent` as specified in ADR-0038. The `duration_ms` field covers the wall-clock duration from tool entry to the terminal audit event emission. CI-only SLIs use criterion micro-benchmarks for latency and OS memory APIs for RSS.

**Compliance reporting**

A compliance script (out of MVP scope but documented here as the consumer contract) reads the audit log JSON Lines stream, maintains rolling windows keyed by `tool_name`, evaluates each SLI expression above, and emits a JSON Lines compliance report with fields `{tool, window_start, window_end, numerator, denominator, sli_ratio, slo_target, compliant}`. The audit log field names and filter semantics defined in this ADR are the stable contract that the script must implement against.

#### Negative

- Audit-log-derived SLIs have a minimum latency equal to the log write latency of `tracing_appender::non_blocking`; in practice this is sub-millisecond but is not zero.
- The compliance script is explicitly deferred from MVP, so there is a gap between defining the SLIs here and having automated compliance reporting in production.
- CI-only memory SLIs will not detect regressions introduced between CI runs (e.g., by a configuration change in production without a corresponding release). Operators must rely on incident response for production memory issues until a runtime metric is added.
- Archive tool windows of 10 minutes mean that a sustained latency regression may take up to 10 minutes to breach the SLO ratio in the compliance report.

## Validation

- A unit test reads a synthetic audit log with 100 events (95 `duration_ms ≤ 2000`, 3 `duration_ms > 2000`, 2 `outcome = cancelled`) for `fs.find` and asserts that the computed SLI ratio is `95/98 ≈ 0.969` (cancelled events excluded from denominator).
- CI cold-start test executes the p99 measurement across 100 launches and asserts the result is ≤ 100 ms. The test is marked `#[ignore]` for local development to avoid contaminating unit test runtime but is always executed in CI.
- CI memory test runs the 10-concurrent burst and asserts peak RSS ≤ 200 MiB. Failure is a blocking CI gate.
- A documentation test in `src/audit/sli.rs` (or equivalent) ensures that the SLI expression implementation matches the filter logic specified in this ADR.

## Cross-References

- [ADR-0009](0009-observability.md) — Observability (tracing framework; audit log is the measurement source)
- [ADR-0030](0030-performance-budgets.md) — SLO targets (parent; defines the target ratios this ADR measures against)
- [ADR-0038](0038-audit-event-semantics.md) — Audit event semantics (defines `duration_ms`, `outcome`, `tool_name` fields consumed here)

---

## Amendment — 2026-06-10: SLIs for Subprocess (Bucket E) and Network-Info Tools

### Context

The original SLI table covered seven tools across five bounded contexts (filesystem-query, process, system-info, text-processing, archive). Two bounded contexts added after the initial acceptance — subprocess (ADR-0052, Bucket E always-async dispatch) and network-info (ADR-0058) — have no production-path SLIs, leaving the error budget framework incomplete for `subprocess.*` and `net.*` tools.

ADR-0054 introduces a `stream_chunks_dropped` quality counter (`SUBSTRATE_STREAM_CHUNK_DROPPED` audit events) for Bucket E jobs. This counter is a distinct quality dimension from latency and warrants its own SLI.

### New SLI Definitions

**Subprocess Bucket E spawn-to-running latency**

```
SLI(subprocess.spawn, threshold_ms=500) =
  count(audit events where tool_name = "subprocess.spawn"
                       AND outcome IN {success, error}
                       AND duration_ms <= 500)
  /
  count(audit events where tool_name = "subprocess.spawn"
                       AND outcome IN {success, error})
```

Measurement window: 5-minute rolling. `duration_ms` covers the wall-clock time from `subprocess.spawn` invocation to the first `job_state = Running` notification emission (i.e., spawn-to-running latency, not job completion). Cancelled and timeout outcomes excluded from both numerator and denominator per the general rule above.

SLO target: p95 spawn-to-running ≤ 500 ms. See `docs/arch/slo/subprocess-stream-integrity.yaml` for the corresponding OpenSLO file.

**Subprocess stream chunk drop rate**

```
SLI(stream_integrity) =
  count(audit events where event_type = "SUBSTRATE_STREAM_CHUNK_DELIVERED")
  /
  count(audit events where event_type IN {
    "SUBSTRATE_STREAM_CHUNK_DELIVERED",
    "SUBSTRATE_STREAM_CHUNK_DROPPED"
  })
```

`SUBSTRATE_STREAM_CHUNK_DROPPED` events are emitted by the dispatcher task per ADR-0054 whenever `mpsc::Sender::try_send` returns `Err::Full`. `SUBSTRATE_STREAM_CHUNK_DELIVERED` events are emitted on each successful `notifications/progress` emission. Measurement window: 10-minute rolling (long-lived services accumulate chunks slowly in low-throughput phases). SLO target: delivery rate ≥ 0.999 (drop rate ≤ 0.1%). See `docs/arch/slo/subprocess-stream-integrity.yaml`.

**Network-info read latency**

```
SLI(net_tools, threshold_ms=500) =
  count(audit events where tool_name IN {
    "net.tcp_list", "net.udp_list",
    "net.tcp_stats", "net.connection_count"
  }
                       AND outcome IN {success, error}
                       AND duration_ms <= 500)
  /
  count(audit events where tool_name IN {
    "net.tcp_list", "net.udp_list",
    "net.tcp_stats", "net.connection_count"
  }
                       AND outcome IN {success, error})
```

Measurement window: 5-minute rolling. Net tools are Zone A (async-native, no job registry overhead); a 500 ms p95 threshold aligns with the `sys.info` tier. Cancelled and timeout outcomes excluded. SLO target: p95 ≤ 500 ms. See `docs/arch/slo/net-read-latency.yaml`.

### Amended SLI Table

The table in the Consequences section above now extends to:

| Tool | Threshold (ms) | Window |
|---|---|---|
| `fs.find` | 2000 | 5 min |
| `proc.list` | 200 | 5 min |
| `sys.info` | 50 | 5 min |
| `text.search` | 500 | 5 min |
| `archive.tar.create` | 30000 | 10 min |
| `archive.zip.create` | 30000 | 10 min |
| `archive.gzip.compress` | 5000 | 5 min |
| `subprocess.spawn` (spawn-to-running) | 500 | 5 min |
| `net.tcp_list` / `net.udp_list` / `net.tcp_stats` / `net.connection_count` | 500 | 5 min |

The `stream_chunks_dropped` SLI is a ratio metric (not a latency threshold) and is listed separately above.

### Cross-References (amendment)

- [ADR-0052](0052-subprocess-execution-architecture.md) — Subprocess BC (Bucket E)
- [ADR-0054](0054-subprocess-stream-multiplex.md) — Stream chunk drop counter (`SUBSTRATE_STREAM_CHUNK_DROPPED`)
- [ADR-0058](0058-network-socket-introspection.md) — Network-info BC tools
