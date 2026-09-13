# mcp-os Constitution

This document establishes the foundational principles and governance model for
the `mcp-os` repository, which implements `substrate`: a Model Context
Protocol (MCP) server exposing POSIX baseutils-equivalent OS management to LLM
agents over STDIO. It guides every decision about scope, contribution, and
evolution of the system. Where this constitution and an individual ADR appear
to disagree, the ADR carries the implementation detail and this document
carries the invariant the ADR must not violate; a real conflict is a defect in
one of the two and must be resolved by amending the wrong one, not by ignoring
either.

## Principles

1. **Strategic domain-driven design.** Ten bounded contexts partition the tool
   surface by semantic family and mutation risk — filesystem-query,
   filesystem-mutation, process, system-info, text-processing, archive, job,
   subprocess, network-info, and launch — each with its own ubiquitous
   language, aggregates, and adapter crate. No aggregate root crosses a
   context boundary; the only permitted inter-context dependency at the
   domain layer is the shared kernel, `substrate-domain`. The vocabulary in
   `doc/arch/glossary.md` is authoritative: a new domain term is added there
   before it appears in code, ADRs, or Gherkin. See
   [ADR-0002](../../doc/arch/adr/0002-bounded-contexts.md).

2. **Tactical domain-driven design.** Domain concepts are modeled as
   aggregates, entities, and value objects with invariants enforced at
   construction, not validated after the fact. Value objects
   (`JailedPath`, `PageCursor`, `ProgressToken`, `AuditEvent`, and
   bounded-context-local equivalents) are immutable and are constructible
   only through a validating constructor — for example `JailedPath` is built
   exclusively by the policy crate after allowlist and symlink-escape checks
   pass, and adapters receive it by value and MUST NOT construct one
   directly. Each bounded context owns exactly one aggregate root that is the
   sole entry point for mutation within that context (`Stack` for launch,
   `SubprocessHandle` for subprocess, `JobEntry` for job, and so on).

3. **Hexagonal layering is compiler-enforced, not a convention.**
   `substrate-domain` is the innermost ring: zero infrastructure dependencies
   beyond std, serde, thiserror, async-trait, futures, uuid, tracing, and the
   narrow accepted amendments (`time`, `serde_json`) recorded in
   [ADR-0022](../../doc/arch/adr/0022-project-layout.md). Bounded-context
   adapter crates depend on `substrate-domain` (and `substrate-policy` for
   write paths) and MUST NOT depend on each other; only
   `substrate-mcp-server`, the composition root, depends on `rmcp` and wires
   adapters to the tokio runtime. The dependency rule is double-enforced: the
   compiler rejects a domain crate that imports an adapter, and
   `policies/hexagonal_layering.rego` gates CI. Any exception (the
   `substrate-subprocess` `outbound-net` health-probe feature, the
   `substrate-launch` supervise-fork re-exec) is a narrow, named,
   Rego-encoded carve-out — never a blanket relaxation.

4. **GoF patterns are named where used, not reinvented ad hoc.** Where a
   Gang-of-Four pattern is the right tool, it is used explicitly and named in
   both code and glossary. Current uses: Abstract Factory (`PortFactory<P>`
   selects the adapter tier for a port from probed `Capabilities`), Decorator
   (`InstrumentedAdapter<A>` wraps every concrete port adapter with a tracing
   span and cancellation propagation before injection into the composition
   root), Mediator (`JobRegistry` is the sole authority for `JobEntry`
   lifecycle transitions), and Null Object (`PollingWatcher` implements the
   `FsWatcher` port with the same `FsEvent` stream contract as the
   kernel-native tiers when no native watch mechanism is available). New
   adapter code reaches for a named pattern before inventing a bespoke
   abstraction.

5. **English, UUIDv7, and stable identifiers.** Every written artifact — code,
   comments, commit messages, ADRs, Gherkin, CUE, error strings, logs — is
   en-US. Every time-ordered identifier that crosses a tool-call boundary
   (`job_id`, `correlation_id`, `IdempotencyKey`, audit-event ids) is UUIDv7;
   no other UUID version and no bare sequential integer is used for such an
   identifier. Error codes are stable `SUBSTRATE_<UPPER_SNAKE>` strings that
   are never renumbered or reused once shipped.

