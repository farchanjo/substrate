---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0003 — Crate Stack and Async Zone Strategy

## Context and Problem Statement

`substrate` is an MCP server that exposes OS management primitives (filesystem traversal, process inspection, archiving, hashing, text search) to LLM agents. Each primitive has a different concurrency profile: some are natively async (I/O), some block the OS scheduler (sync file reads, regex scanning), and some are CPU-saturating (hashing large files, archive compression). Mixing these profiles naively on the tokio executor causes head-of-line blocking and starvation.

We need a dependency set that covers all required primitives, a principled mapping of each crate to its execution zone, and a Cargo feature design that keeps the binary minimal by default while allowing optional networking.

## Decision Drivers

- Rust 1.95 stable; edition 2024; no nightly features.
- `rmcp` transport-io constrains us to stdio; no HTTP/SSE transport.
- tokio is already the async runtime; every async primitive must integrate with it without launching a second runtime.
- Blocking and CPU-bound work must not starve the tokio executor thread pool.
- Supply-chain hygiene: `cargo-deny`, `cargo-audit`, `cargo-machete` are mandatory in CI.
- Binary size and startup time matter for a stdio-launched server; default features must be minimal.

## Considered Options

1. Zone A (async-native) for all I/O; spawn_blocking for the remainder — accepted.
2. Use a dedicated rayon thread pool for CPU work alongside tokio — rejected; two thread pools for file I/O cause cache thrashing and complicate cancellation.
3. Use `async-std` instead of tokio — rejected; `rmcp` explicitly targets tokio; mixing runtimes causes `tokio::spawn` panics.
4. Use `uring` feature unconditionally — rejected; io_uring is Linux-only and not universally available; gate it behind an explicit `linux-io-uring` Cargo feature.

## Decision Outcome

Chosen option: "Zone A/B/C with spawn_blocking and Semaphore", because it integrates cleanly with the tokio multi-thread scheduler, provides back-pressure through Semaphores, and keeps all cancellation paths under a single `CancellationToken` (see ADR-0006).

### Async Zone Taxonomy

| Zone | Label | Mechanism | When |
|------|-------|-----------|------|
| A | async-native | `tokio::fs`, `tokio-tar`, `async-compression`, `async_zip` | Awaitable I/O; no thread needed |
| B | sync I/O | `tokio::task::spawn_blocking` | Blocking syscalls: `sysinfo`, `procfs`, `faccess`, sync reads |
| C | CPU-bound | `spawn_blocking` + `Semaphore(num_cpus)` | `blake3` (mmap/rayon), `sha2`, `regex` scanning |

### Full Crate Inventory

#### MCP transport
| Crate | Version | Features | Zone | Rationale |
|-------|---------|----------|------|-----------|
| `rmcp` | 1.7.x | `server, transport-io, macros` | A | Stdio transport for LLM agent communication; macro derive for tool registration |

No `transport-sse` or `transport-streamable-http` — adds hyper/axum surface area not needed for a local MCP server.

#### Async runtime
| Crate | Version | Features | Zone | Rationale |
|-------|---------|----------|------|-----------|
| `tokio` | 1.4x | `rt-multi-thread, macros, io-std, io-util, fs, process, signal, sync, time` | A/B/C | Work-stealing executor; `net` excluded from default |
| `tokio-util` | latest | `rt, io` | A | `CancellationToken`, codec utilities |
| `tokio-stream` | latest | — | A | Stream adapters for async iteration |
| `async-trait` | latest | — | A | AFIT not yet stable for `dyn`-compatible async traits in rmcp derive |
| `futures` | latest | — | A | `StreamExt`, `FutureExt` combinators |

#### Filesystem
| Crate | Version | Features | Zone | Rationale |
|-------|---------|----------|------|-----------|
| `tokio::fs` | (tokio) | — | A | Async file I/O; wraps OS async internally |
| `ignore` | latest | — | B | Gitignore-aware directory walker; sync iterator — spawn_blocking |
| `globset` | latest | — | B/C | Compile patterns at config time; match in-line |
| `faccess` | latest | — | B | Portable permission check without opening FD |

Path validation uses `std::path::Path::canonicalize` with a soft fallback (lexical normalization) when the path does not yet exist. Traversal is sandbox-checked against a configurable root prefix (see ADR-0009).

#### Process and system information
| Crate | Version | Features | Zone | Rationale |
|-------|---------|----------|------|-----------|
| `sysinfo` | latest | — | B | Cross-platform process/memory/CPU snapshot; sync API |
| `procfs` | latest | — | B | Linux `/proc` parsing; enabled via `#[cfg(target_os = "linux")]` |
| `nix` | latest | — | B | POSIX signal delivery, UID/GID inspection |

#### Text and search
| Crate | Version | Features | Zone | Rationale |
|-------|---------|----------|------|-----------|
| `regex` | latest | — | C | PCRE-like patterns; compilation cached; matching is CPU-bound |
| `grep-searcher` | latest | — | C | Line-oriented searcher with context; mirrors `ripgrep` internals |
| `grep-regex` | latest | — | C | `grep-searcher` regex matcher adapter |
| `memchr` | latest | — | C | SIMD-accelerated byte search used by grep-searcher |

