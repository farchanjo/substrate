---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0006 — Tokio Runtime, Timeout, and Cancellation

## Context and Problem Statement

`substrate` receives MCP tool calls from an LLM agent over stdio. Each tool call may trigger async I/O, blocking syscalls, or CPU-saturating work. Without enforced timeouts and cooperative cancellation, a single stalled tool call can prevent the agent from receiving any further responses. The runtime must be configured to maximize throughput for short-lived tool calls while providing predictable latency bounds and clean resource release on cancellation.

## Decision Drivers

- The process is a stdio-attached MCP server; it must remain responsive to the transport layer while tool calls execute.
- Tools may call `tokio::fs`, `spawn_blocking`, and CPU-bound closures — all need uniform cancellation semantics.
- A hung tool call must not silently consume FDs, child processes, or memory indefinitely.
- Configuration of timeout values must be possible via TOML without recompilation.
- Cancellation must propagate across zone boundaries (async → spawn_blocking → CPU closures).

## Considered Options

1. `tokio::runtime::Builder::new_multi_thread()` with `tokio::time::timeout` per tool call and `CancellationToken` — accepted.
2. Single-threaded runtime (`new_current_thread`) — rejected; spawn_blocking requires the multi-thread scheduler to have a blocking thread pool; single-thread runtime deadlocks when all blocking permits are held.
3. `tokio::time::sleep` + manual future polling for timeouts — rejected; `tokio::time::timeout` is the idiomatic zero-overhead wrapper.
4. `SIGALRM`-based timeout — rejected; POSIX signals are not cancellation-safe in async Rust; they cannot unwind across `.await` points cleanly.
5. Per-tool `JoinHandle::abort()` without `CancellationToken` — rejected; `abort()` does not propagate into `spawn_blocking` threads; the thread continues running until the blocking operation returns naturally.

## Decision Outcome

Chosen option: "multi-thread runtime + tokio::time::timeout + CancellationToken", because it provides cooperative cancellation across all async zones, propagates into spawn_blocking threads via token polling, and integrates with the work-stealing scheduler for maximum throughput.

### Runtime Configuration

```rust
tokio::runtime::Builder::new_multi_thread()
    .enable_all()   // enables time + io drivers; net driver included but tokio/net
                    // feature gates network types — enable_all() is safe here
    .build()
    .expect("tokio runtime init")
```

`mio` is a transitive dependency of tokio's I/O driver. It is not listed directly in `Cargo.toml` and must not be. tokio manages its version. The `enable_all()` call enables the I/O and time drivers required by `tokio::fs`, `tokio::time`, and `spawn_blocking`.

### Timeout Strategy

Every tool call is wrapped at the dispatch layer:

```rust
tokio::time::timeout(tool_timeout, execute_tool(ctx, params))
    .await
    .map_err(|_| ToolError::timeout(tool_name, tool_timeout))?
```

- **Default global timeout**: 30 seconds.
- **Per-tool override**: specified in the TOML config file under `[tools.<name>]` `timeout_secs`. Loaded at startup via `figment`.
- Timeout fires a `tokio::time::error::Elapsed` which is mapped to a structured `ToolError::Timeout` returned to the MCP caller.

### CancellationToken Propagation

A `CancellationToken` is created per tool call invocation and threaded into the tool context:

```
MCP dispatch
  └─ CancellationToken::new()
       ├─ async tool fn  ←─ select! { result = work => ..., _ = token.cancelled() => Err(Cancelled) }
       ├─ spawn_blocking(|| { ... token.is_cancelled() check between chunks ... })
       └─ child process  ←─ kill_on_drop = true (see ADR-0031)
```

Async code uses `tokio::select!` to race the work future against `token.cancelled()`. Blocking closures poll `CancellationToken::is_cancelled()` between processing chunks (e.g., between each file in a directory walk, between each line batch in a grep pass). CPU-bound closures poll between computation units.

The token is cancelled explicitly by the timeout wrapper on `Elapsed`, and may also be cancelled by a future MCP cancellation notification (if the `rmcp` transport exposes one).

### Semaphore for CPU-Bound Work

Concurrent Zone C (CPU-bound) calls are gated by a `tokio::sync::Semaphore` sized to `num_cpus::get()` permits. This prevents simultaneous blake3 mmap hashes from overwhelming the system. See ADR-0017 for full Semaphore sizing rationale.

### No MutexGuard Across `.await`

All `MutexGuard` lifetimes are scoped within `{ }` blocks before any `.await` point. Clippy lint `clippy::await_holding_lock` is enabled in the workspace `[lints]` to enforce this at compile time.

### Cancellation Patterns

Correct use of `tokio::select!`, `OwnedSemaphorePermit` lifetime, `JoinSet` for internal task handles, async-Drop limitation, and Mutex type selection are codified in [ADR-0037](0037-async-cancellation-patterns.md): biased-select rule, permit lifetime rule, JoinSet rule, async-Drop limitation, Mutex policy, and `async_trait` `Send` bound.

### Consequences

#### Positive

- Every tool call has a bounded wall-clock lifetime regardless of what happens inside.
- CancellationToken provides cooperative, race-free cancellation across all three async zones.
- Timeout values are operator-configurable without recompilation.
- The multi-thread scheduler allows spawn_blocking calls to proceed in parallel with async I/O.

#### Negative

- Spawn_blocking threads do not terminate instantly on cancellation — they complete their current chunk. This means resource release after cancellation may be delayed by one chunk duration (e.g., one file's grep pass).
- `enable_all()` activates the I/O driver even when `outbound-net` feature is not enabled; this is harmless but slightly increases the driver surface.
- CancellationToken threading adds a parameter to every tool function signature; this is an API contract enforced by the `substrate-core` crate (see ADR-0028).

## Validation

- Integration test: spawn a tool that sleeps 60 s; assert that the MCP response arrives within `default_timeout + 1 s` with `ToolError::Timeout`.
- Integration test: cancel a running tool via token; assert that all spawned blocking threads terminate within 2 s of cancellation.
- `clippy::await_holding_lock` must emit zero warnings in CI.
- Load test: 10 concurrent tool calls must all complete or timeout independently without deadlock.

## Cross-References

- ADR-0003: crate stack; which crates belong in each async zone.
- ADR-0008: error type hierarchy; how `ToolError::Timeout` and `ToolError::Cancelled` are defined.
- ADR-0017: Semaphore sizing for CPU-bound concurrency limits.
- [ADR-0032](0032-signal-safety.md): signal safety; SIGTERM/SIGINT trigger root `CancellationToken` cancellation and initiate the shutdown drain.
- [ADR-0037](0037-async-cancellation-patterns.md): biased-select rule, permit lifetime rule, `JoinSet` rule, async-Drop limitation, Mutex policy, `async_trait` `Send` bound.
