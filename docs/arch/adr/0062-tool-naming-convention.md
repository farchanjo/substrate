---
status: accepted
accepted_date: 2026-06-10
date: 2026-06-10
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0062 — Tool Naming Convention: Logical Dot-Notation vs. Wire Underscore-Notation

## Context and Problem Statement

Substrate tool names appear in two distinct contexts that have conflicting
character-set constraints:

1. **Spec artifacts** (ADRs, Gherkin features, CUE schemas, Rego policies,
   glossary, tool-card prose, domain code comments) — these documents use
   `<bc>.<verb>` dot-notation, for example `fs.find`, `proc.signal`,
   `net.tcp_list`. The dot communicates bounded-context ownership clearly and is
   idiomatic in DDD ubiquitous language.

2. **MCP wire protocol** (the `name` field in `tools/list` and `tools/call`) —
   JSON-RPC tool names are transmitted as bare strings. Many MCP client SDKs,
   shell variable interpolation, and downstream routing layers treat `.` as a
   structural separator or disallow it entirely. The wire names already shipped
   with underscores (`fs_find`, `proc_signal`, `net_tcp_list`) and are relied
   upon by all existing clients and integration tests.

The drift between the two forms has caused confusion: contributors cannot tell
whether `fs.find` and `fs_find` refer to the same tool, whether one form is
authoritative, and which form to use in a given document. Without a decision,
future ADRs, Gherkin scenarios, and code will continue mixing the two styles,
making automated cross-referencing and lint impossible.

The question: which naming form is canonical for which purpose, and what is the
deterministic rule relating the two forms?

## Decision Drivers

- **Client compatibility**: wire names (`fs_find`) are already shipped and
  tested; renaming them would break every existing MCP client and integration
  test without any user-facing benefit.
- **Ubiquitous language**: dot-notation (`fs.find`) is more readable in spec
  prose and communicates bounded-context membership without requiring readers to
  infer which prefix is the BC name and which is the verb.
- **One-to-one reversibility**: a deterministic mapping rule must exist so that
  documentation generators, linters, and spec-framework validators can
  unambiguously translate between the two forms without a lookup table.
- **Lint feasibility**: both forms must be recognizable by automated tooling
  (conftest, MADR linter, CUE evaluator) without teaching each tool a
  special-case list.

## Considered Options

1. **Dot-notation everywhere** — adopt `fs.find` as the wire name too. Fixes
   the inconsistency but breaks all existing MCP clients and requires
   coordinated SDK updates.

2. **Underscore-notation everywhere** — adopt `fs_find` in all specs and docs.
   Eliminates confusion but sacrifices DDD readability in bounded-context prose
   and forces every ADR and Gherkin scenario to use syntax that obscures
   BC membership.

3. **Dual-notation with a deterministic mapping rule** (chosen) — define
   `<bc>.<verb>` as the *logical name* used in specs, ADRs, Gherkin, glossary,
   and tool-card prose; define `<bc>_<verb>` (single `.` → `_` substitution) as
   the *wire name* used in the MCP protocol. Both forms are valid references to
   the same tool; the wire name is authoritative for the protocol; the logical
   name is authoritative for domain semantics.

## Decision Outcome

Chosen option: "Option 3 — dual-notation with a deterministic mapping rule".

### Mapping Rule

> Replace every `.` (U+002E FULL STOP) in the logical name with `_` (U+005F
> LOW LINE) to obtain the wire name. The substitution is applied globally (all
> occurrences), is one-to-one, and is reversible: replace every `_` that
> separates an alphabetic character from another alphabetic character in a wire
> name with `.` to recover the logical name.

Formal relation:

```
wire_name  = logical_name.replace('.', '_')
logical_name = wire_name.replace('_', '.')   // valid only within the tool-name namespace
```

Examples:

| Logical name (spec) | Wire name (MCP) |
|---|---|
| `fs.find` | `fs_find` |
| `fs.read_dir` | `fs_read_dir` |
| `proc.signal` | `proc_signal` |
| `sys.load_average` | `sys_load_average` |
| `net.tcp_list` | `net_tcp_list` |
| `archive.tar.create` | `archive_tar_create` |
| `subprocess.spawn` | `subprocess_spawn` |
| `job.result` | `job_result` |

