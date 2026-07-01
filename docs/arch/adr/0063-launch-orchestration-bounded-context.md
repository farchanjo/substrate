---
status: accepted
date: 2026-06-30
deciders: [com.archanjo]
consulted: []
informed: []
tags: [bounded-context, orchestration, launch, lifecycle, supervisor]
---

# ADR-0063 — Launch Orchestration Bounded Context

## Context and Problem Statement

The subprocess bounded context ([ADR-0052](0052-subprocess-execution-architecture.md))
spawns and supervises individual child processes, with per-process restart
policy, health probes, and lifecycle states added by
[ADR-0056](0056-subprocess-supervisor-semantics.md). What it does not provide is
a way to declare, version, and bring up a *set* of related processes — a project
dev stack such as an API server, a frontend watcher, and a database — with
ordering, readiness gating between members, and a single named handle the agent
can reason about across sessions.

Operators want to declare such a stack once in a project-local file and have an
MCP client (Claude Code) bring it up, watch it, and tear it down cleanly. The
critical non-functional requirement is OS hygiene: when the MCP client closes,
supervised processes must never be left running as unmanaged orphans, and a
later session must be able to discover and re-attach to anything still running.

The question this ADR answers is: what bounded context hosts declarative
multi-process orchestration, how does it compose the existing subprocess BC
rather than duplicate it, and what lifecycle contract guarantees that no process
is ever left polluting the host?

## Decision Drivers

- The subprocess BC already owns spawn, restart policy, health probes, cascade
  kill, stream multiplex, and the job/Tasks control-plane. Orchestration MUST
  compose those primitives, not re-implement them
  ([ADR-0052](0052-subprocess-execution-architecture.md),
  [ADR-0053](0053-process-lifecycle-cascade-contract.md),
  [ADR-0054](0054-subprocess-stream-multiplex.md),
  [ADR-0056](0056-subprocess-supervisor-semantics.md)).
- OS hygiene is non-negotiable: the default behaviour on client disconnect MUST
  leave zero surviving processes. Survival beyond the session MUST be an explicit
  per-stack opt-in, and even then bounded by a time-to-live.
- The async-zone classification ([ADR-0003](0003-crate-stack-and-async-zones.md)),
  cancellation patterns ([ADR-0037](0037-async-cancellation-patterns.md)), and
  signal safety ([ADR-0032](0032-signal-safety.md)) apply unchanged.
- Hexagonal layering ([ADR-0022](0022-project-layout.md)): the new adapter crate
  depends on `substrate-domain` (whose `SubprocessPort` trait it consumes) and
  `substrate-policy`, never another adapter; the concrete `substrate-subprocess`
  adapter is injected by the `substrate-mcp-server` composition root.
- Tool naming ([ADR-0062](0062-tool-naming-convention.md)): tools are namespaced
  `launch.<verb>`.

## Considered Options

- Option A: Add multi-process orchestration directly to the subprocess BC as
  more optional fields on `SubprocessRequest`.
- Option B: Introduce a dedicated `launch` bounded context that orchestrates the
  subprocess BC through its existing port (selected).
- Option C: Keep orchestration entirely client-side; the agent issues N
  `subprocess.spawn` calls and tracks dependencies itself.

## Decision Outcome

Chosen option: **Option B — a dedicated `launch` bounded context that orchestrates
the subprocess BC**, because multi-process composition (a declarative catalog,
a dependency graph, readiness gating between members, file-level trust, and stack
lifecycle) is a distinct semantic family from single-process supervision, and
folding it into `SubprocessRequest` would overload that aggregate with concerns
it does not own.

Option A is rejected: it conflates "supervise one process" with "orchestrate a
graph of processes"; the `SubprocessRequest` aggregate would acquire dependency,
catalog, and trust concerns unrelated to a single spawn.

Option C is rejected: it pushes ordering, readiness, reconciliation, and orphan
governance into every client, which cannot guarantee OS hygiene (a crashed
client leaves processes running with no authority to reap them).

### The launch bounded context

A new bounded context `launch` is introduced with this ubiquitous language:

- **Profile** — the value object parsed from `.substrate.toml`: a catalog of
  named service definitions plus stack-level defaults. Immutable once loaded.
- **Service** — one entry in a Profile: command, args, env, cwd, `depends_on`,
  readiness probe, restart policy. Each Service materialises to exactly one
  `subprocess.spawn` (with `name`, `restart_policy`, `health_probe` from
  [ADR-0056](0056-subprocess-supervisor-semantics.md)).
- **Stack** — the aggregate root: a running instance of a Profile, owning the
  dependency DAG, the per-service `subprocess` handles, and the lifecycle state.
