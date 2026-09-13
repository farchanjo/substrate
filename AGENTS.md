# AGENTS.md

This file provides guidance to coding agents when working with code in this repository. It is deliberately harness-agnostic: Claude Code, Codex, Cursor, Zed, Amp, and any other `AGENTS.md`-aware client read these same instructions. `CLAUDE.md` and `GROK.md` are symlinks to this file — edit this one only.

A few rules below are also backed by harness-side mechanisms (hooks, slash commands, deferred tool search). Those mechanisms are optional and named where they apply; where a client does not implement them, the rule still holds as written.

## Project: substrate

`substrate` is a Model Context Protocol (MCP) server written in Rust 1.95 that exposes POSIX baseutils-equivalent OS management to LLM agents. Org: `com.archanjo`. Transport: STDIO only (no socket server, no HTTP/SSE). License: MIT/Apache-2.0 dual.

**Current phase: active implementation** (17-crate Cargo workspace, v0.2.0). The spec tree at `doc/arch/` is the source of truth — read it before changing code. Where spec and code contradict, the code is ground truth; raise a spec-correction PR alongside any code change.

**Target platforms**: macOS and Linux, both actively built/clippy'd/tested (commands in [Commands](#commands)). Linux has no local dev machine in this project's usual workflow — verify Linux-only (`#[cfg(target_os = "linux")]`) changes via Docker (`rust:1.95.0` image) on a remote host before assuming they compile; several such code paths (path jail via `openat2`, fs-index/fs-query statx-tier walkers, procfs-based process/system-info readers) went uncompiled on real Linux for a long time and had genuine bugs (API drift, `nix`/`libc` field-width divergence between platforms, an `openat2(2)` `RESOLVE_BENEATH` absolute-path bug) that only surfaced once actually exercised there. **Never remove a clippy-flagged "redundant" cast on one platform without checking the field's width on the other** — `nix::sys::stat::Mode`/`mode_t` is `u32` on Linux but `u16` on macOS/BSD; `nix::sys::statvfs::Statvfs::blocks_available()` is `u64` on Linux but `u32` on macOS/BSD. The established idiom for a genuine divergence is `#[cfg_attr(target_os = "linux", expect(clippy::..., reason = "..."))]`, never a bare removal.

## Commands

Development runs in the `dev` profile, always: unoptimized workspace crates,
incremental compilation, full debug info, and sccache behind `scripts/rustc-wrapper`
as the `rustc` wrapper. `cargo build` and `cargo test` pick that profile up with no
flag. The `release` profile (LTO, `panic = "abort"`, strip) is for deploy artifacts
only — reach for `just build-release` when producing a binary, not while iterating.

```shell
cargo build --workspace --all-targets        # dev profile, incremental, sccache-wrapped
cargo nextest run --workspace -E 'not binary(cucumber)' --no-fail-fast
cargo test -p substrate-mcp-server --test cucumber   # nextest cannot list this harness
cargo test --workspace --doc                 # doctests, which nextest does not run
cargo clippy --workspace --all-targets -- -D warnings
```

The cucumber harness is excluded from the nextest pass because its clap CLI rejects
the `--list` probe nextest uses to enumerate tests, and doctests follow for the same
reason. Both run on cargo's own harness.

`just` and `make` wrap the same lanes plus the CI-only ones: `just test` (nextest plus
doctests), `just test-serial` when interleaved output matters, `just ci-fmt`,
`just ci-clippy`, `just ci-nextest`, `just ci-deny`, `just ci-spec`, `just ci-mermaid`,
`just ci-typos`, `just ci-shear`, aggregated by `just ci`. Release binaries:
`just build-release` (default features) and `just build-release-launch` (with `launch`).

Spec tooling runs through `speckit`: `status`, `next`, `validate` (`--deep` adds the external validators), `verify`, `guard check`, `diagram`. The next section says when each applies.

Full `speckit` surface, for reference — every verb accepts `--json` and returns a stable exit code:

