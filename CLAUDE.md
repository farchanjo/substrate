# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project: substrate

`substrate` is a Model Context Protocol (MCP) server written in Rust 1.95 that exposes POSIX baseutils-equivalent OS management to LLM agents. Org: `com.archanjo`. Transport: STDIO only (no socket server, no HTTP/SSE). License: MIT/Apache-2.0 dual.

**Current phase: architecture-complete, pre-implementation.** No Rust source exists yet. The entire design lives under `docs/arch/` as a spec-as-source-of-truth project. Read the spec before writing code.

## Spec workflow

This repo uses the `spec` CLI (`~/bin/spec`) backed by a Python framework at `~/dev/fapp/spec-framework/`. All spec artifacts live under `docs/arch/`.

```shell
# Fast lane (~1.5s) — runs on every save / pre-commit
spec validate --lane fast

# Default lane (~10s) — adds Structurizr DSL, MADR lint, markdown lint
spec validate

# Full lane (CI) — adds conftest/vale/SLO/AsyncAPI/TLC validators (currently no inputs for several)
spec validate --lane full
```

Individual linters: `spec lint:cue`, `spec lint:madr`, `spec lint:features`, `spec lint:ddd-role`, `spec lint:structurizr`, `spec lint:md`, `spec lint:yaml`.

When changing any file under `docs/arch/`, run `spec validate --lane fast` before committing. Pre-implementation, CI gates on `spec validate --lane full`.

## Spec layout

```text
docs/arch/
  adr/                       MADR 4.0 decision records (0001 meta + 0002–0039)
  architecture/workspace.dsl Structurizr DSL (C4 context + container views)
  cue.mod/module.cue         CUE module: com.archanjo/substrate
  domain/<bc>/README.md      Bounded-context narratives (6 BCs)
  policies/*.rego            Open Policy Agent rules (6 policies)
  schemas/*.cue              CUE schemas (11 files, all with DDD role headers)
  specs/features/<area>/     Gherkin feature specs (64 features)
  glossary.md                Ubiquitous-language vocabulary
  README.md                  Architecture-spec entry point
  .specconfig.yml            Spec framework per-project config
```

## Bounded contexts (DDD strategic)

Seven contexts, split by semantic family (not by binary name):

1. **filesystem-query** — read-side: ls, find, stat, du, file, hash
2. **filesystem-mutation** — write-side: mkdir, write, copy, rename, remove, chmod, symlink, touch
3. **process** — proc.list, proc.tree, proc.signal
4. **system-info** — sys.info, sys.uptime, sys.df, sys.uname, sys.hostname, sys.load_average
5. **text-processing** — text.search, text.count_lines, text.head, text.tail
6. **archive** — archive.tar/zip/gzip create+extract, archive.hash
7. **job** — job.list, job.result, job.cancel, job.status (async control-plane)

Tools are namespaced `<bc>.<verb>` (e.g., `fs.find`, `proc.signal`). Each BC will become a Cargo crate under `crates/substrate-<bc>` (see ADR-0022). The pure-domain shared kernel lives in `crates/substrate-domain` and MUST NOT import any infra crate (hexagonal layering enforced via `policies/hexagonal_layering.rego`).

## Locked architectural decisions

When implementation begins, the following decisions are anchors — do not re-decide without superseding the relevant ADR:

