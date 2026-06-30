---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0008 — MCP Features Map

## Context and Problem Statement

The MCP specification (2025-06-18 and 2025-11-25) provides a rich set of optional features: tool annotations, outputSchema, pagination, progress notifications, cancellation, elicitation, and resource URIs. Substrate must declare which features it uses, per namespace, so that capability negotiation is deterministic and implementors know what to implement and test.

The question: which MCP optional features does substrate use, for which tool namespaces, and what are the concrete defaults and rules for each?

## Decision Drivers

- Determinism: agents must be able to predict feature availability from the initialize response.
- Safety: destructive operations (fs.remove, proc.signal, archive writes) require elicitation guards.
- Performance: progress notifications are necessary for long-running archive and find operations.
- Compatibility: features must degrade gracefully when the client lacks capability.
- Auditability: a single document maps all feature usage; no scattered tribal knowledge.

## Considered Options

1. **Full feature matrix** — enumerate every feature per namespace in this ADR, with defaults and rules.
2. **Feature-by-feature ADRs** — one ADR per MCP feature.
3. **Inline code comments only** — no architectural documentation.

## Decision Outcome

Chosen option: "Full feature matrix", because the features are interdependent (e.g., elicitation depends on annotations, outputSchema depends on pagination shape) and a single map is the least ambiguous reference.

### Tool Annotations Matrix

| Namespace | readOnlyHint | destructiveHint | idempotentHint | openWorldHint |
|-----------|:------------:|:---------------:|:--------------:|:-------------:|
| fs.find | true | false | true | false |
| fs.read | true | false | true | false |
| fs.read_dir | true | false | true | false |
| fs.stat | true | false | true | false |
| fs.write | false | false | false | false |
| fs.mkdir | false | false | true | false |
| fs.remove | false | **true** | false | false |
| fs.rename | false | false | false | false |
| fs.copy | false | false | false | false |
| fs.set_permissions | false | false | false | false |
| proc.list | true | false | true | true |
| proc.signal | false | **true** | false | false |
| proc.tree | true | false | true | true |
| sys.info | true | false | true | false |
| sys.uptime | true | false | true | false |
| sys.df | true | false | true | false |
| sys.uname | true | false | true | false |
| sys.hostname | true | false | true | false |
| sys.load_average | true | false | true | true |
| text.search | true | false | true | false |
| text.count_lines | true | false | true | false |
| text.head | true | false | true | false |
| text.tail | true | false | true | false |
| archive.tar.create | false | false | false | false |
| archive.tar.extract | false | false | false | false |
| archive.zip.create | false | false | false | false |
| archive.zip.extract | false | false | false | false |
| archive.gzip.compress | false | false | false | false |
| archive.gzip.decompress | false | false | false | false |
| archive.hash | true | false | true | false |
| job.status | true | false | true | false |
| job.result | true | false | true | false |
| job.list | true | false | true | false |
| job.cancel | false | false | true | false |
| net.tcp_list | true | false | true | true |
| net.udp_list | true | false | true | true |
| net.tcp_stats | true | false | true | true |
| net.connection_count | true | false | true | true |
| subprocess.spawn | false | **true** | false | true |
| subprocess.signal | false | **true** | false | false |
| subprocess.list | true | false | true | true |
| subprocess.result | true | false | true | false |
| subprocess.search | true | false | true | false |
| subprocess.cancel | false | false | true | false |
| launch.init | false | false | false | false |
| launch.list | true | false | true | false |
| launch.trust | false | false | false | false |
| launch.up | false | **true** | false | true |
| launch.status | true | false | true | true |
| launch.logs | true | false | true | true |
| launch.restart | false | **true** | false | true |
| launch.reload | false | false | false | true |
| launch.down | false | **true** | false | true |

The `job.*`, `net.*`, and `subprocess.*` rows above were added by the
2026-06-10 amendment to cover the job, network-info, and subprocess bounded
contexts introduced by [ADR-0040](0040-async-job-control-plane.md),
[ADR-0058](0058-network-socket-introspection.md), and
[ADR-0052](0052-subprocess-execution-architecture.md). `subprocess.spawn` and
`subprocess.signal` carry `destructiveHint: true` and `openWorldHint: true`
(spawn executes arbitrary allowlisted binaries) in parity with `proc.signal`;
`net.*` introspection rows mirror `proc.list` (volatile live state, hence
`openWorldHint: true`).