`speckit init`, `speckit constitution`, `speckit specify`, `speckit clarify`, `speckit plan`, `speckit plan setup`, `speckit tasks`, `speckit tasks setup`, `speckit analyze`, `speckit implement`, `speckit feature new`, `speckit feature list`, `speckit feature select`, `speckit feature archive`, `speckit feature compact`, `speckit feature insert`, `speckit feature renumber`, `speckit feature reorder`, `speckit feature restore`, `speckit status`, `speckit next`, `speckit check`, `speckit validate`, `speckit verify`, `speckit explain`, `speckit search`, `speckit reindex`, `speckit migrate`, `speckit generate`, `speckit guide`, `speckit manual`, `speckit context score`, `speckit context pack`, `speckit spec score`, `speckit dedupe`, `speckit stats corpus`, `speckit stats findings`, `speckit stats guard`, `speckit stats profile`, `speckit stats attributes`, `speckit stats compliance`, `speckit stats recommendations`, `speckit semantic status`, `speckit semantic enable`, `speckit semantic off`, `speckit semantic eval`, `speckit semantic deep-status`, `speckit model list`, `speckit model add`, `speckit model fetch`, `speckit model select`, `speckit model check`, `speckit model remove`, `speckit model api`, `speckit pack list`, `speckit pack add`, `speckit pack remove`, `speckit pack export`, `speckit pack import`, `speckit pack update`, `speckit library list`, `speckit library add`, `speckit library show`, `speckit library search`, `speckit library browse`, `speckit library open`, `speckit library ask`, `speckit library extract`, `speckit library import`, `speckit library export`, `speckit library serve`, `speckit library validate`, `speckit library update`, `speckit library remove`, `speckit mermaid render`, `speckit workflow render`, `speckit diagram`, `speckit guard check`, `speckit guard hook`, `speckit hook session-start`, `speckit hook user-prompt`, `speckit hook pre-commit`, `speckit hook post-edit`, `speckit config list`, `speckit config get`, `speckit config set`, `speckit config unset`, `speckit config drift`, `speckit on`, `speckit off`, `speckit gitlab status`, `speckit gitlab sync`, `speckit license list`, `speckit license show`, `speckit license set`, `speckit license check`, `speckit completions`, `speckit version`, `speckit ask`, `speckit brief`, `speckit commit`, `speckit commit check`, `speckit commit suggest`, `speckit dismiss`, `speckit missing`.

## Agents and skills

Delegation is pinned, not improvised: when the work matches a row below, the
parent MUST spawn that `subagent_type` instead of reaching for `general-purpose`.
Pass the row's skill names through `skills_hint` so the child starts primed, and
set `capability_mode` from the row — a child that only reads has no business
holding write access.

| Work | `subagent_type` | `capability_mode` | `skills_hint` |
|---|---|---|---|
| Rust implementation, refactor, toolchain or edition migration | `rust-engineer` | `all` | `rust` |
| Review of a diff or a pull request | `code-reviewer` | `read-only` | — |
| Layering, crate-boundary, and ADR-conformance audit | `architect-reviewer`, `arch-advisor` | `read-only` | — |
| Security review of the path jail, allowlist, and redaction | `code-reviewer` | `read-only` | — |
| Failure triage and root cause | `debugger`, `error-detective` | `execute` | — |
| Latency and RSS budget work (ADR-0030) | `performance-engineer` | `execute` | — |
| Test strategy and coverage gaps | `qa-expert` | `execute` | — |
| Gherkin scenarios and cucumber step definitions | `test-automator` | `all` | — |
| rmcp wiring, protocol versioning, elicitation | `mcp-developer` | `read-write` | `rfc` |
| Spec and ADR authoring under `doc/arch/` | `documentation-engineer` | `read-write` | `glfm` |
| Independent second opinion from a non-Claude model | `codex` | `read-write` | `codex` |
| Linux-only verification in Docker on the remote host | `general-purpose` | `execute` | `linux`, `ssh` |

Skills for the working session, loaded on the matching trigger:

- `substrate` — local filesystem, process, network, and system inspection; prefer it over shelling out.
- `linux` — Linux semantics whenever a `#[cfg(target_os = "linux")]` path is in play.
- `ssh` — the remote Docker host used to compile and test Linux-only code.
- `rfc` — MCP protocol and RFC questions.
- `glfm` — CommonMark, GFM, and Mermaid when authoring under `doc/arch/`.
- `codex` — the cross-LLM dispatch envelope and its profile rules.
- `arithma` — every arithmetic, never inline.
- `archive` — archive-format work; routes to `tar-archive`, `zip-archive`, `zstd-archive`.
- `skill-schema` — when touching the companion skill (ADR-0046).