- **Supervisor** — the long-lived component that owns the children of a Stack
  and applies the lifecycle contract below.

Tools exposed, all namespaced `launch.*`:

- `launch.init` — scaffold a `.substrate.toml` with project-type auto-detection.
- `launch.list` — enumerate Services in the Profile (read-only; no trust needed).
- `launch.up` — bring a Stack up (rides the MCP Tasks primitive,
  [ADR-0049](0049-mcp-tasks-primitive-adoption.md)).
- `launch.down` — stop a Stack (reverse-topological cascade kill).
- `launch.status` — structured status of running Stacks and per-Service health.
- `launch.logs` — multiplexed or per-Service output (reuses ADR-0054 streams).
- `launch.restart` — restart one Service.
- `launch.reload` — apply an edited Profile to a running Stack (reconciler).
- `launch.trust` — bless a Profile (see the trust ADR, forthcoming ADR-0064).

`launch` is realised by a new crate `substrate-launch`
([ADR-0022](0022-project-layout.md)) that depends on `substrate-domain` (whose
`SubprocessPort` trait it consumes) and `substrate-policy`; the concrete
`substrate-subprocess` adapter is injected by the `substrate-mcp-server`
composition root, never depended on directly. No adapter depends on
`substrate-launch`.

### Composition over the subprocess BC

Each Service is one supervised subprocess. The launch BC owns only what is
genuinely above a single process: the catalog, the `depends_on` DAG with
topological ordering and cycle detection, readiness gating *between* Services,
reconciler-style reload, file-level trust, and stack lifecycle. Per-process
restart policy, health probes, cascade kill, and stream multiplex are delegated
unchanged to the subprocess BC.

### The Supervisor and OS-hygiene lifecycle

A Stack's children must outlive the request that started it (the control-plane
returns a Task handle immediately) but MUST NOT outlive their governance. The
Supervisor is the owner of the children. Two execution modes:

- **In-session** (`on_client_disconnect = "shutdown"`, default): the Supervisor
  runs as a task inside the MCP server. When the MCP client (Claude Code)
  disconnects, the Stack is drained and killed. Nothing survives the session.
- **Detached** (`on_client_disconnect = "detach"`, opt-in): the Supervisor runs
  as the same binary in `--supervise <stack>` mode, detached via `setsid`, and
  remains the parent of the children after the MCP server exits. A later MCP
  server re-attaches to it.

This is a scoped exception to the anti-sidecar posture of
[ADR-0052](0052-subprocess-execution-architecture.md) (Option C) and
[ADR-0056](0056-subprocess-supervisor-semantics.md) (Option C), justified narrowly:
the detached supervisor is the *same* distributed binary in a documented mode (no
second artifact, so the single-binary invariant of
[ADR-0015](0015-distribution.md) is preserved, not excepted), communicates only
over the filesystem and FIFOs (no network or Unix socket, preserving
[ADR-0005](0005-stdio-transport.md)), and exists solely to make detached survival
governable. Dated forward-link amendments are added to the four affected ADRs —
[ADR-0015](0015-distribution.md) (invariant held),
[ADR-0052](0052-subprocess-execution-architecture.md) and
[ADR-0056](0056-subprocess-supervisor-semantics.md) (Option C scoped exception),
and [ADR-0055](0055-orphan-reaper-on-startup.md) (file reaper extended to process
adopt-or-reap). Detached mode is opt-in; the default ships the in-session model
those ADRs already sanction.

### Zero-orphan guarantee (defence in depth)

No supervised process is ever left running unmanaged. Five independent layers:

1. **Disconnect policy** — default `shutdown` drains and kills on client
   disconnect; nothing survives unless `detach` is explicitly set.
2. **Parent-death binding** — every child is bound to its Supervisor so that if
   the Supervisor dies, the kernel kills the children: `PR_SET_PDEATHSIG` on
   Linux and the `WatchdogPipe` on macOS, both already specified by
   [ADR-0053](0053-process-lifecycle-cascade-contract.md); Windows uses a Job
   Object with kill-on-close.
3. **Orphan TTL** — a detached Stack with no client attached for
   `launch.orphan_ttl_secs` (default 3600) is automatically brought down. A
   forgotten Stack cannot run indefinitely.
4. **Reaper on boot** — on every Supervisor and MCP-server start, a reconcile
   pass reads the durable Stack registry and, for each recorded child: if alive
   and parented to a live Supervisor, re-attach; if orphaned (reparented to init
   or launchd) and the policy is `detach`, adopt it; otherwise reap it and clear
   the registry entry. This extends the file-only `OrphanReaper` of
   [ADR-0055](0055-orphan-reaper-on-startup.md) to processes, with an
   adopt-or-reap decision.
