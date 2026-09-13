---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0014 — Build System and Toolchain

## Context and Problem Statement

`substrate` must compile deterministically on developer workstations and CI with a pinned Rust version. Supply-chain hygiene, binary reproducibility, and fast iteration cycles are all requirements. The project must never silently upgrade the compiler or pull in a git-sourced dependency on the main branch.

## Decision Drivers

- Rust 1.95 is the minimum and pinned version; no nightly features allowed.
- Developers use `mise` for toolchain management; the pin must be machine-readable and enforced by `mise install`.
- CI must catch supply-chain issues (licenses, advisories, unused deps) automatically.
- Build profiles must balance debug speed, release performance, and debuggability in production.
- No Node.js/pnpm/npm/yarn in the build graph — this is a pure Rust project.

## Considered Options

1. `mise.toml` pin + `rust-toolchain.toml` fallback + Cargo-only build — accepted.
2. `rustup override set` per-directory — rejected; not machine-readable, not committed to the repository, lost on clone.
3. Docker-based build via `cross` — considered for cross-compilation but not adopted as the primary build path; `cross` is a CI-only tool for ARM/musl targets.
4. `cargo xtask` for build orchestration — deferred; the current task surface is small enough for a `Makefile` or `cargo` invocations directly.

## Decision Outcome

Chosen option: "`mise.toml` + `rust-toolchain.toml` + Cargo profiles", because it provides a single source of truth for the toolchain version, is enforced by `mise install` on workstations and `mise exec` in CI, and requires no additional runtime dependencies.

### Toolchain Pin

`mise.toml` (repository root):

```toml
[tools]
rust = "1.95.0"
```

`rust-toolchain.toml` (repository root, fallback for non-mise environments):

```toml
[toolchain]
channel = "1.95.0"
components = ["rustfmt", "clippy", "rust-src"]
```

Both files must stay in sync. A CI check compares the version strings; a mismatch is a build failure.

### Cargo Profiles

```toml
[profile.dev]
opt-level = 0
debug = true
incremental = true

[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"

[profile.release-with-debug]
inherits = "release"
debug = true
strip = "none"
```

- **`dev`**: fast incremental compilation; full debug info.
- **`release`**: LTO + codegen-units=1 for maximum code quality; `panic = "abort"` removes unwinding machinery; `strip = "symbols"` minimizes binary size.
- **`release-with-debug`**: production profiling; retains debug info for `flamegraph`/`samply`; never shipped to end users.

### Required CI Tools

The following must be installed and pass in every CI run targeting the main branch:

| Tool | Purpose | Fail condition |
|------|---------|----------------|
| `cargo-deny` | License + advisory + bans check | Any denied license, known advisory, or banned crate |
| `cargo-audit` | RustSec advisory database | Any unfixed advisory |
| `cargo-machete` | Unused dependency detection | Any unused `[dependencies]` entry |
| `cargo fmt --check` | Style | Any formatting delta |
| `cargo clippy -D warnings` | Lint | Any warning |
| `cargo nextest run` | Tests | Any test failure |

`cargo-binstall` is used in CI to install tool binaries without compiling from source, reducing CI wall time.

### Reproducible Builds

- No `git = "..."` dependencies on the `main` branch. Pull requests adding git deps require explicit reviewer sign-off and a tracking issue to replace with a crates.io release.
- `Cargo.lock` is committed (binary application, not a library). `cargo update` bumps are reviewed in a dedicated PR.
- The `[patch]` table in `Cargo.toml` must be empty on main; temporary patches are only allowed on feature branches.
- `SOURCE_DATE_EPOCH` is set in CI to the commit timestamp for reproducible archives.

### No Node.js/pnpm

The repository contains no `package.json`, `pnpm-lock.yaml`, `.nvmrc`, or any Node.js artifact. CI pipelines must not install Node.js. Any documentation tooling that requires Node must be run in an isolated container that never touches the Rust build cache.

### Panic Semantics