Rules:

- Work outside the table falls back to `general-purpose`; spawn it `read-only` unless it must write.
- Do not spawn `general-purpose` for work a row above already covers.
- A child that only reads MUST be spawned `read-only`; `read-write` is the exception, not the default.
- Two children editing in parallel MUST NOT share a working tree — pass `isolation: "worktree"`.
- Omit `model` so the child inherits the parent's route; reach for `search_models` only when a specific model was named.
- The off-stack agents in `/Users/farchanjo/.grok/agents/` — `electron-pro`, `flutter-expert`, `laravel-specialist`, `django-developer`, `swift-expert`, `php-pro`, `angular-architect`, `vue-expert`, `dotnet-core-expert`, `powershell-7-expert` — are for work that actually touches those stacks. Do not spawn them for Rust-side work.
- Children receive a compacted copy of this file, so the spec-first protocol reaches them; do not restate it in the spawn prompt.
- Agent definitions live in `/Users/farchanjo/.grok/agents/`; a `subagent_type` that does not resolve there fails the spawn.
- `security-auditor` is referenced by several agent bodies but does not exist — route security work to `code-reviewer`.

## Spec-first protocol

This repo is spec-first: read the spec before writing code. The driver is `speckit` (`/Users/farchanjo/bin/speckit`), a self-contained, AI-agnostic Spec-Driven Development (SDD) CLI that replaced the legacy Python `spec` framework. All spec artifacts live under `doc/arch/`, validated by `speckit` in place; SDD lifecycle state lives under `.specify/` (feature registry, guard policy) and `doc/arch/memory/` (constitution) — never hand-edit the state files (`config.toml`, `features.json`, `guard-policy.toml`, `guard-audit.jsonl`, `doc/arch/memory/constitution.md`), drive them through the `speckit` subcommands instead.

Lifecycle: `constitution -> specify -> clarify -> plan -> tasks -> analyze -> implement`. Orient with `speckit status` (where the spec system is) and `speckit next` (recommended next step); `speckit check` verifies prerequisites/project health.

```shell
# Native validators (fast, no external deps) — run on every save / pre-commit
speckit validate

# Adds external validators (conftest/vale/Structurizr CLI/TLC) — CI gate
speckit validate --deep

# Executable Gherkin corpus (ADR-0012) against this binary
speckit verify
```

`speckit validate --list` prints the validator catalog; `speckit verify --filter <substr>` scopes to matching scenario names; `speckit diagram` renders the Structurizr model to Mermaid; `speckit guard check` gates file writes against the SDD scope policy.

When changing any file under `doc/arch/`, run `speckit validate` before committing. CI gates on `speckit validate --deep`.

## Spec layout

```text
doc/arch/
  adr/                       MADR 4.0 decision records (0001–0072)
  architecture/workspace.dsl Structurizr DSL (C4 context + container views)
  asyncapi/                  AsyncAPI contract for the notification stream
  cue.mod/module.cue         CUE module: com.archanjo/substrate
  domain/<bc>/README.md      Bounded-context narratives (10 BCs; launch implemented)
  formal/                    TLA+ models (JobRegistry.tla + .cfg; TLC run output ignored)
  operations/operator-guide.md  Operator-facing guide
  policies/*.rego            Open Policy Agent rules (34 policies)
  runbooks/                  Debug runbooks (launch)
  sdd/                       Per-feature SDD artifacts, written by `speckit specify`
  schemas/*.cue              CUE schemas (15 files, all with DDD role headers)
  slo/*.yaml                 SLI/SLO definitions per ADR-0039
  specs/features/<area>/     Gherkin feature specs (167 features)
  specs/tool_cards/          Tool-card fixtures
  styles/Substrate/          Structurizr style definitions
  threat-model/README.md     Threat model (LINDDUN privacy model still pending)
  glossary.md                Ubiquitous-language vocabulary
  README.md                  Architecture-spec entry point
  .specconfig.yml            Legacy spec-framework config; superseded by .specify/config.toml, no longer consulted
```

```text
.specify/
  config.toml               speckit bridge config (Python spec-framework escape hatch, disabled by default)
  features.json             feature registry state (managed by `speckit feature` / `speckit specify`)
  guard-policy.toml         Guard scope-of-writes policy (managed by `speckit guard`)
  guard-audit.jsonl         Guard decision audit log
```