5. **Process-group reap** — every child is a process-group leader (`setsid`,
   ADR-0053); a reap sends `killpg` so the entire descendant subtree dies, not
   just the leader.

### The monitor (reconcile loop)

A janitor task — a timer source on the Supervisor's event loop plus a boot-time
pass — periodically reconciles desired state (the durable registry) against
actual state (the OS process table observed through the `process` BC and pollable
exit sources). It detects unexpected exits (delegated to the ADR-0056 restart
policy), orphans (adopt or reap), zombies (reaped via `waitpid`), and drift
(corrected, with a hygiene event emitted). This loop is what keeps the host clean
and the registry honest.

### Lock-free control plane

The detached Supervisor is a single event-loop reactor over `mio`
(`epoll`/`kqueue`/`IOCP`) that multiplexes every source — the shared command
FIFO, child-exit sources (`pidfd` on Linux, `kqueue NOTE_EXIT` on macOS, Job
Object on Windows), timers, and the event-log writability — and drains them into
a single `mpsc` mailbox consumed by the Supervisor actor. Commands from multiple
concurrent sessions are serialised by the single consumer, not by a lock: there
is no controller election and no advisory file lock. Multi-writer command frames
are bounded to `PIPE_BUF` so the kernel guarantees atomic interleave-free writes.
Restart counters live solely inside the actor; no shared mutable state, no mutex.

### Lifecycle diagram

The sequence below shows the default `shutdown` policy keeping the host clean
when the client disconnects, and the `detach` policy surviving with re-attach.

```mermaid
sequenceDiagram
    participant Client as MCP Client (Claude Code)
    participant Server as MCP Server
    participant Sup as Supervisor
    participant Child as Stack children

    Client->>Server: launch.up (Task)
    Server->>Sup: start Stack (mode per on_client_disconnect)
    Sup->>Child: spawn (setsid, PDEATHSIG/WatchdogPipe)
    Sup-->>Server: Stack Ready
    Server-->>Client: Task handle + status

    Note over Client,Server: client disconnects (Claude Code closes)

    alt on_client_disconnect = shutdown (default)
        Server->>Sup: drain + cascade kill
        Sup->>Child: killpg SIGTERM then SIGKILL
        Note over Child: nothing survives — host clean
    else on_client_disconnect = detach
        Note over Sup,Child: Supervisor survives, owns children; orphan_ttl armed
        Client->>Server: new session: launch.status
        Server->>Sup: re-attach (registry lookup)
        Sup-->>Server: running Stack + replay
        Server-->>Client: restored
    end
```

## Consequences

### Positive

- Multi-process dev stacks are declared once and managed through a single named
  handle, composing the tested subprocess supervisor rather than duplicating it.
- OS hygiene is guaranteed by construction: the default disconnect policy leaves
  nothing running, and survival is an explicit, time-bounded opt-in.
- Orphan governance is defence-in-depth; no single failure leaves an unmanaged
  process, and the reaper-on-boot is the backstop for the rare double-failure.
- The lock-free reactor removes controller-election complexity and the only
  mutex the earlier design introduced.

### Negative

- The detached mode is a scoped exception to three accepted ADRs and requires
  amendment notes plus a dedicated decision record for the detached supervisor
  (forthcoming ADR-0068).
- The durable Stack registry is new persistent state under the user state
  directory that must be reconciled on every boot.
- Composition across BC boundaries (launch → subprocess) adds an integration
  surface that must be covered by Gherkin features.

### Risks

- A misconfigured `detach` policy with a long `orphan_ttl_secs` could keep a
  Stack alive longer than intended. Mitigation: conservative default (1 hour)
  and explicit surfacing of detached Stacks in `launch.status`.
- Cross-platform parent-death binding has three distinct implementations
  (PDEATHSIG, WatchdogPipe, Job Object); each requires its own validation.

## Validation

- Integration test: `launch.up` a two-Service Stack with default policy;
  disconnect the client; assert both children are killed and the registry entry
  is cleared (zero surviving processes).
- Integration test: `launch.up` with `on_client_disconnect = detach`; disconnect;
  assert children survive; reconnect; assert `launch.status` re-attaches and
  reports the running Stack.
- Integration test: kill the detached Supervisor with `SIGKILL`; assert children
  die via PDEATHSIG (Linux) / WatchdogPipe (macOS) and the next boot's reaper
  finds no survivors.
