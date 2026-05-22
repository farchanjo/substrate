---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0028 — Platform Feature Gates (macOS vs Linux)

## Context and Problem Statement

Substrate targets POSIX platforms: macOS and Linux. Windows is explicitly out
of scope. However, macOS and Linux differ substantially in how certain OS
metrics are retrieved: Linux exposes process and memory information via
`/proc` (procfs), while macOS provides equivalent data through `sysctl` and
`libproc`. A uniform implementation strategy is needed so that platform
differences are contained, auditable, and do not leak into domain code.

## Decision Drivers

- POSIX baseline must compile and pass tests on both macOS and Linux without
  conditional compilation in domain code.
- Platform-specific implementations must be localized to adapter crates.
- A future `linux-io-uring` optimization must be additive (feature flag),
  not a source-level fork.
- The `nix` crate provides a safe POSIX abstraction and is the preferred
  vehicle for portable system calls.
- Windows support is explicitly deferred; adding `cfg(windows)` blocks now
  would be misleading.

## Considered Options

- Option A: `cfg(target_os = "linux")` and `cfg(target_os = "macos")` in
  adapter crates, with a shared POSIX baseline via `nix`.
- Option B: Separate adapter crates per platform (`substrate-process-linux`,
  `substrate-process-macos`), selected by Cargo feature.
- Option C: Runtime detection of platform via `std::env::consts::OS`.

## Decision Outcome

Chosen option: "Option A — `cfg` attribute gates in adapter crates with `nix`
as the POSIX baseline", because it keeps the crate graph flat, makes platform
divergence visible at the site of divergence, and is idiomatic Rust.

### Gate Conventions

**POSIX baseline (all targets)**

System calls available on both macOS and Linux are implemented using the `nix`
crate with no `cfg` guard. Examples: `nix::unistd::stat`, `nix::fcntl::open`,
`nix::sys::signal::kill`. These form the default implementation path.

**Linux-specific (procfs)**

Process metadata (memory maps, open file descriptors, CPU time per thread)
that is cheaper to read from `/proc/<pid>/` on Linux is gated:

```rust
#[cfg(target_os = "linux")]
mod procfs_adapter { ... }
```

The `procfs` crate (or direct `BufReader` over `/proc`) is used only within
this module. The port trait remains platform-neutral.

**macOS-specific (sysctl)**

Equivalent metrics on macOS are obtained via `sysctl` calls exposed through
the `sysctl` crate or `nix::sys::sysctl`. Gated:

```rust
#[cfg(target_os = "macos")]
mod sysctl_adapter { ... }
```

**Reserved: linux-io-uring**

An optional Cargo feature `linux-io-uring` is reserved for a future
`io-uring`-based I/O path in the filesystem-query and text-processing contexts.
It is not implemented for MVP. When activated, it requires Linux 5.1+ and
enables the `tokio-uring` or `rio` backend. The feature gate is:

```toml
[features]
linux-io-uring = ["dep:tokio-uring"]
```

This feature must not be enabled in default builds. CI must verify that
`cargo check --features linux-io-uring` compiles only on Linux targets.

### Rules

1. No `cfg(target_os)` block may appear in `substrate-domain` or
   `substrate-policy`. Platform gates are adapter-layer concerns only.
2. Every platform-specific code path must have a corresponding cfg gate;
   dead code must not be suppressed with `#[allow(dead_code)]` as a workaround
   for missing platform implementations.
3. If a capability is unavailable on one platform, the adapter returns a
   `ToolResult::Err` with error kind `PlatformNotSupported`, not a panic.
4. CI must build and test on both `x86_64-unknown-linux-gnu` and
   `aarch64-apple-darwin` (or `x86_64-apple-darwin`).

### Consequences

#### Positive

- Platform divergence is localized to adapter modules; domain code is uniform.
- Adding a third POSIX target (e.g., FreeBSD) requires only a new `cfg` block
  in the affected adapters.
- `linux-io-uring` can be introduced without source-level restructuring.

#### Negative

- Adapter crates contain conditional compilation that increases reading
  complexity.
- Test coverage on one platform does not guarantee correctness on the other;
  CI on both targets is mandatory.

## Validation

- `cargo check --target x86_64-unknown-linux-gnu --workspace` must pass.
- `cargo check --target aarch64-apple-darwin --workspace` must pass.
- `grep -r "cfg(target_os" crates/substrate-domain crates/substrate-policy`
  must return no results.
- A CI matrix job must run `cargo test --workspace` on Linux and macOS runners.

## Links

