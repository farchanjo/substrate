---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0005 — STDIO Transport

## Context and Problem Statement

Substrate is an MCP server consumed exclusively by LLM agent runtimes that spawn it as a subprocess and communicate over stdin/stdout (the canonical local-tool pattern). The server must never bind network sockets by default, because binding would (a) expose OS management primitives to the network, (b) require port allocation, and (c) violate the principle of least privilege for a subprocess tool.

The question: which transport mechanism does substrate support, and how is optional outbound connectivity handled?

## Decision Drivers

- Security: OS management tools (fs, proc, sys, archive) must not be reachable over the network by default.
- Simplicity: STDIO setup requires zero configuration; socket/HTTP would require TLS, authentication, and port management.
- MCP spec compliance: STDIO is the canonical local-server transport in the MCP specification.
- Auditability: stdout is the MCP wire; any accidental `println!` corrupts the framing.
- Extensibility: future tool implementations may need to call external APIs (outbound only) without opening an inbound listener.

## Considered Options

1. **STDIO only** (rmcp::transport::stdio + ServiceExt) — no network listener, outbound HTTP behind feature flag.
2. **HTTP/SSE server** — listens on a TCP port, supports remote clients.
3. **Dual mode** — STDIO by default, HTTP opt-in via CLI flag.

## Decision Outcome

Chosen option: "STDIO only", because it eliminates the network attack surface entirely, requires zero configuration from the agent runtime, and matches the MCP specification's recommended pattern for subprocess tools.

### STDIO Bootstrap Pattern

```rust
// main.rs — canonical bootstrap
#[tokio::main]
async fn main() {
    // Logs to stderr; stdout is sacred for MCP framing.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let server = SubstrateServer::new();
    let transport = rmcp::transport::stdio();
    server.serve(transport).await.expect("server error");
}
```

The `ServiceExt::serve` call wires the transport to the JSON-RPC dispatcher. No `TcpListener`, no `axum::Router`, no `tokio::net` imports anywhere in the binary.

### stdout Sanctity — Lint Rule

`println!` and `print!` macros write to stdout and corrupt MCP framing. A Clippy deny rule is enforced project-wide:

```toml
# .cargo/config.toml
[build]
rustflags = ["-D", "clippy::print_stdout"]
```

All diagnostic output uses `tracing::info!` / `tracing::warn!` / `tracing::error!`, which route to stderr via the subscriber initialized above.

### outbound-net Feature Flag

Certain tools (e.g., future `net.fetch`) need to make outbound HTTP requests. These are gated behind the `outbound-net` Cargo feature:

```toml
# Cargo.toml
[features]
default = []
outbound-net = ["reqwest"]
```

- Feature is **OFF** by default in all published builds.
- When disabled, the compiler excludes all `reqwest` and socket code via `#[cfg(feature = "outbound-net")]`.
- Enabling the feature does **not** open any inbound listener; it only allows the process to initiate TCP connections.
- Agent runtimes that require air-gapped operation MUST build with `--no-default-features` and verify the feature is absent.

### Consequences

#### Positive

- Zero network attack surface in the default build.
- No TLS, no port management, no authentication infrastructure required.
- stdout corruption is caught at compile time by the Clippy deny rule.
- outbound-net is opt-in and auditable via `cargo metadata`.

#### Negative

- Remote or networked agent runtimes cannot connect to substrate without a local subprocess wrapper.
- Multiplexing multiple agents on one substrate instance is not possible over STDIO; each agent spawns its own process.

## Validation

- `cargo clippy -- -D clippy::print_stdout` passes with zero warnings in CI.
- Integration test asserts no TCP socket is bound after server startup (checks `/proc/self/net/tcp` or `lsof` on macOS).
- `cargo build` (no features) produces a binary with no `reqwest` or `tokio::net::TcpListener` symbols (verified via `nm`).

### Transport Failure Handling

#### EPIPE on stdout write

When a write to stdout returns `io::ErrorKind::BrokenPipe`, the client has closed its end of the pipe. This is treated as an implicit cancellation of all in-flight requests:

1. Signal all active `CancellationToken`s.
2. Drain no further output.
3. Exit with code 0.

Retries on EPIPE are prohibited — the channel is gone and cannot recover.

#### stdin EOF

When stdin reaches EOF, the client process has either crashed or cleanly closed the pipe. Behavior mirrors SIGTERM handling (see [ADR-0032](0032-signal-safety.md)):

1. Broadcast cancellation to all in-flight `CancellationToken`s.
2. Wait up to `shutdown_drain_secs` (default: **5 seconds**) for in-flight tool handlers to complete.
3. Exit with code 0.

#### SIGPIPE

`SIGPIPE` is set to `SIG_IGN` during process startup. This ensures that broken-pipe conditions on stdout surface as `io::ErrorKind::BrokenPipe` errors in Rust (handled above) rather than immediately killing the process via the default OS signal disposition. Full signal-safety details are in [ADR-0032](0032-signal-safety.md).

#### Stalled client read (backpressure)

If the client is a lazy reader and the stdout write does not complete within **30 seconds**, the write is treated as EPIPE: all `CancellationToken`s are signalled and the process exits with code 0. No retries.

#### Maximum inbound message size

A single JSON-RPC message (newline-delimited) is capped at **1 MiB**. If the cap is exceeded:

1. Respond with JSON-RPC error code `-32600` ("invalid request") to the originating request ID.
2. Log a warning to stderr.
3. Close the session.

This prevents OOM conditions from malformed or adversarially large input.

#### Maximum concurrent in-flight tool calls

Default limit: **32** simultaneous in-flight tool calls. Configurable via `[protocol] max_in_flight_requests` in the substrate configuration file.

When the limit is exceeded, the server responds immediately with:

```json
{
  "code": -32000,
  "message": "server overloaded",
  "data": {
    "recovery_hint": "wait and retry"
  }
}
```

No tool handler is spawned for the rejected request.

## Cross-References

- ADR-0008: MCP Feature Usage Map — which MCP capabilities ride over this transport.
- ADR-0009: (reserved) Logging and Observability.
- ADR-0013: MCP Protocol Version Pinning — initialize handshake over this transport.
- ADR-0032: Signal Safety — SIGPIPE disposition and SIGTERM drain behavior referenced above.
