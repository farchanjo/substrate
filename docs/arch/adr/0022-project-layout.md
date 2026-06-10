---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0022 — Project Layout (Cargo Workspace)

## Context and Problem Statement

Substrate is implemented as a Rust workspace. Without a canonical workspace
layout, contributors make inconsistent decisions about where to place domain
types, policy enforcement, configuration parsing, and MCP adapter wiring.
The layout must encode hexagonal architecture constraints so that the compiler
enforces the dependency rule rather than relying on convention alone.

## Decision Drivers

- Hexagonal architecture: domain crates must not import infrastructure crates.
- Six bounded contexts (ADR-0002) each need an independent crate with its own
  port and adapter.
- A single composition root assembles all adapters and starts the MCP server.
- Tests must be co-located with the crate they exercise.
- The layout must be legible to a new contributor without additional tooling.

## Considered Options

- Option A: Single crate with modules for each bounded context.
- Option B: Cargo workspace with one crate per bounded context plus shared
  support crates.
- Option C: Separate Git repositories per bounded context.

## Decision Outcome

Chosen option: "Option B — Cargo workspace with one crate per bounded context
plus shared support crates", because it gives the compiler visibility into
inter-crate dependency rules, enables independent `cargo check` and `cargo test`
per context, and keeps all code in one repository for atomic commits.

### Workspace Layout

```
mcp-os/
  Cargo.toml                  # workspace manifest
  crates/
    substrate-domain/         # shared kernel: JailedPath, ToolResult,
                              #   PageCursor, ProgressToken, AuditEvent
                              # ZERO infrastructure dependencies
    substrate-policy/         # allowlist evaluation, dry-run engine,
                              #   elicitation decision logic
    substrate-config/         # TOML config parsing and validation
    substrate-fs-query/       # filesystem-query bounded context
    substrate-fs-mutation/    # filesystem-mutation bounded context
    substrate-fs-index/       # optional filesystem index adapter (opt-in feature)
    substrate-fs-index-macos-sys/ # macOS platform shim for FSEvents / kqueue index backend
    substrate-process/        # process bounded context
    substrate-system-info/    # system-info bounded context
    substrate-text/           # text-processing bounded context
    substrate-archive/        # archive bounded context
    substrate-jobs/           # async job control-plane adapter (ADR-0040)
    substrate-signal-sys/     # platform shim: SIGPIPE disposition + signal safety (ADR-0032)
    substrate-mcp-server/     # binary: composition root, MCP transport
  docs/
    arch/                     # spec root (ADRs, schemas, decisions)
```

```mermaid
classDiagram
    direction TB

    class substrate_domain {
        <<innermost ring>>
        JailedPath
        ToolResult
        PageCursor
        ProgressToken
        AuditEvent
        NO infra deps
    }

    class substrate_policy {
        allowlist evaluation
        dry-run engine
        elicitation decision
    }

    class substrate_config {
        figment TOML loader
        allowlist canonicalization
    }

    class substrate_fs_query {
        filesystem-query BC
        ls, find, stat, du, file
    }

    class substrate_fs_mutation {
        filesystem-mutation BC
        mkdir, cp, mv, rm, chmod
    }

    class substrate_process {
        process BC
        ps, kill, pgrep
    }

    class substrate_system_info {
        system-info BC
        uname, df, uptime
    }

    class substrate_text {
        text-processing BC
        grep, sed, wc
    }

    class substrate_archive {
        archive BC
        tar, gzip, zip
    }

    class substrate_mcp_server {
        <<outermost ring>>
        composition root
        rmcp wiring
        tokio runtime
        signal handlers
    }

    substrate_domain <|-- substrate_policy
    substrate_domain <|-- substrate_fs_query
    substrate_domain <|-- substrate_fs_mutation
    substrate_domain <|-- substrate_process
    substrate_domain <|-- substrate_system_info
    substrate_domain <|-- substrate_text
    substrate_domain <|-- substrate_archive
    substrate_policy <|-- substrate_fs_mutation
    substrate_policy <|-- substrate_process
    substrate_policy <|-- substrate_archive
    substrate_config <|-- substrate_mcp_server
    substrate_fs_query <|-- substrate_mcp_server
    substrate_fs_mutation <|-- substrate_mcp_server
    substrate_process <|-- substrate_mcp_server
    substrate_system_info <|-- substrate_mcp_server
    substrate_text <|-- substrate_mcp_server
    substrate_archive <|-- substrate_mcp_server
```

### Hexagonal Layering Rule

`substrate-domain` is the innermost ring. It declares ports as Rust traits and
value objects. It has zero `[dependencies]` entries that reference any other
crate in this workspace or any crate that performs I/O, system calls, or
serialization beyond `serde` derive macros.

Each bounded context crate (`substrate-fs-query`, etc.) depends on
`substrate-domain` and `substrate-policy`. It implements the domain ports as
adapters using the `nix` crate or Rust standard library. It does not depend on
`substrate-mcp-server` or on other bounded context crates.

`substrate-mcp-server` is the outermost ring. It depends on all bounded context
crates and on `rmcp`. It is the only crate that wires adapters to ports and
starts the tokio runtime.

The rule is enforced by Cargo: because `substrate-domain` does not list any
bounded context crate as a dependency, a domain crate that accidentally imports
an adapter will fail to compile.

### Test Layout

Unit tests: `src/` inline `#[cfg(test)]` modules in each crate.
Integration tests: `crates/<crate>/tests/` directory, one file per scenario.
End-to-end tests: `crates/substrate-mcp-server/tests/` exercising the full
MCP JSON-RPC surface against a spawned server process.

### Consequences

#### Positive

