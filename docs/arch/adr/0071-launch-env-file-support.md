---
status: accepted
date: 2026-07-01
deciders: [com.archanjo]
consulted: []
informed: []
tags: [launch, subprocess, environment, dotenv, security]
---

# ADR-0071 — Launch `.env` File Support

## Context and Problem Statement

A launch Service declares child environment variables inline via the `env` map in
`.substrate.toml`. Real projects keep environment configuration in `.env` files next
to the code (twelve-factor config, Docker Compose `env_file`, Vite/Spring/Node
conventions). Forcing every variable to be duplicated inline in the profile is
tedious and couples the profile to secrets/config that usually live in a separate,
git-ignored file. Operators asked for the launcher to load `.env` files.

The subprocess BC clears the child environment (`env_clear`) and re-applies only the
explicit `env_override` map, so a Service already receives a minimal, fully-declared
environment. `.env` support must feed that same `env_override` — and must not become
an arbitrary-file-read primitive or a way to smuggle banned variables
(`LD_PRELOAD`, `DYLD_*`) into a child.

## Decision Drivers

- **Ergonomics / convention.** Match the `env_file` model developers already know
  (Compose-style): a per-Service list of `.env` paths.
- **Security.** Reading a caller-named file is a jail concern (ADR-0035); the `.env`
  path must be contained, and its values must pass the same banned-key gate as inline
  `env`.
- **Deterministic precedence.** Overriding order must be unambiguous.
- **No new dependency.** The dotenv format is trivial; adding a crate is not worth the
  supply-chain surface (`cargo deny`/`vet`).

## Decision Outcome

Add an optional `env_file: [...string]` field to `#LaunchService`. At bring-up the
launch BC loads each file, merges the results into the child `env_override`, and
hands that to the subprocess BC unchanged.

- **Containment.** Each `env_file` path is resolved relative to the **profile
  directory** (where `.substrate.toml` lives). Absolute paths and any `..` component
  are rejected; the canonicalized (symlink-resolved) path must remain within the
  canonical profile directory. This gives the same containment discipline as the
  path jail without needing the allowlist threaded into the launch layer, because the
  base is the already-trusted (blessed) profile directory.
- **Precedence.** Files apply in listed order — a later file overrides an earlier one
  — and the inline `env` map overrides all files. This matches Docker Compose (inline
  `environment` beats `env_file`).
- **Parsing.** A small inline parser handles `KEY=VALUE`, an optional `export `
  prefix, `#` comment and blank lines, and single/double-quoted values (double quotes
  honour `\n \t \r \" \\`; single quotes are literal; an unquoted value may carry a
  trailing ` # comment`). Variable interpolation (`${VAR}`) is out of scope for now.
- **Banned keys.** The merged map is the `env_override`, so a `.env` that sets a
  banned variable is rejected by the existing subprocess validation exactly like an
  inline `env` entry. No separate stripping path.
- **Wiring.** Merging happens in `supervisor::spawn_service` (the single funnel every
  launch spawn path uses, alongside PATH resolution per ADR-0070), so `up`, `reload`,
  `restart`, and the detached supervisor all honour `env_file` uniformly. A read/parse
  failure fails the Service with `SUBSTRATE_LAUNCH_*` (`InvalidProfile`), surfacing the
  bad file rather than silently starting with a partial environment.

## Consequences

- **Positive.** Profiles stay small; config/secrets live in `.env` files as
  developers expect. No new dependency. Uniform across every launch path.
- **Positive (security).** The execution/read boundary is unchanged in spirit:
  `.env` files are confined to the trusted profile directory, and their values are
  gated by the same banned-key validation as inline `env`.
- **Negative / accepted.** No `${VAR}` interpolation yet; values are literal. A
  missing or unreadable `env_file` is a hard error (chosen over silent-empty so a
  typo is caught). `.env` values are not treated as secrets by the vault — they are
  passed to the child as-is; operators keep `.env` out of version control themselves.

## Links

- [ADR-0063](0063-launch-orchestration-bounded-context.md) — launch bounded context.
- [ADR-0064](0064-launch-profile-trust-model.md) — profile trust model; the profile
  directory is trusted once blessed, which anchors `.env` containment.
- [ADR-0070](0070-launch-path-binary-resolution.md) — PATH binary resolution; the
  companion per-Service input resolved in the same `spawn_service` funnel.
- [ADR-0035](0035-path-safety-hardening.md) — path safety; the canonicalize-then-
  contain discipline reused here for the `.env` read.
