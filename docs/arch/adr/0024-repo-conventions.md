---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0024 — Repository Conventions

## Context and Problem Statement

A consistent set of repository conventions enables automated tooling (changelog generation, CI scope targeting, MR description linting) and reduces reviewer cognitive load. Without explicit conventions, commit messages drift, branch names become ambiguous, and the audit trail connecting commits to architectural decisions is lost.

Conventions must be lightweight enough that contributors adopt them without tooling overhead, yet structured enough that CI can enforce them automatically.

## Decision Drivers

- Angular commit format is machine-parseable for changelog generation and CI scope targeting.
- Commit scope must map to crate names to allow `cargo test -p <scope>` targeted runs.
- Small contextual commits reduce bisect surface and simplify revert.
- DCO sign-off replaces CLA (see ADR-0021) and requires a verifiable `Signed-off-by` trailer.
- Branch protection on `main` ensures no direct pushes and requires CI to pass before merge.
- MR templates must reference the ADR that motivates the change, creating a durable audit trail.

## Considered Options

1. Conventional Commits (superset of Angular) — compatible, but broader type vocabulary increases variance.
2. Angular commit format — subset of Conventional Commits; tighter type list; toolchain well-supported.
3. Free-form commit messages with mandatory issue reference — no changelog automation benefit.
4. Gitmoji prefixes — visually distinct but not machine-parseable without emoji-to-type mapping.

## Decision Outcome

Chosen option: "Angular commit format", because it is already enforced by the project's existing tooling scripts and aligns with the `commitlint` configuration in `.commitlintrc.cjs`.

### Commit Format

```
<type>(<scope>): <subject>

[optional body]

[optional footers]
Signed-off-by: Full Name <email@example.com>
```

**Allowed types:**

| Type | Use |
|---|---|
| `feat` | New tool or capability visible to MCP consumers |
| `fix` | Bug fix in existing tool behavior |
| `perf` | Performance improvement (no behavioral change) |
| `refactor` | Internal restructure (no behavioral or interface change) |
| `test` | Test additions or corrections only |
| `docs` | Documentation only |
| `chore` | Maintenance (deps, toolchain, CI config) |
| `build` | Build system or `Cargo.toml` changes |
| `ci` | CI pipeline configuration |
| `revert` | Reverts a previous commit (reference SHA in body) |

**Scope:** must match a crate directory name (`substrate-core`, `substrate-fs`, `substrate-proc`, `substrate-sys`, `substrate-text`, `substrate-archive`, `substrate-net`, `substrate-mcp-adapter`) or one of the reserved scopes `spec`, `ci`, `deps`, `release`.

**Subject:** imperative mood, lowercase, no trailing period, ≤72 characters total line length.

**Breaking changes:** append `!` after scope and add `BREAKING CHANGE:` footer.

### Branch Naming

| Prefix | Use |
|---|---|
| `feat/<short-description>` | New feature or tool |
| `fix/<short-description>` | Bug fix |
| `chore/<short-description>` | Maintenance, deps, CI |
| `docs/<short-description>` | Documentation only |
| `release/<version>` | Release preparation |

Branch names use lowercase kebab-case only. No issue numbers in the branch name (use MR description instead).

### Commit Hygiene

- Commits must be small and contextual: one logical change per commit.
- `git add -p` is the preferred staging workflow; avoid `git add .` bulk staging.
- Merge commits are forbidden on `main`; rebase-merge is the only merge strategy.
- Do not squash unless the MR author explicitly requests it and the reviewer agrees.

### Branch Protection (main)

| Rule | Setting |
|---|---|
| Direct push | Forbidden |
| MR required | Yes |
| CI required | All jobs through `security` stage must pass |
| Approvals required | 1 approval from a `Maintainer` |
| Rebase merge only | Yes (no merge commits, no squash-merge by default) |
| DCO check | CI lint job `check-dco` must pass |

### DCO Sign-off

Every non-merge commit must carry:

```
Signed-off-by: Full Name <email@example.com>
```

Contributors assert DCO v1.1 by including this trailer. Use `git commit -s` to append automatically. The CI job `check-dco` runs `git log --format='%H %s' origin/main..HEAD` and verifies each commit hash has a matching `Signed-off-by` trailer. Unsigned commits block MR merge.

No CLA is required (see ADR-0021).

### MR / PR Template

The `.gitlab/merge_request_templates/Default.md` template requires:

- **What**: one-paragraph summary of the change.
- **Why**: motivation and problem being solved.
- **Related ADR**: link to the ADR motivating the change, or "N/A — no architectural impact".
- **Testing**: how the change was tested.
- **Checklist**: `cargo fmt --check`, `cargo clippy`, `cargo test`, DCO trailers present.

### Consequences

#### Positive

- Angular commit format enables automated `CHANGELOG.md` generation via `git-cliff` or `conventional-changelog`.
- Scope-to-crate mapping allows targeted test runs in CI, reducing feedback time on large MRs.
- DCO trailer verification provides a lightweight contribution provenance record without CLA infrastructure.
- Rebase-merge strategy produces a linear `main` history that is trivially bisectable.

#### Negative

- Contributors unfamiliar with Angular format must learn the type vocabulary; onboarding docs required.
- Rebase-merge requires contributors to rebase against `main` before merge; large parallel MRs create rebase churn.
- DCO CI check fails on co-authored commits if `Co-authored-by:` appears without a matching `Signed-off-by:` for each identity.

## Validation

- `commitlint --from origin/main` runs in the `lint` CI stage on every MR.
- `check-dco` CI job verifies `Signed-off-by` on every non-merge commit in the MR.
- Branch naming enforced via GitLab push rule regex: `^(feat|fix|chore|docs|release)\/[a-z0-9-]+$`.
- `spec validate --lane fast` in the `validate` stage catches ADR cross-reference drift.

## Cross-References

- ADR-0021: License (DCO sign-off replaces CLA; dual-license context)
- ADR-0023: CI/CD pipeline (commitlint, check-dco, and branch protection enforcement)