Note: `fs.read_dir` is a deliberate example showing that underscores within a
verb segment (`read_dir`) are preserved under the reverse mapping without
ambiguity because the BC prefix is always a single token before the first
separator.

### Authoritativeness per Context

| Context | Authoritative form | Rationale |
|---|---|---|
| MCP `tools/list` name field | Wire (`fs_find`) | Protocol layer; SDK and client compatibility |
| MCP `tools/call` method name | Wire (`fs_find`) | Protocol layer |
| ADRs, Gherkin scenarios, Rego policies | Logical (`fs.find`) | DDD ubiquitous language; BC membership visible |
| CUE schemas (tool description strings) | Logical (`fs.find`) | Spec artifacts; BC-oriented |
| Glossary terms | Logical (`fs.find`) | Ubiquitous language |
| Tool-card USE / DOES / NEXT / AVOID prose | Logical (`fs.find`) | Human-readable narrative |
| Rust source code — `tool.name` field | Wire (`fs_find`) | Runtime value sent over MCP |
| Rust source code — comments, doc strings | Either; prefer logical | Readability |
| Integration test identifiers | Wire (`fs_find`) | Test names match wire protocol |
| `structuredContent.hints.next_action_suggested` | Wire (`fs_find`) | Machine-readable; consumed by SDK code |

### Linter and Doc-Generation Contract

Automated tooling (spec-framework validators, MADR linter, Gherkin step
parsers, CUE evaluators, doc generators) MUST apply the mapping rule to
cross-reference logical and wire names. A tool is considered to be the same
entity if and only if its logical name and wire name satisfy the mapping rule
above. Lint rules MUST NOT require an explicit lookup table; the mapping rule
is the sole source of truth.

### No Rename Policy

The existing wire names (`fs_find`, `proc_signal`, etc.) are frozen. No wire
name may be renamed without superseding this ADR and coordinating a
client-breaking migration plan documented in a dedicated ADR. Spec artifacts
that currently use wire-name style (underscore in prose) are not required to be
retroactively updated; however, all new spec text SHOULD use the logical form.

## Consequences

### Positive

- The ambiguity between `fs.find` and `fs_find` is resolved: they are the same
  tool, related by a deterministic rule.
- No client breakage: wire names are unchanged.
- Spec prose retains DDD readability: `proc.signal` communicates that the tool
  belongs to the `process` bounded context.
- Automated tooling can validate cross-references without a manually maintained
  lookup table.

### Negative

- Contributors must learn that `fs.find` (spec) and `fs_find` (wire) are the
  same tool. A brief note in `CONTRIBUTING.md` is sufficient mitigation.
- The reverse mapping (`_` → `.`) is context-dependent: it applies only within
  the tool-name namespace and must not be applied to identifiers that contain
  underscores for reasons other than BC separation (e.g., a hypothetical tool
  `fs.read_dir` maps to `fs_read_dir`, and reversing `fs_read_dir` yields
  `fs.read.dir`, which is wrong). The mapping rule therefore applies
  *forward* (logical → wire) without restriction, and *backward* (wire →
  logical) only by treating the first `_`-delimited token as the BC prefix and
  the remainder as the verb. Linters MUST implement the backward mapping as
  "first token = BC, rest = verb" rather than "replace all `_` with `.`".

## Validation

- `spec validate --lane fast` passes with no MADR lint errors on this file.
- A new `spec lint:naming` rule (to be authored in a follow-up task) will verify
  that every tool name referenced in Gherkin `.feature` files uses the logical
  dot-notation form and that the corresponding wire name satisfies the mapping
  rule against the tool registry.
- Existing integration tests referencing wire names (e.g., `fs_find`) continue
  to pass without modification.

## Links

- Related: [ADR-0002](0002-bounded-contexts.md) — bounded contexts that define
  the `<bc>` prefix tokens used in logical names.
- Related: [ADR-0007](0007-tool-card-narrative-arc.md) — tool-card design that
  uses logical names in USE / DOES / NEXT / AVOID prose and in the
  `structuredContent.hints` wire fields.
- Related: [ADR-0008](0008-mcp-features-map.md) — feature matrix using logical
  names in the tool annotations table.