- Related: [ADR-0003](0003-crate-stack-and-async-zones.md)
- Related: [ADR-0014](0014-build-system-and-toolchain.md)
- Related: [ADR-0022](0022-project-layout.md)

## Amendments

### 2026-05-21 — Extended by ADR-0041 filesystem-index-native-tiers

ADR-0041 introduces a tiered DirWalker for the optional filesystem index. The tiers depend on platform syscall availability and are controlled by two new Cargo feature flags and by the existing platform cfg-gate convention established in this ADR. Operators enable the index feature at compile time; which internal tier is selected at runtime is governed by the capability probe (see the ADR-0042 amendment below).

**Additions:**

- `fs-index` — default OFF; enables the optional in-process filesystem index backing fs.find and future query tools. Must not be enabled in default builds. CI must verify that `cargo check --no-default-features --features fs-index` compiles on both supported targets.
- `fs-index-watch` — depends on `fs-index`; enables the external-change watcher (inotify on Linux, FSEvents on macOS). Gated at adapter layer via the existing `#[cfg(target_os)]` convention; the port trait remains platform-neutral.
- `linux-iouring` — Linux-only Cargo feature enabling the io_uring-based DirWalker tier 0. The canonical feature name is `linux-iouring` (no internal hyphen between `io` and `uring`). This aligns with the reserved name `linux-io-uring` recorded in the original ADR; implementors MUST use `linux-iouring` in `Cargo.toml` to match the ADR-0041 definition. Requires Linux 5.1+ and the `tokio-uring` or equivalent backend crate.
- `macos-getattrlistbulk` — macOS-only Cargo feature enabling the tier-1 batched DirWalker via the `getattrlistbulk(2)` syscall. Must not compile or link on Linux; a `#[cfg(target_os = "macos")]` guard is mandatory at the feature entry point.

### 2026-05-21 — Extended by ADR-0042 capability-adapter-factory

ADR-0042 introduces a capability probe that runs at server startup and selects adapter implementations at runtime. This changes the relationship between Cargo features (compile-time) and adapter tier selection (runtime).

**Additions:**

- Cargo features from this ADR remain the compile-time gate: if a feature is not compiled in, the corresponding adapter code is not present in the binary and cannot be selected.
- Within the set of compiled-in features, adapter selection per port falls through probe-determined tiers based on probed OS capabilities: Linux openat2, statx, io_uring; macOS getattrlistbulk, FSEvents, kqueue, O_NOFOLLOW_ANY. The probe runs before the first tool call is accepted.
- Operators who compile a feature in but whose runtime does not satisfy the probe requirements fall through to the next-available tier automatically. The capability probe result is emitted as a structured startup log event at `tracing::info!` level.

### 2026-05-21 — Extended by ADR-0043 simd-runtime-dispatch

ADR-0043 introduces SIMD-aware Cargo features for text encoding, hashing, JSON parsing, and compression. These features follow the same additive, default-OFF convention established by this ADR for platform-specific optimizations.

**Additions:**

- `simd-baseline` (default ON) — pulls simdutf8, memchr with std backend, and blake3 with SIMD enabled (excludes mmap per ADR-0032). This is the only SIMD feature active in default builds.
- `simd-avx2` — depends on `simd-baseline`; enables blake3 AVX2 path, base64-simd AVX2 backend, and simd-json AVX2 backend. Must not be set as default; CI must test with and without this feature on x86-64 Linux.
- `simd-avx512` — depends on `simd-avx2`; enables blake3 AVX-512 path. Opt-in even on capable CPUs because of thermal throttling risk on older Intel steppings; a secondary CPUID whitelist check at runtime gates promotion beyond AVX2. CI must verify this feature compiles on `x86_64-unknown-linux-gnu` and is a no-op on `aarch64-apple-darwin`.
- `simd-neon` — depends on `simd-baseline`; enables blake3 NEON path and base64-simd NEON backend. Active only on AArch64 targets; `#[cfg(target_arch = "aarch64")]` guard is mandatory at the feature entry point.
- `fast-json` — pulls simd-json or sonic-rs (resolved at implementation time). Default OFF. Benchmark regression gate applies: CI fails on greater than 15% throughput regression vs the non-SIMD baseline.
- `fast-zlib` — pulls zlib-ng-sys for zlib-compatible decompression. Default OFF.
- `fast-deflate` — pulls libdeflater for deflate-only (non-streaming) decompression. Default OFF. Mutually exclusive with `fast-zlib` in a single binary configuration; a Cargo feature exclusion comment is required if both are enabled.