The `launch.*` rows were added by the 2026-06-30 amendment for the launch BC
([ADR-0063](0063-launch-orchestration-bounded-context.md) /
[ADR-0069](0069-launch-tool-cards-toolsearch-and-guidance.md)). Process-touching
launch tools (`up` / `down` / `restart` / `reload` / `status` / `logs`) carry
`openWorldHint: true` in parity with `subprocess.spawn` and the `net.*` /
`proc.list` live-state rows; `init` / `list` / `trust` stay closed-world (they
touch only the local profile and trust store). The coarser `tool_annotations.rego`
4-profile matrix records the closed-world default for all of them, exactly as it
does for `subprocess.*` / `net.*`.

### outputSchema via schemars

Every tool that returns a structured result derives `JsonSchema` via `schemars`:

```rust
#[derive(Serialize, JsonSchema)]
pub struct FindResult {
    pub matches: Vec<FileMeta>,
    pub next_cursor: Option<String>,
}
```

The derived schema is registered in the tool's `output_schema` field during server initialization. Clients that support `outputSchema` (2025-06-18+) use the schema for result validation. Clients without support receive the result as untyped JSON.

### Pagination Defaults and Cursor Encoding

- Default page size: **50** items.
- Maximum page size: **500** items (enforced server-side; requests exceeding this are clamped).
- Cursor encoding: opaque base64url-encoded JSON containing `{offset: u64, seed: u64}`. The seed is a per-request random value that invalidates stale cursors after a directory modification.
- Applicable tools: `fs.find`, `fs.read_dir`, `proc.list`, `proc.tree`.
- Tools that do not paginate omit `next_cursor` from their outputSchema.

### Progress Notification Rule

Progress notifications (`$/progress`) are emitted for operations expected to exceed **1 second** wall time. The minimum cadence between notifications is **500 milliseconds** (prevents notification flooding).

Tools that emit progress:

| Tool | Trigger condition |
|------|------------------|
| fs.find | Subtree depth > 3 or estimated match count > 200 |
| archive.tar.create | Archive size estimate > 10 MB |
| archive.tar.extract | Archive size > 10 MB |
| archive.zip.create | Archive size estimate > 10 MB |
| archive.zip.extract | Archive size > 10 MB |
| archive.gzip.compress | Input size > 50 MB |
| archive.hash | Input size > 100 MB |

Progress payload:

```json
{"progress": 0.42, "total": 1.0, "message": "hashing 420/1000 files"}
```

The launch async tools (`launch.up` bucket E; `launch.restart` / `launch.reload` /
`launch.down` bucket C) do NOT use this legacy `$/progress` table: they ride the
MCP Tasks primitive and emit `notifications/tasks/status` keyed by `taskId`
([ADR-0049](0049-mcp-tasks-primitive-adoption.md),
[ADR-0069](0069-launch-tool-cards-toolsearch-and-guidance.md)), with optional
cpu/memory telemetry over a combined `progressToken`.

### Cancellation Propagation

All tools accept cancellation via `$/cancelRequest`. The cancellation token is threaded through the async call chain via `tokio_util::sync::CancellationToken`. Upon cancellation:

1. The tool stops processing and drops all intermediate state.
2. The tool returns a JSON-RPC error with code `-32800` (request cancelled).
3. Partially written files (fs.write) are NOT committed; the original file is preserved.
4. Archive operations abort and clean up temporary files.

### Elicitation Matrix

Elicitation (form-mode, 2025-11-25) is triggered for operations that are destructive or irreversible. The server requests user confirmation before executing:

| Tool | Elicitation trigger | Form fields |
|------|---------------------|-------------|
| fs.remove | Always | `confirm: bool`, `path: string (readonly)` |
| fs.write | When path exists | `overwrite: bool`, `path: string (readonly)` |
| proc.signal | signal in {SIGKILL, SIGTERM, SIGSTOP} | `confirm: bool`, `pid: u32 (readonly)`, `signal: string (readonly)` |
| archive.tar.create | When output path exists | `overwrite: bool`, `dest: string (readonly)` |
| archive.zip.create | When output path exists | `overwrite: bool`, `dest: string (readonly)` |
| fs.set_permissions | mode octal ≤ 0o444 (removing write/exec broadly) | `confirm: bool`, `mode: string (readonly)` |
| launch.trust | Always (authority grant) | `confirm: bool`, `profile: string (readonly)` |
| launch.up | Always (spawns the stack) | `confirm: bool`, `profile: string (readonly)` |
| launch.restart | Always | `confirm: bool`, `service: string (readonly)` |
| launch.reload | Always | `confirm: bool`, `profile: string (readonly)` |
| launch.down | Always (cascade kill) | `confirm: bool`, `stack: string (readonly)` |