- **Stack**: Rust 1.95 (edition 2024) pinned via `mise.toml`. rmcp 1.7.x with features `["server", "transport-io", "macros"]` (NO `transport-sse`, NO `transport-streamable-http`). tokio 1.4x multi-threaded work-stealing, NO `net` feature unless Cargo feature `outbound-net` is opted in. See ADR-0003, ADR-0006.
- **Async zones**: A (async-native), B (sync I/O via `spawn_blocking`), C (CPU-bound via `spawn_blocking` + `Semaphore` sized to `num_cpus`). See ADR-0003.
- **Transport**: STDIO only. `stdout` is sacred (JSON-RPC channel). `println!`/`print!` forbidden in `src/`. All logging to `stderr` via `tracing_subscriber::fmt().with_writer(std::io::stderr)`. See ADR-0005.
- **Security (defense in depth)**: allowlist (TOML, default-deny) → path jail via `strict-path` + `openat2(RESOLVE_BENEATH|NO_SYMLINKS)` on Linux / `O_NOFOLLOW_ANY` on macOS → dry-run mandatory for mutations → elicitation form-mode for destructive ops (fs.remove, fs.rename, fs.set_permissions, proc.signal SIGKILL/SIGTERM/SIGSTOP, archive create/extract). See ADR-0004, ADR-0035.
- **Signal safety**: `signal(SIGPIPE, SIG_IGN)` at startup. blake3 mmap feature DISABLED to avoid SIGBUS on concurrent truncation. SIGTERM/SIGINT trigger graceful drain (`shutdown_drain_secs` default 5s). See ADR-0032.
- **Cancellation**: `tokio-util` `CancellationToken` + `tokio::select! biased` with work as first arm. Use `Arc<Semaphore>::acquire_owned()` for permits; permits MUST live in async scope, never moved into `spawn_blocking` closures (because `panic = "abort"` per ADR-0014 prevents unwind-based RAII inside blocking closures). See ADR-0037.
- **Transactional writes**: every disk-write tool uses `<target>.tmp.<uuid7>` + atomic rename + cleanup on cancel/error. `statvfs` preflight for disk-space guard. See ADR-0033.
- **Tool descriptions ("narrative arc")**: each tool description ≤180 tokens, fixed template USE/DOES/ARGS/RETURNS/NEXT/AVOID. Response bifurcates into `content` (model-oriented text ≤80 tokens) and `structuredContent` (JSON + hints map: `next_action_suggested`, `alternative_tool`, `confirm_destructive`, `quota_status`, `error_recovery`). Targets 10B-param models. See ADR-0007.
- **MCP protocol**: min version 2025-06-18 (structuredContent + outputSchema), preferred 2025-11-25 (form-mode + URL-mode elicitation). Capability intersection computed at handshake. See ADR-0013.
- **Pagination**: cursor-based base64-opaque, page_size 50 default, max 500. See ADR-0008.
- **Error taxonomy**: 13 base codes + 6 kernel-induced (SYMLINK_LOOP, IO_ERROR, STORAGE_FULL, READ_ONLY_FS, ENCODING_ERROR, TRANSIENT_IO) + 7 startup codes. Stable `SUBSTRATE_<UPPER_SNAKE>` form. Every error includes `code`, `message_en_us`, `recovery_hint` (≤150 chars), `correlation_id`. See ADR-0010, ADR-0034, ADR-0036.

## Cargo workspace layout (planned)

When implementation starts, follow ADR-0022 exactly:

```text
crates/
  substrate-domain          pure ports + value objects + errors (zero infra deps)
  substrate-policy          allowlist + path jail enforcement
  substrate-config          figment-based TOML loader
  substrate-fs-query        adapter for filesystem-query BC
  substrate-fs-mutation     adapter for filesystem-mutation BC
  substrate-process         adapter for process BC
  substrate-system-info     adapter for system-info BC
  substrate-text            adapter for text-processing BC
  substrate-archive         adapter for archive BC
  substrate-mcp-server      binary (composition root, rmcp wiring)
```

Hexagonal layering rule: `substrate-domain` imports only std + serde + thiserror + async-trait + futures + uuid + tracing. Adapter crates depend on `substrate-domain` (+ `substrate-policy` for write-paths), never on each other. Only `substrate-mcp-server` depends on rmcp and tokio with `net` (if `outbound-net` feature on). Enforced by `policies/hexagonal_layering.rego`.

## Reading order for a new contributor

When picking up this repo cold, read in this order:

