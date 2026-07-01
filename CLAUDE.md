# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project: substrate

`substrate` is a Model Context Protocol (MCP) server written in Rust 1.95 that exposes POSIX baseutils-equivalent OS management to LLM agents. Org: `com.archanjo`. Transport: STDIO only (no socket server, no HTTP/SSE). License: MIT/Apache-2.0 dual.

**Current phase: active implementation** (17-crate Cargo workspace, v0.2.0). The spec under `docs/arch/` remains the source of truth — read it before changing code. Where spec and code contradict, the code is ground truth; raise a spec-correction PR alongside any code change.

**Target platforms**: macOS and Linux, both actively built/clippy'd/tested (verify commands: `cargo build --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`). Linux has no local dev machine in this project's usual workflow — verify Linux-only (`#[cfg(target_os = "linux")]`) changes via Docker (`rust:1.95.0` image) on a remote host before assuming they compile; several such code paths (path jail via `openat2`, fs-index/fs-query statx-tier walkers, procfs-based process/system-info readers) went uncompiled on real Linux for a long time and had genuine bugs (API drift, `nix`/`libc` field-width divergence between platforms, an `openat2(2)` `RESOLVE_BENEATH` absolute-path bug) that only surfaced once actually exercised there. **Never remove a clippy-flagged "redundant" cast on one platform without checking the field's width on the other** — `nix::sys::stat::Mode`/`mode_t` is `u32` on Linux but `u16` on macOS/BSD; `nix::sys::statvfs::Statvfs::blocks_available()` is `u64` on Linux but `u32` on macOS/BSD. The established idiom for a genuine divergence is `#[cfg_attr(target_os = "linux", expect(clippy::..., reason = "..."))]`, never a bare removal.

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

When changing any file under `docs/arch/`, run `spec validate --lane fast` before committing. CI gates on `spec validate --lane full`.

## Spec layout

```text
docs/arch/
  adr/                       MADR 4.0 decision records (0001–0071)
  architecture/workspace.dsl Structurizr DSL (C4 context + container views)
  cue.mod/module.cue         CUE module: com.archanjo/substrate
  domain/<bc>/README.md      Bounded-context narratives (10 BCs; launch implemented)
  policies/*.rego            Open Policy Agent rules (17 policies)
  schemas/*.cue              CUE schemas (14 files, all with DDD role headers)
  specs/features/<area>/     Gherkin feature specs (155 features)
  glossary.md                Ubiquitous-language vocabulary
  README.md                  Architecture-spec entry point
  .specconfig.yml            Spec framework per-project config
```

## Bounded contexts (DDD strategic)

Ten contexts, split by semantic family (not by binary name):

1. **filesystem-query** — read-side: ls, find, stat, du, file, hash
2. **filesystem-mutation** — write-side: mkdir, write, copy, rename, remove, chmod, symlink, touch
3. **process** — proc.list, proc.tree, proc.signal
4. **system-info** — sys.info, sys.uptime, sys.df, sys.uname, sys.hostname, sys.load_average
5. **text-processing** — text.search, text.count_lines, text.head, text.tail
6. **archive** — archive.tar/zip/gzip create+extract, archive.hash
7. **job** — job.list, job.result, job.cancel, job.status (async control-plane)
8. **subprocess** — subprocess.spawn, subprocess.list, subprocess.result, subprocess.cancel, subprocess.signal, subprocess.search (ADR-0052)
9. **network-info** — net.tcp_list, net.udp_list, net.tcp_stats, net.connection_count (ADR-0058)
10. **launch** *(ADR-0063..0070; implemented including Milestone 2 detached supervisor, on both Linux and macOS)* — declarative process orchestration over subprocess: launch.init/list/trust/up/status/logs/restart/reload/down/forget (10 tools), gated behind Cargo feature `launch` (default-off, implies `subprocess` **and** `substrate-subprocess/outbound-net`). Readiness gating is real (ADR-0056/0065 amendments, 2026-07-01): a probe-gated Service is born `Starting` and only reported `Ready` once its `PortOpen`/`HttpGet` health probe passes (the subprocess supervisor polls and promotes `Starting -> Ready`); `wait_ready` uses a per-probe budget, not the old fixed 1s ceiling; a Service that never becomes ready is stopped. `launch` implies `outbound-net` because those probes are inert without it. Service `command[0]` may be absolute, `cwd`-relative, or a bare name resolved on `$PATH`, resolved to an absolute path before the spawn while the binary allowlist stays the execution gate (ADR-0070). A Service may load `.env` files via `env_file` (paths relative to the profile dir, no escape; later file > earlier, inline `env` > files; ADR-0071)