The constitution lives with the spec, at `doc/arch/memory/constitution.md`.

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
10. **launch** *(ADR-0063..0070; implemented including Milestone 2 detached supervisor, on both Linux and macOS)* — declarative process orchestration over subprocess: launch.init/list/trust/up/status/logs/restart/reload/down/forget (10 tools), gated behind Cargo feature `launch` (default-off, implies `subprocess` **and** the `outbound-net` feature on `crates/substrate-subprocess`). Readiness gating is real (ADR-0056/0065 amendments, 2026-07-01): a probe-gated Service is born `Starting` and only reported `Ready` once its `PortOpen`/`HttpGet` health probe passes (the subprocess supervisor polls and promotes `Starting -> Ready`); `wait_ready` uses a per-probe budget, not the old fixed 1s ceiling; a Service that never becomes ready is stopped. `launch` implies `outbound-net` because those probes are inert without it. Service `command[0]` may be absolute, `cwd`-relative, or a bare name resolved on `$PATH`, resolved to an absolute path before the spawn while the binary allowlist stays the execution gate (ADR-0070). A Service may load `.env` files via `env_file` (paths relative to the profile dir, no escape; later file > earlier, inline `env` > files; ADR-0071)

Tools are namespaced `<bc>.<verb>` (e.g., `fs.find`, `proc.signal`). Total `tools/list` count is 61 with the `launch` feature enabled (51 without). Each BC maps to a Cargo crate under `crates/substrate-*` (see ADR-0022). The `substrate-launch` crate (ADR-0063) is a workspace member, gated behind the default-off Cargo feature `launch`. Its detached-supervisor mode (ADR-0068) is fully built: `LaunchRegistry::up` forks a `substrate --supervise <stack_id>` child on `on_client_disconnect = detach`, polls its durable `supervisor.json`, and a fresh MCP server reaps/re-attaches any Stack left behind by a prior session at startup. See ADR-0068's amendments for the three deliberate deviations from its literal design (tokio `select!` reactor instead of hand-rolled `mio`; poll-based child-exit instead of `pidfd`/`kqueue`; macOS pgid+reaper-on-boot instead of watchdog-pipe cooperation for arbitrary children). The pure-domain shared kernel lives in `crates/substrate-domain` and MUST NOT import any infra crate (hexagonal layering enforced via `doc/arch/policies/hexagonal_layering.rego`).

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

## Architecture: workspace layout and layering

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

Hexagonal layering rule: `substrate-domain` imports only std + serde + thiserror + async-trait + futures + uuid + tracing. Adapter crates depend on `substrate-domain` (+ `substrate-policy` for write-paths), never on each other. Only `substrate-mcp-server` depends on rmcp and tokio with `net` (if `outbound-net` feature on). Enforced by `doc/arch/policies/hexagonal_layering.rego`.

## Reading order for a new contributor

When picking up this repo cold, read in this order:

1. `doc/arch/README.md` — project overview
2. `doc/arch/glossary.md` — ubiquitous language
3. `doc/arch/adr/0002-bounded-contexts.md` — strategic DDD
4. `doc/arch/adr/0007-tool-card-narrative-arc.md` — tool design template
5. `doc/arch/adr/0004-security-model.md` + ADR-0035 — security layers
6. `doc/arch/adr/0003-crate-stack-and-async-zones.md` — Rust stack + async zones
7. `doc/arch/architecture/workspace.dsl` — C4 model (render with Structurizr CLI or Lite)
8. `doc/arch/domain/*/README.md` — BC you intend to touch

For implementation work later: read the ADRs cross-referenced from the relevant BC README, then the matching CUE schemas under `doc/arch/schemas/`, then the matching Gherkin features under `doc/arch/specs/features/*/`.

For the launch BC specifically, ADR-0063 (bounded context), ADR-0064 (profile trust model), ADR-0065 (dependency graph + reconciler/reload; 2026-07-01 amendment: readiness gating made real + per-probe budget + `launch` implies `outbound-net`), ADR-0066 (event stream), ADR-0067 (concurrency/messaging topology), ADR-0068 (detached supervisor + orphan governance), ADR-0069 (tool cards + ToolSearch discoverability), ADR-0070 (PATH binary resolution), and ADR-0071 (`.env` file support) form one connected design — read them together, in that order. The health-probe wiring these depend on is in ADR-0056 (2026-07-01 amendment: `Starting -> Ready` edge + probe supervisor).

