# Security Policy

## Supported versions

Pre-1.0: only the latest `0.x` minor receives fixes. Once `1.0.0` ships, the
two most recent minor versions will be supported.

| Version | Supported |
|---|---|
| 0.2.x | yes (latest) |
| 0.1.x | no |
| < 0.1.0 | no |

## Reporting a vulnerability

Email **farchanjo@gmail.com** with subject prefix `[substrate-security]`.
Do **not** open a public issue, PR, or discussion for a vulnerability.

Please include:

- substrate version (`substrate-mcp-server --version`).
- Affected tool name and bounded context.
- Reproduction steps (config snippet, JSON-RPC request, expected vs observed
  behavior). Minimal repro preferred.
- Impact assessment (confidentiality, integrity, availability).
- Optional: proposed fix or patch.
- Optional: PGP-encrypted body if the issue is sensitive (request the public
  key in the first message).

## Response timeline

| Step | Target |
|---|---|
| Acknowledge receipt | 72 hours |
| Initial triage + severity rating (CVSS 3.1) | 7 days |
| Fix in private branch | 30 days (best effort) |
| Public disclosure + CVE (if applicable) | After fix release |

## Scope

In scope:

- The `substrate-mcp-server` binary and any `substrate-*` adapter crate.
- The security model (allowlist, path jail, dry-run, elicitation) defined in
  [ADR-0004](docs/arch/adr/0004-security-model.md) and
  [ADR-0035](docs/arch/adr/0035-path-safety-hardening.md).
- The audit-event taxonomy ([ADR-0038](docs/arch/adr/0038-audit-event-semantics.md))
  and error taxonomy ([ADR-0010](docs/arch/adr/0010-error-taxonomy.md)).
- Supply-chain hygiene (Cargo manifest, build scripts, release artifacts).

Out of scope:

- Misconfiguration of the operator's host OS or filesystem permissions
  (e.g., running substrate as root with a `/` root in the allowlist).
- Issues in upstream crates that have an existing advisory (file with the
  upstream first; reference here only for substrate-side mitigation).
- Denial-of-service caused by exhausting OS-level limits not controlled by
  substrate (file descriptors, memory, inodes).

## Coordinated disclosure

We coordinate disclosure with the reporter. The default is a 90-day window
between report receipt and public disclosure; this can be shortened (active
exploitation) or extended (complex multi-component fix) by mutual agreement.

## Security advisories

Published as GitHub Security Advisories on the `farchanjo/substrate`
repository and referenced from the `CHANGELOG.md` entry for the release that
contains the fix.

## Hardening references

- [ADR-0004 — Security model](docs/arch/adr/0004-security-model.md): allowlist + path jail + dry-run + elicitation layers.
- [ADR-0035 — Path safety hardening](docs/arch/adr/0035-path-safety-hardening.md): `openat2(RESOLVE_BENEATH | NO_SYMLINKS)` (Linux) and `O_NOFOLLOW_ANY` (macOS).
- [ADR-0032 — Signal safety](docs/arch/adr/0032-signal-safety.md): SIGPIPE handling, blake3 mmap disabled.
- [ADR-0044 — No subprocess policy](docs/arch/adr/0044-no-subprocess-policy.md): no `std::process::Command` in shipping crates.
- [ADR-0029 — Threat model](docs/arch/adr/0029-threat-model.md): documented attacker model and trust boundaries.