- Compiler enforces the dependency rule with zero additional tooling.
- Each context can be checked, tested, and published independently.
- Composition root is the single place where all wiring decisions are visible.

#### Negative

- More `Cargo.toml` files to maintain.
- Cross-crate refactors touch multiple manifests.

## Validation

- `cargo check --workspace` must pass with zero warnings on the clean tree.
- `cargo deny check` must confirm `substrate-domain` has no workspace-internal
  dependencies beyond itself.
- CI must run `cargo test -p substrate-domain` in isolation to verify the zero
  infra rule holds.

## Amendment — 2026-05-24 — New crate substrate-subprocess (ADR-0052)

[ADR-0052](0052-subprocess-execution-architecture.md) introduces `crates/substrate-subprocess/` as the adapter for the subprocess bounded context.

**Layering.** `substrate-subprocess` depends on `substrate-domain` (port traits, `SubprocessPort` definition), `substrate-policy` (binary allowlist and env allowlist enforcement at Layer 5 of ADR-0004), and `substrate-jobs` (JobRegistry integration for Bucket E dispatch per ADR-0040). It does not depend on any other bounded context adapter crate. It is the only crate in the workspace permitted to import `tokio::process::Command`.

**Cargo feature gating.** The crate is a workspace member and is compiled as part of the workspace build. However, `substrate-mcp-server` declares it as an optional dependency:

```toml
# crates/substrate-mcp-server/Cargo.toml
[dependencies]
substrate-subprocess = { path = "../substrate-subprocess", optional = true }

[features]
subprocess = ["substrate-subprocess"]
# default does not include subprocess
default = []
```

When the `subprocess` feature is not enabled, the subprocess BC tools are absent from the MCP tool list and no subprocess execution paths are compiled into the server binary.

**Hexagonal layering rule.** `substrate-subprocess` is the ONLY crate in the workspace permitted to import `tokio::process::Command` (and by extension `tokio::process::Child`, `tokio::process::Stdio`). All other crates remain subject to the prohibition from [ADR-0044](0044-no-subprocess-policy.md). This restriction is enforced by updating the `no_subprocess.rego` Rego policy to include a whitelist entry for the path `crates/substrate-subprocess/`; the policy continues to deny `Command` usage in all other crate paths.

Cross-reference: [ADR-0052](0052-subprocess-execution-architecture.md).

## Amendment — 2026-05-22 — Additional domain dependencies (accepted)

In practice the domain crate carries two additional dependencies beyond the
original allow-list:

- `time` (0.3) — value-object timestamp arithmetic for `IdempotencyKey` TTL
  bounds and audit-event `seq` ordering. `time` is a pure-data crate (no I/O)
  and does not pull in any runtime; consistent with hexagonal layering.
- `serde_json` — error-envelope serialization for the `recovery_hint` JSON
  path expressions and `structuredContent` value-object marshalling.
  `serde_json` is used only inside value-object impls, never inside port
  traits.

Both are accepted on hexagonal grounds (no I/O, no runtime, no syscalls) and
the Rego policy `hexagonal_layering.rego` is updated alongside this amendment.

## Amendment — 2026-06-10 — Layering corrections (network-info, subprocess net, fs-index test-only)

Three layering clarifications are recorded here and reflected in
`policies/hexagonal_layering.rego`:

- **`substrate-network-info` classified as an adapter.** The network-info
  bounded-context crate was previously unclassified by the Rego taxonomy. It is
  an adapter on the same footing as the other bounded-context crates: it may
  depend on `substrate-domain` (+ `substrate-policy`) and MUST NOT depend on any
  other adapter crate. It is now a member of `_adapter_crates`.
- **`substrate-subprocess` tokio net is a documented Rule-5 exception
  ([ADR-0056](0056-subprocess-supervisor-semantics.md)).** Rule 5 reserves the
  tokio `net` feature for `substrate-mcp-server`. `substrate-subprocess` is
  granted a single, narrow exception: it MAY activate tokio `net`, but ONLY
  through its optional `outbound-net` Cargo feature, used for active TCP
  connect health-probes. The exception is encoded narrowly in the policy — it
  fires solely for `substrate-subprocess` and only for a dependency token
  carrying the explicit `outbound-net` feature marker. A bare `tokio/net` on
  `substrate-subprocess` (net without the `outbound-net` gate), and tokio `net`
  on every other non-server adapter, remain denied.
- **`substrate-fs-mutation` depends on the domain `FsIndexPort` only;
  `substrate-fs-index` is test-only.** Production code in `substrate-fs-mutation`
  touches the index exclusively through the domain port
  `Arc<dyn FsIndexPort>` (re-exported from `substrate-domain`). The `fs-index`
  Cargo feature is now a pure marker feature (no `dep:` activation): it gates the
  optional `index` field on `FsMutationDeps` and the `write_through` call sites,
  all of which are domain-port-only. The concrete `substrate-fs-index` adapter
  has been moved to `[dev-dependencies]`, used solely by `#[cfg(test)]` setups to
  build a real index. This removes the adapter→adapter dependency that Rule 3
  prohibits while preserving the production write-through behavior.

## Links

- Related: [ADR-0002](0002-bounded-contexts.md)
- Related: [ADR-0028](0028-platform-feature-gates.md)
- Related: [ADR-0040](0040-async-job-control-plane.md) — introduced `substrate-jobs`
- Related: [ADR-0041](0041-filesystem-index-native-tiers.md) — introduced `substrate-fs-index`
- Related: [ADR-0032](0032-signal-safety.md) — introduced `substrate-signal-sys`
- Related: [ADR-0056](0056-subprocess-supervisor-semantics.md) — `substrate-subprocess` `outbound-net` Rule-5 exception