## Spec conventions (enforced by linters)

- All artifacts en-US. Spec markdown uses CommonMark + Mermaid diagrams (per ADR-0047). GFM tables, emojis, and task lists remain disallowed in spec markdown. Mermaid is MANDATORY where a diagram aids comprehension (flowchart, sequence, state, ER, class, gantt, pie, gitGraph, mindmap, timeline, C4). ASCII art is retained only when Mermaid cannot render the intended shape.
- ADR filenames: `NNNN-kebab-case-slug.md`. ADR numbers never reused; superseded ADRs link forward.
- CUE filenames: `snake_case.cue`. CUE definitions: `#PascalCase`. Every CUE file requires header `// DDD role: <AggregateRoot|Entity|ValueObject|DomainService|ReadModel>`.
- Gherkin filenames: `kebab-case.feature`. One scenario per behavior.
- Rego packages: `substrate.<area>`.
- Cross-ref ADRs via relative markdown links: `[ADR-NNNN](NNNN-slug.md)`.
- UUIDv7 only.
- pnpm/npm FORBIDDEN. `speckit` is a self-contained Rust binary — no Python/uv dependency for day-to-day SDD workflow. `.specify/config.toml`'s `[bridge]` section is a disabled-by-default escape hatch to the legacy Python spec-framework (uv-managed) and should stay off.

## Commit / branch conventions

- Angular format: `<type>(<scope>): <subject>` where types are `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `ci`, `chore`, `perf`, `style`, `security`. Scopes match crate names (`fs-query`, `process`, `mcp-server`, `launch`, etc.) or `adr` for ADR-only changes. See ADR-0024.
- Small contextual commits — never bulk "various changes".
- Branch naming: `feat/*` or `fix/*`, where the remainder is the scope plus a short description; `chore/*` for housekeeping.
- DCO sign-off required (`Signed-off-by:` trailer, `git commit -s`). No CLA.

## Implementation guidance

Rule: read a spec artifact under `doc/arch/` before editing source code within a session. Claude Code enforces this through the optional `impl-guard` hook, with a single-shot `/impl-ok` override when a deviation is explicitly justified; harnesses without hooks apply the same rule by convention.

The workspace is bootstrapped and active. When adding a new BC or adapter, the recommended onboarding order is:

1. Confirm the relevant BC ADR and CUE schema are up-to-date with the code before changing anything.
2. Work in `substrate-domain` for new port traits / value objects / error codes. Round-trip with `schemas/*.cue` definitions.
3. Work in `substrate-policy` for allowlist / path jail changes. Test against `doc/arch/specs/features/filesystem-query/fs-find-path-traversal-blocked.feature` and ADR-0035 scenarios. Path-jail changes are security-critical — read the full existing implementation for the platform(s) you touch before editing, and verify both the macOS (`ONoFollowAnyJail`) and Linux (`Openat2Jail`) tiers stay behaviorally consistent (same absolute-path calling convention, same NFC-normalized containment check).
4. Adjust `substrate-config` (figment + TOML, `deny_unknown_fields`, allowlist canonicalization at startup) for any new configuration surface.
5. Implement adapter changes BC-by-BC. For each adapter, validate against corresponding Gherkin features (executable via `cucumber-rs` in `crates/substrate-mcp-server/tests/`). Note: the cucumber suite has a known, pre-existing gap of undefined step definitions for some `text.head`/`text.tail` and other scenarios (steps referenced in `.feature` files with no matching `#[given]`/`#[when]`/`#[then]` regex) — this is a step-coverage gap from the project's spec-first workflow, not a regression signal; don't assume every cucumber failure is your change's fault, but don't silently paper over new ones either.
6. Wire `substrate-mcp-server` composition root for any new service surface (rmcp service, signal handlers per ADR-0032, capability negotiation per ADR-0013).

Every adapter implementation must obey async-zone classification (A/B/C) declared in ADR-0003 and the cancellation patterns in ADR-0037. Use `criterion` benchmarks per ADR-0030 to verify performance budgets; CI fails on >15% regression.
