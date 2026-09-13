---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0021 — License (MIT/Apache-2.0 Dual)

## Context and Problem Statement

Substrate is a Rust project distributed as open-source software. The Rust ecosystem has established a strong convention of dual-licensing under MIT and Apache-2.0. Contributors and downstream consumers need clear, legally unambiguous terms. A contributor agreement (CLA) creates friction for small-team and individual contributors.

The project must satisfy the following requirements:

- Compatible with the Rust standard library and all `crates.io` dependencies.
- No patent retaliation asymmetry for contributors (Apache-2.0 patent clause).
- Permissive enough for embedding in commercial products.
- No CLA overhead; lightweight sign-off via DCO.

## Decision Drivers

- Rust ecosystem convention — virtually all foundational crates (`tokio`, `serde`, `thiserror`, `clap`) are `MIT OR Apache-2.0`.
- Apache-2.0 patent grant protects contributors and users from patent retaliation.
- MIT is maximally permissive and compatible with GPL-2.0-only consumers.
- DCO (Developer Certificate of Origin) provides provenance without a CLA.
- `cargo-deny` can enforce license policy to block incompatible transitive dependencies.

## Considered Options

1. MIT-only — simpler, but no patent grant.
2. Apache-2.0-only — patent grant, but incompatible with GPL-2.0-only consumers.
3. MIT OR Apache-2.0 dual — maximum compatibility, Rust convention, patent protection.
4. MPL-2.0 — copyleft at the file level; incompatible with the permissive-open goal.

## Decision Outcome

Chosen option: "MIT OR Apache-2.0 dual", because it is the Rust ecosystem standard, provides patent protection via Apache-2.0, and is maximally compatible with downstream consumers of all types.

### Implementation

**Repository root artifacts:**

- `LICENSE-MIT` — full MIT license text, copyright `com.archanjo`.
- `LICENSE-APACHE` — full Apache-2.0 license text, copyright `com.archanjo`.

**`Cargo.toml` (workspace root):**

```toml
[workspace.package]
license = "MIT OR Apache-2.0"
```

All member crates inherit via `license.workspace = true`. No crate may override to a different license without an ADR amendment.

**SPDX identifier in every crate `Cargo.toml`:**

```toml
license.workspace = true
```

The workspace-level SPDX expression `MIT OR Apache-2.0` satisfies SPDX 3.x `LicenseExpression` requirements.

**`cargo-deny` licenses policy (`deny.toml`):**

```toml
[licenses]
allow = [
  "MIT",
  "Apache-2.0",
  "MIT OR Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unicode-DFL",
  "CC0-1.0",
]
deny = [
  "GPL-2.0-only",
  "GPL-3.0-only",
  "AGPL-3.0-only",
  "LGPL-2.0-only",
  "LGPL-3.0-only",
]
```

CI fails on any dependency introducing a denied license.

**DCO sign-off:**

Every commit MUST carry a `Signed-off-by:` trailer:

```
Signed-off-by: Full Name <email@example.com>
```

No CLA is required. Contributors assert the DCO v1.1 by including the trailer. The CI lint job (`scripts/check-dco.sh`) verifies all commits on non-draft MRs.

### Consequences

#### Positive

- Zero CLA friction; contributors sign off with a single `git commit -s`.
- `cargo-deny` enforces license policy in CI, preventing GPL contamination automatically.
- Dual-license satisfies the broadest range of downstream users without negotiation.

#### Negative

- Two `LICENSE-*` files must be kept in sync with copyright year updates.
- Any crate vendored outside `crates.io` requires manual license review and `deny.toml` exemption.

## Validation

- `cargo deny check licenses` passes on every CI run (validate stage).
- MR CI lint step verifies `Signed-off-by` trailer on every non-merge commit.
- `docs/arch/schemas/cargo-workspace.cue` asserts `license == "MIT OR Apache-2.0"` for all workspace members.

## Cross-References

- ADR-0023: CI/CD pipeline (cargo-deny runs in the security stage)
- ADR-0024: Repository conventions (DCO sign-off requirement, commit format)
