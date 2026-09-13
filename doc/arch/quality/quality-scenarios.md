# Quality Scenarios

This document records the measurable quality scenarios substrate holds itself to. Each row
pairs an ATAM-style stimulus and environment with the response the system owes and the
measurement that decides whether the response happened. The `Attribute` column uses the
ISO/IEC 25010:2023 product-quality characteristic names verbatim:
`functional-suitability`, `performance-efficiency`, `compatibility`,
`interaction-capability`, `reliability`, `security`, `maintainability`, `flexibility`, and
`safety`. Budgets, ratios, and windows are taken from
[ADR-0030](../adr/0030-performance-budgets.md), the SLI expressions in
[ADR-0039](../adr/0039-sli-definitions.md), and the OpenSLO objectives under
[`doc/arch/slo/`](../slo/); a number that already exists there is reused here unchanged.

| ID | Attribute | Stimulus | Environment | Response | Measure |
|---|---|---|---|---|---|
| QS-001 | functional-suitability | an agent invokes a tool that has a matching Gherkin scenario | workspace at the pinned toolchain, allowlist configured | the tool returns the specified result and the documented error code on every failure branch | every scenario under `doc/arch/specs/features/` passes `speckit verify`; a dry-run preview matches the mutation applied by the confirmed call |
| QS-002 | performance-efficiency | an MCP client spawns the server over stdio and issues a light read call | reference CI runner, 4 cores and 8 GiB RAM | the first JSON-RPC response arrives and the call returns inside budget | cold start p99 ≤ 100 ms across 100 launches; `sys.info` p95 ≤ 50 ms; `proc.list` p95 ≤ 200 ms |
| QS-003 | performance-efficiency | a heavy operation runs: `fs.find` over 100,000 files at depth ≤ 10, `text.search` over a 100 MB file, `archive.tar.create` over 1 GB | reference CI runner, no competing load | the call returns inside the documented budget and the criterion baseline is not regressed | p95 ≤ 2 s, p95 ≤ 500 ms, p95 ≤ 30 s respectively; `critcmp baseline current --threshold 15` exits 0 |
| QS-004 | performance-efficiency | ten tool calls run concurrently, then the server sits idle | reference CI runner, default caps | resident memory stays inside the ceilings and no call is rejected by the RSS guard | idle RSS ≤ 30 MiB; peak RSS during the burst ≤ 200 MiB |
| QS-005 | reliability | a client sends `notifications/cancelled` while a write is in flight | production, `shutdown_drain_secs` default of 5 s | the worker observes the cancellation token, removes its transit file, and leaves the target absent or complete | `job.cancel` `duration_ms` p99 < 1 s over a 5-minute window; no `.tmp.<uuid7>` file remains; cleanup completes inside 1 s |
| QS-006 | reliability | `SIGTERM` or `SIGINT` arrives with a 10-second call in flight | server draining, root `CancellationToken` cancelled | in-flight calls return `ToolError::Cancelled`, subprocess groups are terminated, the process exits 0 | exit within `shutdown_drain_secs + 2 s`; zero orphaned transit files; a broken stdout pipe is reported as `BrokenPipe` rather than aborting the process |
| QS-007 | reliability | steady client traffic over a rolling month | 30-day window, `initialize` and `tools/list` served | both requests answer without error and inside 50 ms | non-error ratio ≥ 0.995, with `cancelled` and `timeout` outcomes excluded from numerator and denominator |
| QS-008 | security | a path argument escapes the allowlist by traversal, symlink swap, NFC/NFD mismatch, or a `/proc` prefix | Linux `openat2(RESOLVE_BENEATH)` tier and macOS `O_NOFOLLOW_ANY` tier | the call is rejected before the OS call and the rejection is audited | `SUBSTRATE_PATH_OUTSIDE_ALLOWLIST` or `SUBSTRATE_PATH_TRAVERSAL_BLOCKED` returned; block rate 1.0 over a 30-day window |
| QS-009 | security | `subprocess.spawn` names a binary absent from the allowlist or passes a hard-banned variable such as `LD_PRELOAD` | subprocess feature enabled, binary allowlist left empty | the spawn is rejected before `tokio::process::Command` is built | `SUBSTRATE_SUBPROCESS_BINARY_DENIED` or `SUBSTRATE_SUBPROCESS_ENV_BANNED`; an empty allowlist denies every binary |
| QS-010 | safety | a destructive mutation is requested without `dry_run: false` | production, host supporting form-mode elicitation | a preview is returned, then an elicitation form; the mutation runs only after confirmation | preview equals the applied diff; a 60-second elicitation timeout returns `SUBSTRATE_CONFIRMATION_REQUIRED` and no mutation occurs |
| QS-011 | safety | an adversarial input drives memory or disk past the configured ceiling | default caps, RSS guard armed | the call is rejected with a structured error before the host is starved | `ERR_OUTPUT_TOO_LARGE` at the 8 MiB per-tool cap; RSS above 256 MiB enters a 30-second cooldown returning `SUBSTRATE_RESOURCE_LIMIT`; archive input above 1 GiB is rejected on declared size |
| QS-012 | compatibility | the same workspace is built and exercised on the second supported platform | macOS and Linux CI runners, `--all-features` | both platforms build, lint, and pass the corpus with identical observable behavior | `cargo build`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo nextest run --workspace --all-features`, and the doctest pass green on both; identical jail error codes per tier |
| QS-013 | compatibility | a client negotiates a protocol version at handshake | minimum 2025-06-18, preferred 2025-11-25 | the negotiated version is the lower of client and preferred, capabilities are intersected, stdout carries only JSON-RPC | handshake succeeds for every client version from 2025-06-18 through 2025-11-25; a version below the minimum closes the connection with `-32600`; a 2025-06-18 client receives `-32001` on destructive tools |
| QS-014 | maintainability | a change lands inside one adapter crate | workspace under CI | the change ships without edits to unrelated crates and without relaxing lint | the hexagonal layering policy passes; coverage ≥ 80% in `substrate-domain` and ≥ 70% in adapters; `clippy --all-targets --all-features -- -D warnings` is clean |
| QS-015 | interaction-capability | a 10-billion-parameter model selects a tool and recovers from a failure | published tool cards, structured content with the hints map | the model picks the tool and retries from the hint without a human | card ≤ 180 tokens, `content` ≤ 80 tokens, and every `recovery_hint` ≤ 150 characters, each asserted by lint; no clarification round-trip in the review run |
| QS-016 | flexibility | an operator changes a limit, enables a feature, or lands on a host without the tier-1 jail | deployed host, no recompilation | the change takes effect from configuration; the jail tier is probed and named at startup | TOML override applied without a rebuild; `launch` and `outbound-net` stay default-off; a degraded jail aborts startup with `SUBSTRATE_JAIL_DEGRADED_REFUSED` unless the operator opts out |

## Observability

Every measure above has a named detector. Latency and ratio scenarios are computed from the
audit log JSON Lines stream with the SLI expressions in
[ADR-0039](../adr/0039-sli-definitions.md) — `duration_ms` and `outcome` per event, with
`cancelled` and `timeout` excluded from both numerator and denominator — and compared against
the OpenSLO objectives under [`doc/arch/slo/`](../slo/), so a breach surfaces as a compliance
finding instead of a user report. Cold start, RSS, coverage, and layering measures cannot come
from the runtime log, so the CI harness produces them: criterion benches with
`critcmp baseline current --threshold 15` block merge in the `test` stage, and the RSS and
coverage numbers are written as CI artifacts. Corpus-level drift — a missing attribute, an
empty cell, a table that lost its header — is caught by `speckit validate`, whose `quality`
rule reads this table directly on every save and pre-commit run.