When the client does not support elicitation (pre-2025-11-25), the tool returns an error with code `-32001` (elicitation required but unsupported) and takes no action.

### Resources URI Catalog

Substrate exposes on-demand resources (not pre-listed in `resources/list`):

| URI pattern | Content | MIME type |
|-------------|---------|-----------|
| `substrate://docs/{tool_name}` | Full tool documentation (Markdown) | `text/markdown` |
| `substrate://examples/{tool_name}` | Worked usage examples (Markdown) | `text/markdown` |
| `substrate://errors/{code}` | Error code explanation and recovery guide | `text/markdown` |
| `substrate://prompts/{workflow_name}` | Canned workflow prompt template | `text/plain` |

Resources are generated at request time from embedded static assets. The `resources/list` response advertises the URI templates; individual resources are fetched via `resources/read`.

### Consequences

#### Positive

- Single source of truth for all feature usage; implementors and reviewers consult one document.
- Elicitation matrix prevents accidental destructive operations even when the agent runtime is poorly constrained.
- Pagination defaults prevent memory exhaustion on large directory trees.

#### Negative

- Maintaining the annotations matrix manually is error-prone; a macro or derive attribute would be preferable.
- Elicitation fallback (error on unsupported clients) may break older agent runtimes; those runtimes must upgrade to 2025-11-25.

## Validation

- CI runs a schema comparison between the schemars-derived outputSchema and a golden file for each tool.
- Integration tests exercise pagination with page_size=1 to verify cursor correctness across all paginated tools.
- Cancellation tests inject a cancel signal 100ms into a large `fs.find` and assert no result is returned.
- Elicitation tests mock the client form submission and verify that fs.remove does not execute without `confirm: true`.

### Concurrency and Capability Edge Cases

#### Concurrency ceiling

A maximum of **32** simultaneous in-flight tool calls is permitted per session (configurable via `[protocol] max_in_flight_requests`). See [ADR-0005](0005-stdio-transport.md) for the transport-level enforcement and the `-32000` response format.

#### Elicitation timeout

Substrate waits at most **60 seconds** for the client to respond to an `elicit/create` request. On timeout, the tool returns `SUBSTRATE_CONFIRMATION_REQUIRED` with:

```json
{"recovery_hint": "user did not respond within 60s; retry the operation when ready"}
```

The tool **never** proceeds without explicit user confirmation regardless of the timeout path.

#### Elicitation declined vs cancelled distinction

| User action | Error code | `recovery_hint` value |
|---|---|---|
| Explicit decline | `SUBSTRATE_CONFIRMATION_REQUIRED` | `"user declined the operation"` |
| Dialog closed / timeout | `SUBSTRATE_CONFIRMATION_REQUIRED` | `"user did not respond within 60s; retry the operation when ready"` |

#### Elicitation response schema re-validation

Before acting on an `elicit/create` response, substrate MUST re-validate the returned fields against the declared `requestedSchema`. If validation fails (e.g., `confirm: "yes"` when `bool` is expected), the tool returns `SUBSTRATE_INVALID_ARGUMENT` with an `offending_field` key naming the field that failed validation.

#### Nested elicitation prohibited

If a tool handler attempts to send a second `elicit/create` while one is already pending for the same request, the second attempt is rejected with `SUBSTRATE_INTERNAL_ERROR`. Tool handlers must not chain elicitation calls.

#### `content` MUST be non-empty when `structuredContent` is populated

When a tool response includes a `structuredContent` object, the `content` array MUST contain at least one text item carrying a one-line human-readable summary of the result. Older MCP clients that ignore `structuredContent` receive this text as their only display string.

#### Progress capability gating

Substrate emits `notifications/progress` **only** when the client advertised the `progress` capability during `initialize`. When the capability is absent, progress events are silently suppressed; the underlying operation continues and completes normally.

#### Logging capability gating

Substrate emits `notifications/message` (MCP log notifications) **only** when the client advertised the `logging` capability during `initialize`. When the capability is absent, log output goes to stderr only and is never sent over the MCP channel.

#### Cancel-before-progress race

