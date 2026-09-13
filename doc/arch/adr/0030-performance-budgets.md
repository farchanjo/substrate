---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0030 — Performance Budgets

## Context and Problem Statement

Substrate is invoked by LLM agents in tight feedback loops. Latency and memory usage directly affect agent responsiveness and host system stability. Without explicit, measurable budgets, performance regressions silently accumulate across releases until they become user-visible.

Budgets must be anchored to realistic workloads, enforced in CI, and tracked across releases so regressions are caught before they reach consumers.

## Decision Drivers

- LLM agents expect sub-second response times for lightweight operations (sys.info, proc.list).
- Heavy operations (archive.tar.create 1 GB, text.search 100 MB) have natural lower bounds; budgets must be achievable on common CI runner hardware.
- Cold start latency affects every agent session; p99 ≤ 100 ms is required for interactive use.
- Memory bounds prevent Substrate from crowding out the LLM process it serves on memory-constrained hosts.
- A regression detection threshold of >15% ensures noise does not trigger false alarms while catching real slowdowns.

## Considered Options

1. No formal budgets — rely on manual review to catch regressions.
2. Informal targets in documentation — guidance only, not CI-enforced.
3. Criterion microbenchmarks with CI regression detection — enforced, but requires baseline management.
4. End-to-end timing tests only — catches regressions but misses component-level hot spots.
5. Criterion microbenchmarks + end-to-end timing tests — comprehensive; selected.

## Decision Outcome

Chosen option: "criterion microbenchmarks plus end-to-end timing tests", because microbenchmarks isolate component regressions while end-to-end tests validate the full MCP request path including serialization and transport overhead.

### Operational SLO Budgets

All measurements at p95 unless noted. Reference hardware: 4-core / 8 GB RAM (CI runner minimum). macOS and Linux runners both enforced.

| Operation | Workload | Budget |
|---|---|---|
| `fs.find` | 100,000 files, depth ≤ 10 | p95 ≤ 2 s |
| `proc.list` | ~500 processes | p95 ≤ 200 ms |
| `sys.info` | single call | p95 ≤ 50 ms |
| `text.search` | 100 MB file, literal pattern | p95 ≤ 500 ms |
| `archive.tar.create` | 1 GB uncompressed input | p95 ≤ 30 s |
| Cold start (stdio transport) | first tool response after process spawn | p99 ≤ 100 ms |
| Idle RSS | no active requests | ≤ 30 MiB |
| 10-concurrent-call RSS | 10 simultaneous MCP tool calls | ≤ 200 MiB |

### Criterion Microbenchmarks

Location: `benches/` at workspace root, one file per bounded context.

```
benches/
  fs_find.rs
  proc_list.rs
  sys_info.rs
  text_search.rs
  archive_tar.rs
  cold_start.rs
```

Each benchmark uses `criterion::Criterion` with:

- `sample_size(50)` minimum.
- `measurement_time(Duration::from_secs(30))` for heavy operations.
- Baseline saved to `target/criterion/` and committed to CI cache.

Run locally:

```sh
cargo bench --bench fs_find
```

### End-to-End Timing Tests

Location: `tests/perf/` using `tokio::time::Instant` and `assert!` with budget constants.

```rust
const FS_FIND_P95_MS: u64 = 2_000;
```

Tests spawn a real `substrate` process over stdio, send a valid MCP request, and assert the response arrives within the budget. These tests are gated behind the `perf` feature flag and run only in the `test` CI stage with `--features perf`.

### Regression Detection

CI compares benchmark output against the stored baseline using `critcmp`:

```sh
critcmp baseline current --threshold 15
```

A result showing >15% slowdown on any benchmark causes the `test` CI job to exit non-zero, blocking merge. The threshold is intentionally above measurement noise (typically ≤ 3% on stable runners) but sensitive enough to catch meaningful regressions.

Baseline is updated manually by a Maintainer after a deliberate performance trade-off is accepted via an ADR amendment.

### Memory Profiling (Optional)

`cargo-flamegraph` is available for local investigation but is not required in CI. Developers investigating a regression run:

```sh
cargo flamegraph --bench fs_find -- --bench
```

RSS measurements in CI use `/usr/bin/time -v` (Linux) or `command time -l` (macOS) wrapping the end-to-end perf test binary.

### Consequences

#### Positive

- Regressions are caught before merge, not after user reports.
- Explicit budgets set expectations for downstream consumers integrating Substrate into latency-sensitive agents.
- Criterion baselines provide a quantitative record of performance evolution across releases.

#### Negative

- CI runner hardware variance can produce false positives on the 15% threshold during runner fleet changes; baseline may need resetting after infrastructure upgrades.
- End-to-end perf tests add ~2 minutes to the `test` stage on macOS runners.
- Maintaining benchmark fixtures (100k-file directory, 100 MB text file, 1 GB archive input) requires CI artifact storage management.

## Validation

- `cargo bench` runs in the `test` CI stage on every MR targeting `main`.
- `critcmp` regression check exits non-zero on >15% slowdown; blocks merge.
- End-to-end perf tests (`cargo test --features perf --test perf`) run in the `test` stage on `main` pushes only (not on every MR, to control runner cost).
- RSS measurements are logged as CI artifacts and surfaced in the MR performance widget.

## Cross-References

- ADR-0012: Observability (OTEL spans used to instrument benchmark hot paths)
- ADR-0016: Async execution model (tokio runtime configuration affects cold-start and concurrency budgets)
