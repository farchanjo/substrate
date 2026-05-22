---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0027 — MCP Protocol Migration Path

## Context and Problem Statement

The MCP specification releases new date-versioned revisions periodically. Each revision may introduce features substrate wants to adopt, or deprecate behaviors substrate relies on. Without a defined policy, version bumps happen ad hoc, breaking agent runtimes that have not yet upgraded. Equally, delaying adoption indefinitely leaves substrate behind the spec and unable to use new capabilities.

The question: what is the policy for adopting new MCP spec versions, for simultaneously supporting multiple versions, and for deprecating and removing old version support?

## Decision Drivers

- Stability: agent runtimes (CI pipelines, production deployments) must not break on a substrate patch release.
- Velocity: substrate should be able to adopt new MCP features within a reasonable window of spec publication.
- Predictability: operators must know in advance when a minimum version will be bumped.
- Simplicity: supporting arbitrarily many versions simultaneously is not feasible in a single-binary server.

## Considered Options

1. **30-day stability window + N and N-1 simultaneous support + MINOR bump for min-version change** — structured adoption with explicit deprecation cycle.
2. **Always on latest** — immediately adopt every new spec version; drop old support on same release.
3. **Manual per-version ADR** — no general policy; each version migration decided independently.

## Decision Outcome

Chosen option: "30-day stability window + N and N-1 simultaneous support + MINOR bump for min-version change", because it balances adoption velocity with operational stability and gives operators a predictable window to upgrade their agent runtimes.

### 30-Day Stability Window

When a new MCP spec version is published:

1. Substrate evaluates the new version within 7 days of publication.
2. Substrate may begin implementation immediately but does not change the **preferred version** in `ServerInfo` until 30 days after spec publication.
3. The 30-day window allows the MCP ecosystem (SDKs, agent runtimes, test harnesses) to stabilize around the new spec before substrate advertises it.
4. Exception: if the new spec fixes a security vulnerability, the stability window is waived and substrate adopts immediately.

### N and N-1 Simultaneous Support

Substrate supports at most two spec versions simultaneously:

- **N** (preferred): the version substrate negotiates with up-to-date clients.
- **N-1** (previous minimum): the version still accepted but with degraded capabilities.

When a new version N+1 is adopted, N becomes the new minimum and N-1 is dropped. The three-version range (N+1, N, N-1) is never simultaneously supported; complexity caps at two.

Current state (as of this ADR):

| Role | Version |
|------|---------|
| Preferred (N) | `2025-11-25` |
| Minimum (N-1) | `2025-06-18` |

### Bumping the Minimum Version

Bumping the minimum version (dropping N-1 support) is a **MINOR** substrate version increment per semantic versioning:

```
substrate 0.2.0 — drops 2025-06-18; new minimum: 2025-11-25
substrate 0.3.0 — drops 2025-11-25; new minimum: <next spec>
```

Rationale: dropping a minimum version is a breaking change for agent runtimes on that version. MINOR rather than MAJOR is used because substrate is pre-1.0 and MINOR already signals breaking changes in the 0.x range. Post-1.0, dropping minimum version support will be a MAJOR increment.

A PATCH release never changes the minimum version.

### Deprecation — One-Cycle Warning

Before dropping support for a minimum version, substrate must:

1. Emit a deprecation warning in the `initialize` response `_meta` field for clients on the about-to-be-dropped version:

```json
{
  "_meta": {
    "deprecation_warning": "protocol version 2025-06-18 support will be removed in substrate 0.3.0; upgrade to 2025-11-25 or later"
  }
}
```

2. Publish a CHANGELOG entry and GitHub release note at least one substrate MINOR release (one cycle) before removal.
3. The deprecation warning must be present for at least one released MINOR version before the version is dropped.

This guarantees operators receive at least one published release with the warning before the breaking change.

### Adoption Checklist for a New Spec Version

When evaluating a new spec revision for adoption:

- [ ] Read the diff from the previous spec version; catalogue new features and removed/changed behaviors.
- [ ] Determine which new features substrate will use (update ADR-0008 if needed).
- [ ] Implement new features behind a capability flag gated on the negotiated version.
- [ ] Add integration tests for the new version's handshake and new capabilities.
- [ ] Wait 30 days from spec publication (or waive for security fixes).
- [ ] Update `preferred_version` in server initialization.
- [ ] Add deprecation warning for the version being demoted to N-1.
- [ ] Bump substrate MINOR version.
- [ ] Update ADR-0013 with new version table.

### Consequences

#### Positive

- Operators have a predictable 30-day window before substrate advertises a new spec version.
- The one-cycle deprecation warning gives at least one release cycle to migrate.
- MINOR bumps for minimum-version changes signal breaking changes clearly in the version number.

#### Negative

- Substrate never simultaneously supports more than two versions; edge cases with three-generation spans in the wild require intermediate upgrades.
- The 30-day window may delay adoption of features that are immediately useful (security fixes exempted).
- Maintaining N and N-1 simultaneously adds conditional paths in capability negotiation logic.

## Validation

- Policy compliance is verified in the release checklist (GitHub Actions release workflow).
- Integration tests assert that a client on the about-to-be-dropped version receives the `_meta.deprecation_warning` field.
- The CHANGELOG for each MINOR release is linted to confirm the min-version bump is documented.

## Cross-References

- ADR-0013: MCP Protocol Version Pinning — current minimum and preferred versions, and the negotiation flow this policy governs.