If a cancellation notification arrives before the first progress notification is emitted, the tool aborts immediately. The client receives a JSON-RPC error with code `-32800` (request cancelled). No progress notification ever fires. Tool handlers MUST check their `CancellationToken` at the very start of execution to handle this case.

#### Cancel-vs-completion race

If the tool result is already queued for write when a cancellation notification arrives, the result is delivered to the client. The cancellation becomes a no-op. Per the MCP specification, clients are permitted to receive a valid result for a request they attempted to cancel.

#### Cancel for unknown request id

Silently dropped. Cancellation is a notification (no response expected), and referencing an unknown id is not an error condition.

#### Double-cancel for in-flight request

Idempotent. `tokio_util::sync::CancellationToken` is already-cancelled-safe; signalling it a second time has no effect.

### tools/list Cursor Policy

Substrate currently implements fewer than 30 tools. The `tools/list` response fits in a single page. **Cursor support on `tools/list` itself is NOT implemented in MVP.** Clients must not pass a `cursor` parameter to `tools/list`; doing so is ignored and the full list is returned.

Tool *results* still use cursor pagination (per [ADR-0007](0007-tool-card-narrative-arc.md)) — this restriction applies only to the `tools/list` meta-endpoint.

### Tool Annotation Audit-Log Exception

`readOnlyHint: true` in the annotations matrix refers to the **primary data** the tool accesses. It does NOT account for incidental audit-log writes that substrate may make as a cross-cutting internal concern. Operations such as `fs.read`, `fs.stat`, and `sys.info` are correctly annotated as `readOnlyHint: true` even if an audit record is persisted, because auditing is a side-effect of the infrastructure layer, not of the tool's declared behavior.

## Cross-References

- ADR-0005: STDIO Transport — transport layer over which all these features are delivered; transport-level concurrency cap.
- ADR-0007: Tool Card Narrative Arc Design — how annotations and structuredContent hints relate to tool descriptions; cursor pagination for tool results.
- ADR-0013: MCP Protocol Version Pinning — which features require which minimum version.
- ADR-0032: Signal Safety — cancellation and shutdown signal propagation that interacts with in-flight tool calls.
- ADR-0033: Elicitation Security Model — threat model and rate-limit policy for elicitation flows.
- ADR-0037: Audit Logging — cross-cutting audit-log concern referenced in the annotation exception above.
- [ADR-0060](0060-page-size-value-object-at-domain-port-boundary.md): PageSize value object — domain-port pagination bound (`1..=10000`) that the per-tool handler cap layers on top of.

## Amendments

### 2026-06-10 — Pagination layering reconciled with ADR-0060

The "Pagination Defaults and Cursor Encoding" section above states a maximum
page size of 500 and a default of 50. [ADR-0060](0060-page-size-value-object-at-domain-port-boundary.md)
later introduced a shared `PageSize` value object at the domain-port boundary
with a wider bound. The two are NOT in conflict; they enforce limits at
different layers. This amendment records which layer owns which bound.

- **Domain layer (ADR-0060).** The `PageSize` value object validates
  `MIN = 1`, `MAX = 10_000`, `DEFAULT = 50`. A request with `page_size = 0` or
  `page_size > 10_000` is rejected with `SUBSTRATE_INVALID_ARGUMENT` before any
  handler-level clamp is applied. A second associated default,
  `DEFAULT_PAGINATION = 100`, is used by line- and record-oriented operations
  (subprocess result, search, network TCP/UDP list).
- **Handler / protocol layer (this ADR).** After domain validation succeeds,
  each paginated handler clamps the validated value down to a per-tool ceiling
  via `.get().min(cap)`. The ceilings are: `fs.find`, `proc.list`, and
  `text.search` clamp to 500; `fs.read_dir` clamps to 5_000. The TOML
  `[protocol] max_page_size` (default 500) and `[protocol] default_page_size`
  (default 50) configure this layer. Consequently a request with
  `page_size` in `501..=10_000` is accepted by the domain VO and silently
  clamped down to the per-tool ceiling rather than rejected.

In other words: the domain `PageSize` bound (`1..=10000`) is the outer
validation gate (reject-if-outside); the handler/protocol cap (500, or 5_000
for `fs.read_dir`) is the inner clamp (silently reduce). The "Maximum page
size: 500" figure in the section above is the handler/protocol clamp for
`fs.find`/`proc.list`/`text.search`, not the domain VO ceiling.
