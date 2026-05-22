---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0007 — Tool Card Narrative Arc Design

## Context and Problem Statement

Substrate exposes ~30 tools across namespaces (fs, proc, sys, text, archive). Agent runtimes at the 10B-parameter scale have limited context windows and perform best when each tool description encodes not only what the tool does but also when to use it, what comes next, and what to avoid. Generic OpenAPI-style descriptions are insufficient for autonomous multi-step workflows.

The question: what is the canonical format for tool descriptions in substrate, and how are hints structured for both the model-text and the programmatic structuredContent layers?

## Decision Drivers

- 10B models hallucinate less when tool descriptions include workflow context (USE, NEXT, AVOID).
- Token budget: MCP tool-list responses are sent in full on every session; descriptions must be concise.
- Bifurcation: MCP 2025-06-18+ supports both `content` (human/model text) and `structuredContent` (programmatic JSON); both layers must carry complementary information.
- Consistency: uniform grammar across all tools reduces model confusion.
- Namespace clarity: tool name prefix (`fs.`, `proc.`, `sys.`, `text.`, `archive.`) must be parseable without reading the description.

## Considered Options

1. **Narrative Arc template** — USE / DOES / ARGS / RETURNS / NEXT / AVOID, ≤180 tokens per card.
2. **OpenAPI-style description** — plain prose summary + parameter table; no workflow hints.
3. **Minimal description** — one sentence only; rely on parameter names for semantics.

## Decision Outcome

Chosen option: "Narrative Arc template", because it encodes workflow state into every tool description at a fixed token budget, enabling 10B models to chain tools correctly without external orchestration context.

### Template

```
USE: <when the user/agent would invoke this tool>
DOES: <atomic action performed>
ARGS: <name (type, default) — purpose>; <name (type, default) — purpose>; ...
RETURNS: <shape summary ≤20 tokens>
NEXT: <tool_a>, <tool_b>
AVOID: <anti-pattern> → use <correct_tool> instead
```

**Hard constraints:**
- Total card length: ≤180 tokens (GPT-4o tokenizer reference).
- Each NEXT entry: a valid tool name from the same or adjacent namespace.
- AVOID entry: exactly one anti-pattern and exactly one correction.
- ARGS entries: semicolon-separated; each entry fits on one line.

### Example — fs.find

```
USE: locate files by name glob, extension, or mtime within a directory tree
DOES: recursive walk emitting matching paths with stat metadata
ARGS: root (string) — search root; pattern (string, "*") — glob; max_depth (u32, 16) — recursion limit; modified_since (RFC3339, null) — mtime filter; page_cursor (string, null) — pagination token
RETURNS: {matches:[{path,size,mtime}], next_cursor?}
NEXT: fs.read, fs.stat
AVOID: calling fs.read_dir recursively in a loop → use fs.find with max_depth
```

### 10B Model Rationale

At the 10B scale, models benefit from:
1. **Explicit USE context** — reduces false positives (invoking the tool for the wrong task).
2. **NEXT suggestions** — reduces dead-end states where the model loops on the same tool.
3. **AVOID anti-patterns** — prevents the most common misuse pattern per tool, which at 10B scale tends to be high-frequency without this guard.

The 180-token cap is derived from the observation that 10B models achieve near-saturating accuracy on tool selection at that budget; tokens beyond 180 per card contribute marginally to selection quality but linearly to context cost.

```mermaid
sequenceDiagram
    participant A as Agent Runtime
    participant D as MCP Dispatch Layer
    participant T as Tool Handler
    participant J as Job Registry

    A->>D: tools/call (tool_name, args)
    D->>D: validate args against inputSchema
    D->>T: execute(ctx)

    alt Sync completion (Zone A/B, below bucket threshold)
        T-->>D: ToolOutput
        D-->>A: content (text, <=80 tokens) +\nstructuredContent {hints}
    else Async dispatch (Zone C or bucket-B threshold exceeded)
        T->>J: register job (job_id = UUIDv7)
        J-->>T: job_id
        T-->>D: job_state: pending, job_id
        D-->>A: structuredContent {job_id, job_state: pending,\npolling_endpoint: job.status}
        A->>D: job.status(job_id)
        D->>J: query(job_id)
        J-->>D: state=running, progress_pct
        D-->>A: job_state: running, job_progress_pct
        J->>D: notifications/progress (state=succeeded)
        D-->>A: push notification
        A->>D: job.result(job_id)
        D->>J: retrieve result
        J-->>D: ToolOutput
        D-->>A: content + structuredContent
    end
```

### Bifurcation Rules

Model-text layer (`content`, ≤80 tokens subset of the full card):

```
USE: <one sentence>
DOES: <one sentence>
NEXT: <tool_a>, <tool_b>
AVOID: <anti-pattern> → <correct_tool>
```

Programmatic JSON layer (`structuredContent`):

```json
{
  "tool": "fs.find",
  "args_schema": "<ref to outputSchema>",
  "hints": {
    "next_action_suggested": "fs.read",
    "alternative_tool": "fs.stat",
    "confirm_destructive": false,
    "quota_status": null,
    "error_recovery": "retry with narrower pattern"
  }
}
```

### Hint Grammar Keys

Each key in `hints` carries ≤25 tokens of content:

| Key | Type | Purpose |
|-----|------|---------|
| `next_action_suggested` | string (tool name) | Primary recommended follow-up tool |
| `alternative_tool` | string (tool name) | Secondary option when primary is inapplicable |
| `confirm_destructive` | bool | True when tool mutates or deletes; triggers elicitation |
| `quota_status` | string \| null | Current resource usage note (e.g., "inode 82%") |
| `error_recovery` | string | ≤25-token recovery hint for the most common error path |