1. `docs/arch/README.md` — project overview
2. `docs/arch/glossary.md` — ubiquitous language
3. `docs/arch/adr/0002-bounded-contexts.md` — strategic DDD
4. `docs/arch/adr/0007-tool-card-narrative-arc.md` — tool design template
5. `docs/arch/adr/0004-security-model.md` + ADR-0035 — security layers
6. `docs/arch/adr/0003-crate-stack-and-async-zones.md` — Rust stack + async zones
7. `docs/arch/architecture/workspace.dsl` — C4 model (render with Structurizr CLI or Lite)
8. `docs/arch/domain/<bc>/README.md` — BC you intend to touch

For implementation work later: read the ADRs cross-referenced from the relevant BC README, then the matching CUE schemas under `docs/arch/schemas/`, then the matching Gherkin features under `docs/arch/specs/features/<bc>/`.

## Spec conventions (enforced by linters)

- All artifacts en-US. Spec markdown uses CommonMark + Mermaid diagrams (per ADR-0047). GFM tables, emojis, and task lists remain disallowed in spec markdown. Mermaid is MANDATORY where a diagram aids comprehension (flowchart, sequence, state, ER, class, gantt, pie, gitGraph, mindmap, timeline, C4). ASCII art is retained only when Mermaid cannot render the intended shape.
- ADR filenames: `NNNN-kebab-case-slug.md`. ADR numbers never reused; superseded ADRs link forward.
- CUE filenames: `snake_case.cue`. CUE definitions: `#PascalCase`. Every CUE file requires header `// DDD role: <AggregateRoot|Entity|ValueObject|DomainService|ReadModel>`.
- Gherkin filenames: `kebab-case.feature`. One scenario per behavior.
- Rego packages: `substrate.<area>`.
- Cross-ref ADRs via relative markdown links: `[ADR-NNNN](NNNN-slug.md)`.
- UUIDv7 only.
- pnpm/npm FORBIDDEN. uv for Python tooling only (this is a Rust project; uv only used by the `spec` framework itself).

## Commit / branch conventions

- Angular format: `<type>(<scope>): <subject>` where types are `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `ci`, `chore`, `perf`, `style`, `security`. Scopes match crate names (`fs-query`, `process`, `mcp-server`, etc.) or `adr` for ADR-only changes. See ADR-0024.
- Small contextual commits — never bulk "various changes".
- Branch naming: `feat/<scope>-<short-desc>`, `fix/<scope>-<short-desc>`, `chore/<short-desc>`.
- DCO sign-off required (`Signed-off-by:` trailer). No CLA.

## When implementation begins

The `impl-guard` hook (if enabled in user settings) blocks source-code edits until a spec artifact under `docs/arch/` is observed in the session. Override single-shot via `/impl-ok` when explicitly justified.

Recommended order to start coding:

1. Bootstrap Cargo workspace + `mise.toml` per ADR-0014 + ADR-0022.
2. Implement `substrate-domain` (port traits + value objects + error enum). Round-trip with `schemas/*.cue` definitions.
3. Implement `substrate-policy` (allowlist + path jail wrapping `strict-path`). Test against `specs/features/filesystem-query/fs-find-path-traversal-blocked.feature` and ADR-0035 scenarios.
4. Implement `substrate-config` (figment + TOML, with `deny_unknown_fields`, allowlist canonicalization at startup).
5. Implement adapters BC-by-BC. For each adapter, validate against corresponding Gherkin features (executable via `cucumber-rs` in `crates/substrate-mcp-server/tests/`).
6. Wire `substrate-mcp-server` composition root last (rmcp service, signal handlers per ADR-0032, capability negotiation per ADR-0013).

Every adapter implementation must obey async-zone classification (A/B/C) declared in ADR-0003 and the cancellation patterns in ADR-0037. Use `criterion` benchmarks per ADR-0030 to verify performance budgets; CI fails on >15% regression.