6. **Security is defense-in-depth against a local, single-client threat
   model.** The threat model
   ([ADR-0029](../../doc/arch/adr/0029-threat-model.md)) is a local MCP
   server invoked by one trusted-transport client, not a network service: the
   primary attacker is a prompt-injected payload that causes the LLM to emit
   malicious tool arguments, and the secondary attacker is a compromised or
   malicious MCP client process, or a malicious project the agent has been
   pointed at, running on the same machine. Network-based remote attackers
   and physical-access scenarios are explicitly out of scope — substrate
   binds no listener in default builds. Every tool invocation passes, in
   order, through: (1) an allowlist, default-deny; (2) a path jail
   (`strict-path` plus `openat2`/`O_NOFOLLOW_ANY`); (3) a mandatory dry-run
   gate for mutating tools; (4) elicitation (explicit human confirmation) for
   destructive operations; and, for the subprocess and launch contexts, (5) a
   binary allowlist, an environment-variable allowlist, and a cwd jail. All
   five layers are enforced server-side before any syscall; client-side
   validation is advisory only and MUST NOT be relied upon. See
   [ADR-0004](../../doc/arch/adr/0004-security-model.md).

7. **Async work is zone-classified, never guessed.** Every tool
   implementation is one of Zone A (async-native, awaited directly on the
   tokio executor), Zone B (blocking syscalls via
   `tokio::task::spawn_blocking`), or Zone C (CPU-saturating work via
   `spawn_blocking` plus a `Semaphore` sized to `num_cpus`), per
   [ADR-0003](../../doc/arch/adr/0003-crate-stack-and-async-zones.md). Zone
   C work MUST NOT execute synchronously on the request path; it is
   dispatched through the async job control-plane and returns a `job_id`
   immediately. A `Semaphore` permit lives only in async scope and is never
   moved into a `spawn_blocking` closure, because the release profile's
   `panic = "abort"` makes unwind-based RAII unsound inside a blocking
   closure.

8. **STDIO is sacred; the process is disciplined.** substrate speaks MCP
   JSON-RPC exclusively over stdio; stdout carries only the wire protocol.
   `println!`/`print!` are forbidden in `src/`; every diagnostic goes to
   stderr via `tracing_subscriber::fmt().with_writer(std::io::stderr)`. The
   release profile builds with `panic = "abort"`; `SIGPIPE` is set to
   `SIG_IGN` at startup; `SIGTERM`/`SIGINT` trigger a bounded graceful drain
   (default 5 s). No network listener is opened by default; outbound network
   access requires the explicit, non-default `outbound-net` Cargo feature.
   See [ADR-0005](../../doc/arch/adr/0005-stdio-transport.md).

9. **Spec is the source of truth, and it is validated, not assumed.** Every
   architectural decision is recorded as a MADR 4.0 ADR under
   `doc/arch/adr/`, immutable once accepted and cross-referenced forward
   when superseded. Every bounded context has CUE schemas carrying a DDD-role
   header and Gherkin feature specs under `doc/arch/specs/features/`; the C4
   model lives in Structurizr DSL. Code and spec MUST agree; when they
   diverge in practice, the code is ground truth and a spec-correction change
   is raised alongside the code change, not deferred. Compliance is
   machine-checked — `speckit validate --json` for this constitution and
   feature lifecycle, and the `spec` CLI lanes for
   ADR/CUE/Gherkin/Structurizr/Rego — not asserted by review comment alone.

## Governance

Changes to this constitution require a recorded Architecture Decision (MADR)
under `doc/arch/adr/` with status `accepted` and at least one `deciders`
entry, mirroring the ADR process used for every other architectural decision
in this repository. Trivial corrections (typos, formatting, broken links) may
be committed directly; changes to a principle's substance — adding, removing,
or materially narrowing an article — must go through the ADR process and are
then reflected here as a dated entry under an `## Amendments` heading,
following the same append-only amendment convention this repository already
uses for its ADRs (see the `## Amendments` sections of ADR-0002, ADR-0003,
ADR-0004, ADR-0007, and ADR-0022): the prior text is retained and a new,
dated subsection records what changed and why, rather than silently rewriting
ratified language.

A pull request that touches `crates/**` or `doc/arch/**` and conflicts with
an article in this constitution is out of compliance regardless of whether it
passes `cargo test` or `speckit validate`; constitutional compliance is a
precondition for merge, not an optional lint.