The release profile sets `panic = "abort"`. Consequences:

- `std::panic::catch_unwind` is a no-op under abort; it will not catch a panic in `spawn_blocking` closures or anywhere else. Code that relies on `catch_unwind` for error recovery must not be used.
- Stack unwinding does not run; RAII `Drop` impls inside a panicking thread are NOT called. Any resource acquired inside a `spawn_blocking` closure (file handles, permits, buffers) will not be released if the closure panics.
- Semaphore permits MUST live in the surrounding async scope, not inside `spawn_blocking` closures, to ensure they are released by the async executor's drop path rather than by unwind. See [ADR-0037](0037-async-cancellation-patterns.md) for the canonical permit lifetime rule.
- `spawn_blocking` closures are expected to be short-lived and panic-free. Any panic in a blocking closure aborts the entire process immediately.

### CI Tool Version Pinning

The following tools are pinned at specific versions in `mise.toml` and installed in CI via `mise install` rather than `cargo install`. This prevents silent version drift and ensures reproducible CI behavior across developer workstations and CI runners:

```toml
[tools]
"cargo:cargo-deny"     = "0.16.4"
"cargo:cargo-audit"    = "0.21.2"
"cargo:cargo-nextest"  = "0.9.96"
"cargo:cargo-machete"  = "0.7.0"
"cargo:cargo-tarpaulin" = "0.32.7"
"cargo:cargo-geiger"   = "0.12.0"
```

Tool versions are updated in a dedicated PR that includes a changelog review. A CI job validates that the installed versions match the pinned versions in `mise.toml`.

### Transitive Unsafe Auditing

`cargo geiger --all-features` runs as an informational CI step (non-blocking on main; blocking on PRs that add new dependencies). It reports the count of `unsafe` blocks in each crate, including transitive dependencies.

The following transitive unsafe blocks are expected and approved:

| Crate | Reason |
|---|---|
| `blake3` | SIMD acceleration for hashing |
| `sha2` | SIMD acceleration for hashing |
| `memchr` | SIMD-accelerated byte scanning |
| `nix` | Thin syscall bindings over libc |
| `libc` | C FFI bindings |
| `tokio` | `mio` I/O integration; task scheduler internals |

Any unsafe block in a transitive dependency not listed above is flagged for review in the PR that introduces the dependency. The allowlist is maintained in `geiger-allow.toml` at the repository root. Adding to the allowlist requires explicit reviewer sign-off.

### Consequences

#### Positive

- Single `mise install` brings any developer to a ready state.
- `cargo-deny` catches license incompatibilities and known CVEs before they reach main.
- `cargo-machete` prevents dependency drift that inflates compile times and supply-chain surface.
- LTO + codegen-units=1 in release gives the optimizer full visibility across crate boundaries, enabling inlining of hot MCP dispatch paths.

#### Negative

- `lto = "fat"` significantly increases link time for release builds. Developers must use the `dev` profile locally and only trigger release builds in CI.
- Pinning to exactly 1.95.0 (not a channel range) means toolchain updates are a deliberate PR rather than automatic.
- `cargo-machete` may produce false positives for `proc-macro` crates or crates imported only through `#[cfg(...)]` guards; these require `# allow` comments in `Cargo.toml`.

## Validation

- `mise install` on a clean macOS and Linux environment must produce a `rustc --version` matching `1.95.0`.
- CI must fail if `cargo-deny check` emits any error.
- CI must fail if `cargo-machete` reports any unused dependency.
- A PR that adds a `git = "..."` dep must be blocked by a CI job that greps `Cargo.toml` for the pattern.

## Cross-References

- ADR-0003: crate inventory; the full dependency list that `cargo-deny` and `cargo-machete` validate against.
- ADR-0023: workspace layout; which `Cargo.toml` files exist and how `[workspace.lints]` is shared.
- [ADR-0037](0037-async-cancellation-patterns.md): permit lifetime rule; unwind-RAII is unsound under `panic = "abort"`.
