# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Commit messages follow the Angular convention defined in
[ADR-0024](docs/arch/adr/0024-repo-conventions.md).

## [Unreleased]

### Changed

- Fixed copyright holder identity in `LICENSE-MIT` and `LICENSE-APACHE`
  (Fabricio Archanjo `<farchanjo@gmail.com>`).
- Corrected workspace `repository` / `homepage` URLs in `Cargo.toml`
  (`github.com/farchanjo/substrate`) and added `authors`.
- Fixed `archanjo` → `farchanjo` GitHub handle references in
  ADR-0023 (CI release commands) and ADR-0015 (Homebrew tap).

### Added

- Community health files: `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1
  by reference), `SECURITY.md`, `CHANGELOG.md`, `.github/ISSUE_TEMPLATE/`,
  `.github/PULL_REQUEST_TEMPLATE.md`, `.github/dependabot.yml`.

## [0.1.0] — 2026-05-22

First public release. Spec-as-source-of-truth phase complete; across 276 Gherkin
scenarios, 47 pass, 148 skip (no step definition yet), and 81 fail — tracked as
known gaps under follow-up tickets.

### Added

**Architecture spec** (`docs/arch/`)

- 49 MADR 4.0 ADRs covering: bounded contexts, crate stack + async zones,
  security model, error taxonomy, MCP protocol negotiation, async job
  control-plane, filesystem index, capability adapter factory, SIMD
  runtime dispatch, no-subprocess policy, local-deploy + codesign,
  Mermaid spec diagrams, MCP Tasks primitive adoption.
- 64 Gherkin features across 7 bounded contexts plus cross-cutting
  concerns (job lifecycle, SIMD fallback, capability negotiation,
  startup contract, internal-error correlation).
- 8 CUE schemas (`#PolicyConfig`, `#ErrorEnvelope`, `#HintsMap`,
  `#JobReceipt`, `#FsIndexConfig`, `#AuditEvent`, `#SimdTier`,
  `#KernelCapabilities`).
- 6 Rego policies including `hexagonal_layering`, `no_subprocess`,
  `commit_conventions`.
- Structurizr DSL with C4 context + container views.
- TLA+ formal model for the job registry (`JobRegistry.tla`).
- 41 Mermaid diagrams across ADRs and domain READMEs (ADR-0047).

**Workspace** (`crates/`)

- Cargo workspace layout per ADR-0022 with hexagonal layering enforced
  via Rego policy: `substrate-domain` (pure ports + value objects),
  `substrate-policy` (allowlist + path jail), `substrate-config`
  (figment-based TOML loader), seven adapter crates, and
  `substrate-mcp-server` composition root.
- Rust 2024 edition, MSRV `1.85`, toolchain pinned to `1.95` via
  `rust-toolchain.toml`.
- rmcp 1.7 with `["server", "transport-io", "macros"]` features
  (no networked transports).
- Tokio 1.4x multi-thread runtime, no `net` feature unless the
  `outbound-net` Cargo feature is opted in.

**Bounded contexts and tools**

- `filesystem-query`: `fs.read`, `fs.read_dir`, `fs.find`, `fs.stat`,
  `fs.hash`, with platform-native stat tiers
  (`LinuxStatx`, `MacosGetattrlist`).
- `filesystem-mutation`: `fs.mkdir`, `fs.write`, `fs.copy`, `fs.rename`,
  `fs.remove`, `fs.set_permissions`, `fs.symlink`, `fs.touch`, with
  transactional write pattern (`<target>.tmp.<uuid7>` + atomic rename).
- `process`: `proc.list`, `proc.tree`, `proc.signal` — Linux start-time
  via `/proc/uptime` + `CLK_TCK`; macOS via `sysctl`.
- `system-info`: `sys.info`, `sys.uptime`, `sys.df`, `sys.uname`,
  `sys.hostname`, `sys.load_average`.
- `text-processing`: `text.search`, `text.count_lines`, `text.head`,
  `text.tail` with SIMD-accelerated literal search (Teddy when available).
- `archive`: `archive.tar.create`, `archive.tar.extract`,
  `archive.zip.create`, `archive.zip.extract`, `archive.gzip.compress`,
  `archive.gzip.decompress`, `archive.hash`.
- Asynchronous job control-plane (`job.list`, `job.result`,
  `job.cancel`, `job.status`) for Bucket C long-running operations,
  with idempotency-key dedup and TTL-based result GC.

**Security**

- Allowlist (TOML, default-deny) → path jail via `strict-path` +
  `openat2(RESOLVE_BENEATH | NO_SYMLINKS)` on Linux,
  `O_NOFOLLOW_ANY` on macOS.
- Dry-run mandatory for mutations; elicitation form-mode required for
  destructive ops (`fs.remove`, `fs.rename`, `fs.set_permissions`,
  `proc.signal SIGKILL/SIGTERM/SIGSTOP`, archive create/extract).
- Signal safety: `SIGPIPE` ignored at startup via dedicated
  `substrate-signal-sys` crate; blake3 mmap feature disabled to avoid
  SIGBUS on concurrent truncation.
- Cancellation: `CancellationToken` + `tokio::select! biased` with work
  as the first arm.
- Transactional writes with `statvfs` preflight disk-space guard.

**Errors and audit**

- 13 base error codes plus 6 kernel-induced and 7 startup codes; stable
  `SUBSTRATE_<UPPER_SNAKE>` form.
- Every error envelope carries `code`, `message_en_us`, `recovery_hint`
  (`≤150` chars), and `correlation_id` (UUIDv7).
- Audit-event taxonomy: structured JSON emitted to `stderr` only;
  `stdout` remains the JSON-RPC channel (ADR-0005).

**Tooling**

- `spec validate` framework wired (CUE, Conftest/Rego, Vale,
  Structurizr, MADR lint, Mermaid lint, OpenAPI lint, TLC for the
  TLA+ model).
- GitHub Actions CI workflow (`.github/workflows/ci.yml`) covering
  `cargo build / fmt / clippy / nextest` and the spec full lane.
- `mise.toml` pinning Rust + the tool versions used by the spec
  framework.

**Testing**

- Cucumber test harness in `crates/substrate-mcp-server/tests/cucumber.rs`
  with 311 step definitions matching 64 Gherkin features.
- Workspace unit suite: 278 tests passing.
- Property tests via `proptest` for system-info, text, archive,
  filesystem-mutation invariants.

### Known limitations

- 81 of 276 Gherkin scenarios are currently failing (mostly fixture
  bodies, OS-specific edges, and async cancellation timing). See
  open follow-up issues.
- macOS notarization pipeline (ADR-0045) is documented but not yet
  automated end-to-end.
- TLC model coverage is partial; only the job-registry invariants
  are checked.

[Unreleased]: https://github.com/farchanjo/substrate/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/farchanjo/substrate/releases/tag/v0.1.0