Tools are namespaced `<bc>.<verb>` (e.g., `fs.find`, `proc.signal`). Total `tools/list` count is 61 with the `launch` feature enabled (51 without). Each BC maps to a Cargo crate under `crates/substrate-<bc>` (see ADR-0022). The `substrate-launch` crate (ADR-0063) is a workspace member, gated behind the default-off Cargo feature `launch`. Its detached-supervisor mode (ADR-0068) is fully built: `LaunchRegistry::up` forks a `substrate --supervise <stack_id>` child on `on_client_disconnect = detach`, polls its durable `supervisor.json`, and a fresh MCP server reaps/re-attaches any Stack left behind by a prior session at startup. See ADR-0068's amendments for the three deliberate deviations from its literal design (tokio `select!` reactor instead of hand-rolled `mio`; poll-based child-exit instead of `pidfd`/`kqueue`; macOS pgid+reaper-on-boot instead of watchdog-pipe cooperation for arbitrary children). The pure-domain shared kernel lives in `crates/substrate-domain` and MUST NOT import any infra crate (hexagonal layering enforced via `policies/hexagonal_layering.rego`).

## Locked architectural decisions

When implementation begins, the following decisions are anchors — do not re-decide without superseding the relevant ADR:

- **Stack**: Rust 1.95 (edition 2024) pinned via `mise.toml`. rmcp 1.7.x with features `["server", "transport-io", "macros"]` (NO `transport-sse`, NO `transport-streamable-http`). tokio 1.4x multi-threaded work-stealing, NO `net` feature unless Cargo feature `outbound-net` is opted in. See ADR-0003, ADR-0006.
- **Async zones**: A (async-native), B (sync I/O via `spawn_blocking`), C (CPU-bound via `spawn_blocking` + `Semaphore` sized to `num_cpus`). See ADR-0003.
- **Transport**: STDIO only. `stdout` is sacred (JSON-RPC channel). `println!`/`print!` forbidden in `src/`. All logging to `stderr` via `tracing_subscriber::fmt().with_writer(std::io::stderr)`. See ADR-0005.
- **Security (defense in depth)**: allowlist (TOML, default-deny) → path jail via `strict-path` + `openat2(RESOLVE_BENEATH|NO_SYMLINKS)` on Linux / `O_NOFOLLOW_ANY` on macOS → dry-run mandatory for mutations → elicitation form-mode for destructive ops (fs.remove, fs.rename, fs.set_permissions, proc.signal SIGKILL/SIGTERM/SIGSTOP, archive create/extract). The Linux `openat2` jail resolves the requested path relative to the allowlist-root dirfd before the syscall (`RESOLVE_BENEATH` categorically rejects absolute pathnames per the kernel ABI); a lexical `..`-escape still reaches the kernel's own containment check. See ADR-0004, ADR-0035.
- **Signal safety**: `signal(SIGPIPE, SIG_IGN)` at startup. blake3 mmap feature DISABLED to avoid SIGBUS on concurrent truncation. SIGTERM/SIGINT trigger graceful drain (`shutdown_drain_secs` default 5s). See ADR-0032.
- **Cancellation**: `tokio-util` `CancellationToken` + `tokio::select! biased` with work as first arm. Use `Arc<Semaphore>::acquire_owned()` for permits; permits MUST live in async scope, never moved into `spawn_blocking` closures (because `panic = "abort"` per ADR-0014 prevents unwind-based RAII inside blocking closures). See ADR-0037.
- **Transactional writes**: every disk-write tool uses `<target>.tmp.<uuid7>` + atomic rename + cleanup on cancel/error. `statvfs` preflight for disk-space guard. See ADR-0033.
- **Tool descriptions ("narrative arc")**: each tool description ≤180 tokens, fixed template USE/DOES/ARGS/RETURNS/NEXT/AVOID. Response bifurcates into `content` (model-oriented text ≤80 tokens) and `structuredContent` (JSON + hints map: `next_action_suggested`, `alternative_tool`, `confirm_destructive`, `quota_status`, `error_recovery`). Targets 10B-param models. See ADR-0007.
- **MCP protocol**: min version 2025-06-18 (structuredContent + outputSchema), preferred 2025-11-25 (form-mode + URL-mode elicitation). Capability intersection computed at handshake. See ADR-0013.
- **Pagination**: cursor-based base64-opaque, page_size 50 default, max 10000 (domain `PageSize::MAX`). See ADR-0008, ADR-0060.
- **Error taxonomy**: 58 codes total (original 13 base + 6 kernel-induced + 7 startup + additions from job control-plane, capability/elicitation, subprocess, and launch BCs). Stable `SUBSTRATE_<UPPER_SNAKE>` form. Every error includes `code`, `message_en_us`, `recovery_hint` (≤150 chars), `correlation_id`. See ADR-0010, ADR-0034, ADR-0036, ADR-0040, ADR-0042, ADR-0052, ADR-0063, ADR-0068.