- Integration test: detached Stack with `orphan_ttl_secs = 2` and no client;
  assert the Stack is auto-brought-down after the TTL.
- Integration test: leave an orphaned child in the registry (simulated crash);
  assert reaper-on-boot reaps it under a `shutdown` policy and adopts it under a
  `detach` policy.
- Unit test: concurrent `launch.down` and `launch.reload` frames from two
  sessions; assert serialised application with no interleaving and consistent
  final state.

## Links

- [ADR-0003](0003-crate-stack-and-async-zones.md) — async zones; the Supervisor
  reactor is async-native (Zone A)
- [ADR-0005](0005-stdio-transport.md) — STDIO transport; the detached supervisor
  uses filesystem and FIFO IPC only, no socket
- [ADR-0015](0015-distribution.md) — single-binary distribution; detached mode is
  the same binary, not a second artifact (scoped exception)
- [ADR-0022](0022-project-layout.md) — workspace layout; new crate
  `substrate-launch`
- [ADR-0049](0049-mcp-tasks-primitive-adoption.md) — MCP Tasks; `launch.up` rides
  the Tasks control-plane
- [ADR-0052](0052-subprocess-execution-architecture.md) — subprocess BC composed
  by launch; Option C (sidecar) scoped exception
- [ADR-0053](0053-process-lifecycle-cascade-contract.md) — cascade kill,
  PDEATHSIG, WatchdogPipe reused as orphan-prevention layers
- [ADR-0054](0054-subprocess-stream-multiplex.md) — stream multiplex reused by
  `launch.logs`
- [ADR-0055](0055-orphan-reaper-on-startup.md) — file-only reaper extended to
  processes with adopt-or-reap
- [ADR-0056](0056-subprocess-supervisor-semantics.md) — per-process supervisor
  semantics composed by each Service; Option C scoped exception
- [ADR-0062](0062-tool-naming-convention.md) — `launch.*` tool namespace

## Amendments

### 2026-06-30 — Accepted; MVP landed as the substrate-launch crate

The launch BC is implemented as the `substrate-launch` workspace crate (gated
behind the default-off Cargo feature `launch`) and registered in
`substrate-mcp-server` as nine `launch.*` tools. Status moves from `proposed` to
`accepted`. The MVP composes every Service through the injected `SubprocessPort`
(no `tokio::process::Command` in `substrate-launch`, so no `no_subprocess.rego`
exception is required): `launch.init` / `list` / `trust` / `up` (in-session,
readiness-gated topological start) / `status` / `logs` / `restart` / `reload`
(diff: added/removed/edge-only) / `down` (reverse-topological).

The **detached supervisor** ([ADR-0068](0068-launch-detached-supervisor-and-orphan-governance.md):
`--supervise` self-fork, control FIFO, `mio` reactor, reaper-on-boot, orphan
adopt/reap, TTL, PID-recycle, PIPE_BUF framing) is deferred to **Milestone 2**.
Until then an `on_client_disconnect = detach` request returns
`SUBSTRATE_LAUNCH_SUPERVISOR_UNREACHABLE` before any spawn, and the MVP enforces
`shutdown` semantics. Reload subgraph-degrade and event-replay summary are
likewise deferred (tail-only logs in the MVP).

### 2026-07-01 — `launch.forget` added (tenth tool)

`LaunchRegistry.stacks` (a `DashMap<StackId, StackEntry>`) is process-lifetime
scoped for `on_client_disconnect = shutdown` Stacks — only `detach` Stacks get a
durable `supervisor.json` (ADR-0068). `launch.down` transitions a Stack to
`Down` in place but never evicts its entry, so `launch.status`/`launch.logs`
kept listing every Stack an operator had ever brought up for as long as the MCP
server process lived; the only way to clear the listing was to reconnect the
client (restarting the server process, and its in-memory registry with it).

Found via a live end-to-end exercise of all nine MVP tools through a real MCP
connection (not the cucumber harness): bringing up and tearing down two
short-lived smoke-test Stacks left both permanently visible in `launch.status`
for the remainder of the session.

Added `launch.forget(stack_id)`: removes a Stack's entry from the registry,
rejecting with the new `SUBSTRATE_LAUNCH_STACK_NOT_TERMINAL` (-32058) error
(ADR-0010 amendment) when the Stack's state is not `Down`. No process is
signalled — the Stack is already fully torn down by `launch.down` before
`launch.forget` is ever meaningful to call. The launch BC now registers ten
`launch.*` tools; ADR-0069's tool-card budget and ToolSearch-discoverability
checks (`launch-tool-descriptions-toolsearch-discoverable.feature`) cover the
new card identically to the original nine.
