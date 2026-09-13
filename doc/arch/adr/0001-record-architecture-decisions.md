---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# Record architecture decisions

## Context and Problem Statement

The substrate architecture needs a durable, reviewable record of
significant decisions. How should those decisions be captured?

## Decision Drivers

- A decision must be reviewable in the same pull request as the change it
  constrains, so rationale and code are read together.
- The record must be machine-validatable: one fixed template keeps every entry
  uniform enough for a validator to check structure.
- Reading and writing a decision must need no external account or hosted service.
- A superseded decision must stay readable, so the history is reconstructible
  from the repository alone.

## Considered Options

- MADR 4.0 markdown files under `adr/`
- An external wiki
- No formal record

## Decision Outcome

Chosen option: "MADR 4.0 markdown files under `adr/`", because the
records live beside the schemas they constrain and are validated by
`speckit validate`.

### Consequences

#### Positive

- Decisions are versioned alongside the code and schemas they constrain,
  reviewable in the same pull request rather than in a separate system.
- `speckit validate` enforces the MADR 4.0 structure across every record,
  preventing drift between decisions.
- No external tooling or account (wiki login, wiki hosting) is required to
  read or write a decision.

#### Negative

- ADR authors must learn and follow the MADR 4.0 template; free-form notes
  are not accepted by the validator.
- Superseding a decision requires a new numbered file plus cross-reference
  links rather than an in-place wiki edit.
