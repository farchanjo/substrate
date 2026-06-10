# substrate

Model Context Protocol (MCP) server in Rust 1.95 exposing POSIX baseutils-equivalent
OS management to LLM agents over STDIO. Nine bounded contexts: filesystem-query,
filesystem-mutation, process, system-info, text-processing, archive, job,
subprocess, network-info.

## Quick Start

### Prerequisites

- Rust 1.95+ (`rustup install 1.95 && rustup default 1.95`).
- macOS 11+ or Linux kernel 5.6+ for tier-1 PathJail (otherwise userspace tier
  with WARN).
- `mise` (optional, manages Rust + cargo tools automatically).

### Build

```bash
mise trust && mise install   # if mise is available
cargo build --workspace --release
```

The release binary lands at `target/release/substrate-mcp-server`.

### Run

Create `substrate.toml` at the project root or `~/.config/substrate/config.toml`:

```toml
[policy]
roots = ["/path/to/sandbox"]

[logging]
level = "info"
target = "stderr"

[security]
refuse_degraded_jail = true

[timeouts]
global_default_seconds = 30
shutdown_drain_secs = 5
```

Run the server (it speaks JSON-RPC 2.0 over STDIO):

```bash
./target/release/substrate-mcp-server
```

### Example interaction

```bash
printf '%s\n%s\n' \
  '{"jsonrpc":"2.0","method":"initialize","id":1,"params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0.0.1"}}}' \
  '{"jsonrpc":"2.0","method":"tools/call","id":2,"params":{"name":"sys_hostname","arguments":{}}}' \
| ./target/release/substrate-mcp-server
```

## System Overview

The following flowchart shows the principal actors and their relationships at the system boundary.

```mermaid
flowchart LR
    OP[Operator / Human] -->|elicitation forms| HOST[MCP Host\ne.g. Claude Desktop]
    AGENT[LLM Agent] -->|MCP JSON-RPC| HOST
    HOST -->|STDIO stdin/stdout| SRV[substrate\nMCP server]
    SRV -->|syscalls| FS[Local Filesystem]
    SRV -->|procfs / sysctl| OS[OS / Kernel]
```

The following sequence diagram shows the smoke-test interaction from connection through a tool call.

Note: tool names use `_` as separator (e.g. `sys_hostname`), not `.`.

```mermaid
sequenceDiagram
    participant C as MCP Client
    participant S as substrate server
    C->>S: initialize (protocolVersion, clientInfo)
    S-->>C: InitializeResult (capabilities, serverInfo)
    C->>S: tools/list
    S-->>C: ListToolsResult (tool cards array)
    C->>S: tools/call (sys_hostname, args={})
    S-->>C: CallToolResult (content text + structuredContent)
```

## Architecture

This repository uses spec-as-source-of-truth. All architectural decisions live
under `docs/arch/` as MADR 4.0 ADRs, CUE schemas, Gherkin features, Rego policies,
Structurizr DSL, OpenSLO definitions, AsyncAPI spec, and a TLA+ formal model.

Read in order:

1. [Architecture Overview](docs/arch/README.md) — entry point for the spec
2. [Glossary](docs/arch/glossary.md) — ubiquitous-language vocabulary
3. [ADR-0002](docs/arch/adr/0002-bounded-contexts.md) — strategic DDD and the nine bounded contexts (filesystem-query, filesystem-mutation, process, system-info, text-processing, archive, job, subprocess, network-info)
4. [ADR-0040](docs/arch/adr/0040-async-job-control-plane.md) — async job control-plane (Push/Pull dual channel)

## Validation

```bash
spec validate --lane full      # 13 spec validators
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo nextest run --locked --workspace --no-fail-fast
```

## TLA+ formal verification

A `JobRegistry.tla` model lives under `docs/arch/`. To enable the `run_tlc`
validator:

```bash
# Download tla2tools.jar once (see tools/README.md)
curl -L -o tools/tla2tools.jar \
  https://github.com/tlaplus/tlaplus/releases/latest/download/tla2tools.jar

# Then run the spec full lane -- TLC will be invoked automatically
spec validate --lane full
```

The `TLA2TOOLS_JAR` environment variable is auto-set by `mise` when
`tools/tla2tools.jar` is present. See `tools/README.md` for details.

## License

Dual-licensed under MIT OR Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE`.