#### Archiving
| Crate | Version | Features | Zone | Rationale |
|-------|---------|----------|------|-----------|
| `async-compression` | latest | `tokio, gzip, zstd, deflate` | A | Stream-based compression wrapping `AsyncRead`/`AsyncWrite` |
| `async_zip` | latest | `tokio, deflate` | A | ZIP read/write without blocking |
| `tokio-tar` | latest | — | A | TAR streaming; async-native |

#### Hashing
| Crate | Version | Features | Zone | Rationale |
|-------|---------|----------|------|-----------|
| `blake3` | latest | `rayon, mmap` | C | Memory-mapped parallel hashing; CPU-saturating — Semaphore-gated |
| `sha2` | latest | — | C | SHA-256/SHA-512; CPU-bound |

#### Observability
| Crate | Version | Features | Zone | Rationale |
|-------|---------|----------|------|-----------|
| `tracing` | latest | — | A/B/C | Structured spans across all zones |
| `tracing-subscriber` | latest | — | — | Writes to stderr; never stdout (reserved for MCP transport) |

#### Errors and utilities
| Crate | Version | Features | Zone | Rationale |
|-------|---------|----------|------|-----------|
| `thiserror` | latest | — | — | Library-layer typed errors |
| `anyhow` | latest | — | — | Application-layer error context chains |
| `uuid` | latest | `v7` | — | Time-ordered UUIDs for request correlation |
| `num_cpus` | latest | — | — | Semaphore sizing for Zone C |

#### Configuration
| Crate | Version | Features | Zone | Rationale |
|-------|---------|----------|------|-----------|
| `figment` | latest | — | — | Layered config (env → TOML file → defaults) |
| `toml` | latest | — | — | TOML deserializer for figment |

### Cargo Feature Design

```toml
[features]
default = []
outbound-net = ["tokio/net"]
linux-io-uring = []
```

- **`default = []`**: minimal binary; no network stack compiled in.
- **`outbound-net`**: enables `tokio/net` for optional DNS/HTTP calls that tools may need in future. Gated to prevent accidental network access from a local MCP server.
- **`linux-io-uring`**: reserved flag; enables io_uring code paths when stabilized. Currently a no-op; Linux-only build guard via `#[cfg(feature = "linux-io-uring")]` + `#[cfg(target_os = "linux")]`.

### Consequences

#### Positive

- All blocking work isolated from the async executor; no starvation of MCP message dispatch.
- Cargo features keep the default binary free of `tokio/net` — defense-in-depth against unintended outbound connections.
- Single cancellation primitive (`CancellationToken`) covers all three zones.
- `cargo-machete` will catch any unused crate immediately.

#### Negative

- `async-trait` remains a dependency until AFIT is stable enough for `dyn`-compatible use in rmcp derive macros.
- `procfs` adds a Linux-only path; `#[cfg]` guards increase compile-matrix complexity.
- `blake3` with `rayon` transitively pulls in rayon's thread pool, which runs alongside tokio. Rayon is bounded and non-async so there is no executor conflict, but the two pools must be sized carefully (see ADR-0017).

## Validation

- `cargo build --all-features` must succeed on Rust 1.95.
- `cargo deny check` must pass with no licenses blocked and no known advisories.
- `cargo machete` must report zero unused dependencies.
- Zone classification is enforced by code review: any `std::fs` or blocking call outside a `spawn_blocking` closure is a CI lint violation (`clippy::disallowed_methods` configured in `Cargo.toml` `[lints]`).

## Cross-References

- ADR-0006: tokio runtime configuration, timeout, and cancellation details.
- ADR-0017: Semaphore sizing strategy for Zone C.
- ADR-0028: workspace crate layout and where each zone lives.

## Amendments

### 2026-05-21 — Extended by ADR-0040 async-job-control-plane

ADR-0040 introduces a mandatory async job control-plane for long-running tool invocations. This amendment restricts synchronous Zone C execution on the request path and permits Zone B promotion to async-job execution when inputs exceed inline thresholds.

**Additions:**

- Zone C (CPU-bound) tools MUST NOT execute synchronously on the tokio request path. Every Zone C tool invocation must be dispatched through the async job control-plane defined in ADR-0040, receiving a `job_id` (UUIDv7) and returning immediately with `job_state: pending`. Synchronous Zone C request-path execution is forbidden because it blocks the tokio scheduler beyond the per-request budget and violates back-pressure guarantees.
- Zone B (sync I/O via `spawn_blocking`) tools MAY be promoted to async-job execution when the input payload exceeds the per-tool inline threshold declared in runtime config. The threshold is expressed as a bucket-B size limit per tool card; tools below the threshold remain synchronous Zone B; tools above it are re-dispatched as async jobs.

### 2026-05-21 — Extended by ADR-0043 simd-runtime-dispatch

ADR-0043 introduces SIMD runtime dispatch to accelerate CPU-bound adapter paths. This amendment grants a narrow, scoped exception to the `forbid(unsafe_code)` invariant previously cross-referenced from ADR-0014.

**Additions:**

- Each adapter crate MAY contain a dedicated `simd_impl` sub-module that uses unsafe Rust intrinsics to implement SIMD-accelerated code paths. This exception is confined to `simd_impl` modules only; all other modules in every crate retain the `forbid(unsafe_code)` invariant without exception.
- SIMD intrinsic wrappers in `simd_impl` modules MUST route through the `OnceLock`-cached `SimdTier` selection established at startup per ADR-0042. Direct runtime CPUID probing outside the `SimdTier` mechanism is forbidden.
