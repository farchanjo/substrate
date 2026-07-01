---
status: accepted
date: 2026-07-01
deciders: [com.archanjo]
consulted: []
informed: []
tags: [launch, subprocess, binary-resolution, path, security, trust]
---

# ADR-0070 — Launch PATH-Based Binary Resolution

## Context and Problem Statement

A launch Profile declares each Service command in argv form, `command[0]` being the
binary ([ADR-0064](0064-launch-profile-trust-model.md)). Until now `command[0]` had
to be an absolute path: the launch BC wrapped it verbatim into a `SubprocessRequest`,
and the subprocess adapter canonicalizes `binary_path` before the binary-allowlist
check. A bare name such as `node`, `java`, or `mvn` is canonicalized relative to the
server's working directory, not `$PATH`, so it fails with `SpawnFailed` (ENOENT) — the
subprocess `binary_path` contract in `subprocess.cue` states it "MUST be an absolute
path".

That is hostile to how developers actually declare processes. Every real toolchain
lives on `$PATH` (`node`, `pnpm`, `java`, `gradle`), and a repo-relative launcher such
as `./gradlew` is idiomatic. Forcing operators to hard-code `/opt/homebrew/bin/node`
(different on Linux, different per machine) makes Profiles non-portable and couples
them to install layout.

The tension is that resolving a bare name via `$PATH` reintroduces exactly the
trust-order-confusion class ADR-0064 exists to defend against: the resolved binary
now depends on `$PATH` content and ordering, which an earlier poisoned entry could
hijack.

## Decision Drivers

- **Portability / DX.** A Profile should name binaries the way a shell does — bare
  name resolved on `$PATH`, `cwd`-relative path, or absolute — not hard-code install
  paths.
- **The binary allowlist stays the security boundary.** Whatever resolution produces,
  it must still pass the unchanged subprocess canonicalize + allowlist + regular-file
  gate ([ADR-0052](0052-subprocess-execution-architecture.md)). Resolution may only
  produce a *candidate*; it must never widen what is permitted to execute.
- **Preserve the subprocess contract.** `subprocess.cue`'s `binary_path` "MUST be
  absolute" invariant should remain true, so the subprocess BC is unaffected and only
  the launch input surface relaxes.
- **No new attack surface beyond the allowlist.** PATH-order hijacking must be bounded
  by the allowlist, not merely by the resolution algorithm.

## Considered Options

1. **Keep requiring absolute paths.** Simplest, no new surface — but poor DX and
   non-portable Profiles; rejected.
2. **Resolve inside the subprocess adapter for all spawns.** Would also let
   `subprocess.spawn` accept bare names. Rejected: it broadens a security-sensitive
   tool's input contract beyond the reported need and changes `subprocess.cue`.
3. **Resolve in the launch layer only, then pass an absolute path to subprocess
   (chosen).** The launch BC resolves `command[0]` to an absolute path before building
   the `SubprocessRequest`; the subprocess adapter and its `binary_path` contract are
   untouched.
4. **Pin the resolved path into the trust record at bless time.** Eliminates
   run-time PATH drift but adds trust-record/allowlist-drift complexity. Rejected: the
   allowlist already gates execution, so pinning buys little for real cost.

## Decision Outcome

Chosen: **option 3** — PATH-aware resolution in the launch layer, gated by the
unchanged subprocess allowlist.

`supervisor::spawn_service` resolves `request.binary_path` immediately before handing
the request to the injected `SubprocessPort`, so it covers every launch spawn path
(in-process bring-up, reload, restart, and the detached supervisor). Resolution rules
mirror shell semantics:

- **Absolute path** — used unchanged.
- **Relative path containing a separator** (`./gradlew`, `bin/tool`) — resolved
  against the Service's own `cwd`, never `$PATH`.
- **Bare name** (no separator) — searched on `$PATH`; the first entry that is a
  regular, executable file (exec bit via `std::os::unix::fs::PermissionsExt::mode()`)
  wins.
- **Any miss** — the original value is returned unchanged, so the subprocess adapter
  surfaces the canonical `SUBSTRATE_SUBPROCESS_BINARY_NOT_ALLOWED` / `SpawnFailed`
  error. No new error code is introduced.

The resolved absolute path is then subject to the **unchanged** subprocess pipeline:
canonicalize (symlink-resolving), require containment in the configured
`binary_allowlist` (literal or glob) after canonicalization, and require a regular
file. Therefore a poisoned earlier-`$PATH` `node` executes only if its resolved
canonical path is itself allowlisted — the allowlist, not `$PATH`, remains the trust
anchor. Resolution is performed live at each `launch.up` rather than pinned at bless
time, precisely because the allowlist is the boundary and the resolution is only a
convenience.

## Consequences

- **Positive.** Profiles become portable: `command = ["node", "server.js"]`,
  `["java", "-jar", "app.jar"]`, or `["./gradlew", "bootRun"]` all work, matching
  developer expectation. The subprocess BC and `subprocess.cue` are unchanged.
- **Positive (security).** The execution boundary is exactly what it was — the
  canonicalized binary allowlist. PATH resolution cannot execute anything the
  allowlist does not already permit.
- **Negative / accepted.** The resolved binary is host- and `$PATH`-dependent: the
  same `"node"` may resolve differently on different hosts. This is intended (shell
  parity) and bounded by the allowlist; operators who need a pinned binary declare an
  absolute path.
- **Operational.** Operators must allowlist the resolved install directories
  (typically via globs such as `/opt/homebrew/bin/*`, `/usr/local/bin/*`,
  `~/.asdf/shims/*`) for bare-name Services to launch; otherwise the Service fails
  with `BINARY_NOT_ALLOWED`, which is the correct default-deny behavior.

## Links

- [ADR-0064](0064-launch-profile-trust-model.md) — launch profile trust model; command
  argv form. This ADR extends its command model with PATH/`cwd`-relative resolution;
  the allowlist trust boundary is unchanged.
- [ADR-0052](0052-subprocess-execution-architecture.md) — subprocess execution; the
  binary-allowlist canonicalization gate that resolution feeds into, unchanged.
- [ADR-0035](0035-path-safety-hardening.md) — path safety hardening; the same
  canonicalize-then-contain discipline applied to `cwd` and stdin paths.
- [ADR-0063](0063-launch-orchestration-bounded-context.md) — launch bounded context.