### Namespace Convention

Tool names follow `<namespace>.<verb>` with no deeper nesting. Permitted namespaces:

- `fs` — filesystem operations
- `proc` — process management
- `sys` — system information
- `text` — text processing
- `archive` — archive and hash operations

Sub-namespacing (e.g., `archive.tar.create`) is permitted for archive sub-formats where disambiguation is necessary. No other namespace nests deeper than two levels.

### Consequences

#### Positive

- Uniform tool cards enable template-driven documentation generation.
- 10B models chain tools correctly in integration tests without explicit orchestration prompts.
- Bifurcation separates concerns: model reads prose, programmatic client reads JSON hints.

#### Negative

- 180-token cap requires discipline; verbose ARGS lists must be truncated or grouped.
- NEXT and AVOID must be maintained when new tools are added (cross-tool dependency).
- Template enforcement is manual until a lint rule is implemented.

## Validation

- All tool description strings pass a token-count check (≤180) in CI via a custom `xtask check-cards` command.
- Integration tests with a local 10B model (Qwen2.5-10B-Instruct) assert ≥90% correct tool selection on a benchmark of 50 multi-step scenarios.
- `structuredContent.hints` keys are validated against the hint grammar schema in `xtask check-cards`.

## Cross-References

- ADR-0008: MCP Feature Usage Map — outputSchema and structuredContent wire format.
- ADR-0010: (reserved) Error Taxonomy and Recovery Hints.

## Amendments

### 2026-05-21 — Extended by ADR-0040 async-job-control-plane

ADR-0040 introduces a push/pull job orchestration model for long-running tool invocations. This amendment extends the `hints` map in `structuredContent` with six new optional keys that communicate job lifecycle state to clients. The 80-token narrative-arc text body (`content`) is unchanged; the new keys appear exclusively in `structuredContent`.

**Additions:**

- `job_id` (string, UUIDv7 base32-encoded) — serves simultaneously as the async job identifier, the MCP `progressToken`, and the `correlation_id` for log correlation. Present only when the tool invocation is dispatched as an async job.
- `job_state` (string, enum) — current lifecycle state of the job. One of: `pending`, `running`, `succeeded`, `failed`, `cancelled`, `timed_out`. Present only when `job_id` is present.
- `job_progress_pct` (integer, 0-100) — best-effort completion percentage. Present only when `job_id` is present and the underlying operation can report progress.
- `polling_endpoint` (string, enum) — identifies the MCP method the client should call to poll job status or retrieve the result. One of: `job.status`, `job.result`. Present only when `job_id` is present.
- `estimated_completion_ms` (integer, non-negative) — best-effort estimate of remaining latency in milliseconds. Present only when `job_id` is present; omitted when the estimate is unavailable.
- `sequence_number` (integer, monotonic) — monotonically increasing counter within a job; used by clients to detect and discard out-of-order progress events. Present only when `job_id` is present.
- Bucket-A and bucket-D tools that complete within the synchronous request window do not emit any of the six new keys. Presence of `job_id` is the sole signal that a response represents an async job dispatch.

### 2026-05-21 — Extended by ADR-0042 capability-adapter-factory

ADR-0042 introduces a capability adapter factory that selects native adapter tiers at startup. This amendment permits two optional diagnostic annotations in `structuredContent` for responses that traverse a tier-selected adapter path.

**Additions:**

- `simd_tier_used` (string, optional) — records the SIMD tier that was active during the operation (e.g., `avx512`, `avx2`, `sse42`, `sse2`, `neon`, `portable`). Emitted only when the operation executed through a SIMD-accelerated adapter path. Diagnostic; clients MUST NOT branch on this value.
- `walker_tier_used` (string, optional) — records the filesystem walker tier selected by ADR-0042 for the operation (e.g., `native-fts`, `ignore-crate`, `portable`). Emitted only when the operation executed through a walker adapter path. Diagnostic; clients MUST NOT branch on this value.
- Neither annotation counts toward the 80-token text budget of the narrative arc and neither is part of the hint grammar validated by `xtask check-cards`. Both annotations are appended to `structuredContent` outside the `hints` map as top-level sibling keys.

### 2026-05-22 -- MCP + skill synergy (token efficiency)

The original USE/DOES/ARGS/RETURNS/NEXT/AVOID narrative-arc template
prescribed by this ADR proved redundant once the companion SKILL.md
contract (per ADR-0046) and `schemars`-derived `inputSchema` (per the
Wave K wiring) were in place. The description string in
`tools/list` is now thin; the JSON Schema carries args; the companion
skill carries the full lookup reference.

Description budget revised:

- Hard cap reduced from 180 tokens to <= 100 chars (approximately 25
  tokens) per tool description.
- Required content: one-line verb + object describing what the tool
  does; non-trivial behavior tag (destructive + elicitation, async
  job promotion, bucket B threshold) when applicable; literal closing
  phrase `See substrate skill.`.
- Forbidden: USE/DOES/ARGS/RETURNS/NEXT/AVOID labels; ARGS field
  enumeration; RETURNS shape; ADR cross-references; implementation
  framing (Zone A/B/C, syscalls, SIMD, crate names).

The `structuredContent` envelope + hints map (per the original ADR
contract, extended by the 2026-05-21 amendment) is unchanged. The
revision affects only the textual `description` string.

Cross-reference: ADR-0046 Amendment 2026-05-22 -- MCP + skill synergy
contract. The two amendments are coordinated: every byte saved here is
supplemented by the skill body which loads inline only on matching turns
(driven by `triggers:` covering the `mcp__substrate__` namespace plus
dotted tool names).