## Cargo workspace layout

Follow ADR-0022 for layering rules. Current workspace members (`Cargo.toml`):

```text
crates/
  substrate-domain              pure ports + value objects + errors (zero infra deps)
  substrate-policy              allowlist + path jail enforcement
  substrate-config              figment-based TOML loader
  substrate-fs-index            filesystem index / watch infrastructure
  substrate-fs-index-macos-sys  macOS FSEvents sys bindings (substrate-fs-index dep)
  substrate-signal-sys          low-level signal handling sys bindings
  substrate-fs-query            adapter for filesystem-query BC
  substrate-fs-mutation         adapter for filesystem-mutation BC
  substrate-process             adapter for process BC
  substrate-system-info         adapter for system-info BC
  substrate-text                adapter for text-processing BC
  substrate-archive             adapter for archive BC
  substrate-jobs                adapter for job control-plane BC
  substrate-subprocess          adapter for subprocess BC (ADR-0052)
  substrate-network-info        adapter for network-info BC (ADR-0058)
  substrate-launch              adapter for launch orchestration BC (ADR-0063..0070), feature-gated
  substrate-mcp-server          binary (composition root, rmcp wiring)
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

For the launch BC specifically, ADR-0063 (bounded context), ADR-0064 (profile trust model), ADR-0065 (dependency graph + reconciler/reload; 2026-07-01 amendment: readiness gating made real + per-probe budget + `launch` implies `outbound-net`), ADR-0066 (event stream), ADR-0067 (concurrency/messaging topology), ADR-0068 (detached supervisor + orphan governance), ADR-0069 (tool cards + ToolSearch discoverability), ADR-0070 (PATH binary resolution), and ADR-0071 (`.env` file support) form one connected design — read them together, in that order. The health-probe wiring these depend on is in ADR-0056 (2026-07-01 amendment: `Starting -> Ready` edge + probe supervisor).

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

- Angular format: `<type>(<scope>): <subject>` where types are `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `ci`, `chore`, `perf`, `style`, `security`. Scopes match crate names (`fs-query`, `process`, `mcp-server`, `launch`, etc.) or `adr` for ADR-only changes. See ADR-0024.
- Small contextual commits — never bulk "various changes".
- Branch naming: `feat/<scope>-<short-desc>`, `fix/<scope>-<short-desc>`, `chore/<short-desc>`.
- DCO sign-off required (`Signed-off-by:` trailer, `git commit -s`). No CLA.

## Implementation guidance

The `impl-guard` hook (if enabled in user settings) blocks source-code edits until a spec artifact under `docs/arch/` is observed in the session. Override single-shot via `/impl-ok` when explicitly justified.

The workspace is bootstrapped and active. When adding a new BC or adapter, the recommended onboarding order is:

1. Confirm the relevant BC ADR and CUE schema are up-to-date with the code before changing anything.
2. Work in `substrate-domain` for new port traits / value objects / error codes. Round-trip with `schemas/*.cue` definitions.
3. Work in `substrate-policy` for allowlist / path jail changes. Test against `specs/features/filesystem-query/fs-find-path-traversal-blocked.feature` and ADR-0035 scenarios. Path-jail changes are security-critical — read the full existing implementation for the platform(s) you touch before editing, and verify both the macOS (`ONoFollowAnyJail`) and Linux (`Openat2Jail`) tiers stay behaviorally consistent (same absolute-path calling convention, same NFC-normalized containment check).
4. Adjust `substrate-config` (figment + TOML, `deny_unknown_fields`, allowlist canonicalization at startup) for any new configuration surface.
5. Implement adapter changes BC-by-BC. For each adapter, validate against corresponding Gherkin features (executable via `cucumber-rs` in `crates/substrate-mcp-server/tests/`). Note: the cucumber suite has a known, pre-existing gap of undefined step definitions for some `text.head`/`text.tail` and other scenarios (steps referenced in `.feature` files with no matching `#[given]`/`#[when]`/`#[then]` regex) — this is a step-coverage gap from the project's spec-first workflow, not a regression signal; don't assume every cucumber failure is your change's fault, but don't silently paper over new ones either.
6. Wire `substrate-mcp-server` composition root for any new service surface (rmcp service, signal handlers per ADR-0032, capability negotiation per ADR-0013).

Every adapter implementation must obey async-zone classification (A/B/C) declared in ADR-0003 and the cancellation patterns in ADR-0037. Use `criterion` benchmarks per ADR-0030 to verify performance budgets; CI fails on >15% regression.
