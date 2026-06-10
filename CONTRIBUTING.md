# Contributing to substrate

Thanks for your interest. This project is spec-as-source-of-truth: every
behavioral and architectural change starts in `docs/arch/` (ADR + CUE + Gherkin
+ Rego where applicable), and only then in Rust.

## Ground rules

- Be respectful. See [Code of Conduct](CODE_OF_CONDUCT.md).
- All written artifacts in **en-US** (code, comments, commits, ADRs, docs).
- Security issues go through [SECURITY.md](SECURITY.md) — **do not** open a
  public issue for a vulnerability.

## Development workflow

### 1. Pick or open an issue

Use the GitHub issue templates. For non-trivial proposals, open a draft ADR
under `docs/arch/adr/` (see [ADR-0001](docs/arch/adr/0001-record-architecture-decisions.md))
before writing code.

### 2. Fork and branch

```bash
gh repo fork farchanjo/substrate --clone
git switch -c feat/<scope>-<short-desc>
```

Branch naming (per [ADR-0024](docs/arch/adr/0024-repo-conventions.md)):

| Type | Prefix |
|---|---|
| New feature | `feat/<scope>-<short-desc>` |
| Bug fix | `fix/<scope>-<short-desc>` |
| Chore/build/docs | `chore/<short-desc>` |

`<scope>` matches a crate name (`fs-query`, `process`, `mcp-server`, etc.) or
`adr` for ADR-only changes.

### 3. Validate the spec

Every change under `docs/arch/` must pass:

```bash
spec validate --lane fast      # ~1.5 s — runs on every save / pre-commit
spec validate                  # ~10 s — default lane
spec validate --lane full      # CI gate (13 validators)
```

See [docs/arch/README.md](docs/arch/README.md) for the spec layout and
individual linters.

### 4. Build, lint, test

Before running these, run `spec validate --lane fast` (see section 3 above) if you
touched anything under `docs/arch/`.

```bash
cargo build --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo nextest run --locked --workspace --no-fail-fast
```

### 5. Commit

Use the Angular convention from [ADR-0024](docs/arch/adr/0024-repo-conventions.md):

```text
<type>(<scope>): <subject>

<body>

Signed-off-by: Your Name <you@example.com>
```

**Allowed types:** `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `ci`,
`chore`, `perf`, `style`, `security`.

**Rules:**

- Small contextual commits — never bulk "various changes".
- DCO sign-off required (`git commit -s ...`). No CLA.
- Subject under 72 chars, imperative ("add", not "added"/"adds").
- Reference the ADR or feature file in the body when relevant.

### 6. Open a Pull Request

```bash
gh pr create --base main --fill
```

The PR template asks for a Summary, ADR/spec references, and a Test plan.
Branch protection requires the `ci-success` job to pass.

## Architectural anchors (do not re-decide without an ADR)

When implementing, treat these as locked unless a new ADR supersedes the
relevant one:

- **Stack**: Rust 1.95 (edition 2024), rmcp 1.7 (`server`/`transport-io`/`macros`
  only), Tokio 1.4x multi-thread (no `net` unless feature `outbound-net` is on).
  See [ADR-0003](docs/arch/adr/0003-crate-stack-and-async-zones.md), [ADR-0006](docs/arch/adr/0006-tokio-runtime-timeout-cancellation.md).
- **Transport**: STDIO only. `stdout` is sacred. `println!`/`print!` forbidden
  in `src/`. See [ADR-0005](docs/arch/adr/0005-stdio-transport.md).
- **Security**: allowlist → path jail (`openat2`/`O_NOFOLLOW_ANY`) → dry-run →
  elicitation. See [ADR-0004](docs/arch/adr/0004-security-model.md) and
  [ADR-0035](docs/arch/adr/0035-path-safety-hardening.md).
- **Subprocess**: an opt-in bounded context gated behind the Cargo feature
  `subprocess` (default-OFF). When enabled, every spawn is constrained by a
  binary allowlist (`subprocess.binary_allowlist`, default deny-all), PathJail
  validation of the working directory, environment filtering, and mandatory
  elicitation. The original blanket "no subprocess" ban
  ([ADR-0044](docs/arch/adr/0044-no-subprocess-policy.md)) is superseded by
  [ADR-0052](docs/arch/adr/0052-subprocess-execution-architecture.md).
- **Signal safety**: `SIGPIPE` ignored at startup; `SIGTERM`/`SIGINT` drain.
  See [ADR-0032](docs/arch/adr/0032-signal-safety.md).
- **Cancellation**: `tokio-util` `CancellationToken` + `tokio::select! biased`.
  See [ADR-0037](docs/arch/adr/0037-async-cancellation-patterns.md).
- **Error taxonomy**: `SUBSTRATE_<UPPER_SNAKE>` codes; every error carries
  `code`, `message_en_us`, `recovery_hint`, `correlation_id`. See
  [ADR-0010](docs/arch/adr/0010-error-taxonomy.md).

## Adding a new tool

1. Draft an ADR if the tool introduces new semantics or crosses a bounded
   context.
2. Add a Gherkin feature under `docs/arch/specs/features/<bc>/` capturing the
   happy path + at least one failure mode.
3. Add a CUE schema under `docs/arch/schemas/` if the tool exposes a new
   value object.
4. Implement the adapter in the matching `crates/substrate-<bc>/` crate.
5. Wire the tool in `crates/substrate-mcp-server/src/composition.rs`.
6. Run `spec validate --lane full` + the workspace test suite.

## Reporting issues

Use the **Bug report** or **Feature request** templates. Include the substrate
version (`substrate-mcp-server --version`), OS + kernel (`uname -a`), and the
relevant `correlation_id` from the stderr audit log when reporting bugs.

## Disagreement and discussion

Open a draft ADR (proposed status) or a GitHub Discussion. Architectural
decisions need consensus before merge.

## License

By contributing you agree your work is licensed under the project's dual
MIT OR Apache-2.0 license. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).
