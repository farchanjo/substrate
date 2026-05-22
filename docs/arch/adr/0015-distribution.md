---
status: accepted
date: 2026-05-21
deciders: [com.archanjo]
consulted: []
informed: []
---

# ADR-0015 — Distribution

## Context and Problem Statement

substrate is consumed by LLM agents as a child process over STDIO. Operators need a reliable, verifiable way to obtain and trust the binary without requiring a Rust toolchain or build environment. The distribution strategy must address platform variation (macOS, Linux), supply chain integrity, and the macOS Gatekeeper restrictions that prevent unsigned binaries from running without manual override.

## Decision Drivers

- macOS executes unsigned binaries only after a user override in System Settings; LLM agent launchers cannot automate this override, so notarization is mandatory for macOS.
- Supply chain integrity requires that every published artifact can be cryptographically verified against a known signing identity.
- Operators must be able to audit the full set of dependencies included in a release (SBOM).
- Cargo install as a fallback must remain functional for developers who already have a Rust toolchain.
- SemVer must be strictly followed because LLM orchestration frameworks pin tool versions.

## Considered Options

1. Single static binary per platform + macOS codesign/notarize/staple + SPDX SBOM + sigstore cosign + GitHub Releases + Homebrew tap + cargo install fallback.
2. Docker image only.
3. OS-native package managers only (Homebrew, apt, winget).
4. Unsigned binaries on GitHub Releases with a manual installation script.

## Decision Outcome

Chosen option: "single static binary with full signing, notarization, SBOM, and multi-channel distribution", because it minimises operator friction (no runtime required), satisfies macOS Gatekeeper without user intervention, provides cryptographic supply-chain assurance, and keeps cargo install available for the developer audience.

### Consequences

#### Positive

- **Targets**: `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` (static, no glibc dependency); `x86_64-apple-darwin` and `aarch64-apple-darwin` (hardened runtime flag, no dynamic linking of non-system libraries).
- **macOS pipeline**: `cargo build --release` → `codesign --options runtime --entitlements` → `xcrun notarytool submit` (staple on success). Stapling embeds the notarization ticket so the binary verifies offline.
- **SBOM**: `cargo-sbom` generates an SPDX 2.3 JSON document for each release. The SBOM is published as a GitHub Release asset alongside the binary and checksums.
- **Signatures**: `cosign sign-blob` with a keyless OIDC identity (GitHub Actions OIDC) signs each binary. Verification: `cosign verify-blob --certificate-identity <CI_URL> --certificate-oidc-issuer https://token.actions.githubusercontent.com`.
- **Channels**: GitHub Releases is the canonical source. A Homebrew tap (`homebrew-archanjo/substrate`) provides `brew install archanjo/substrate/substrate`. `cargo install substrate-mcp` is supported as a convenience for developers.
- **Versioning**: strict SemVer. BREAKING changes in MCP tool schemas or error codes bump the major version. New tools or optional parameters bump the minor version. Bug fixes bump the patch version. Pre-release labels (`-alpha.N`, `-rc.N`) are used for unstable builds.

#### Negative

- macOS notarization requires an Apple Developer account and access to the `notarytool` credentials; this creates a single-operator dependency for macOS releases.
- musl builds exclude crates that link against glibc-only APIs; this constraint must be audited when adding new dependencies.
- Homebrew tap maintenance is manual; formula updates must be committed after each release.
- cosign keyless verification requires network access to Sigstore's transparency log (Rekor) at verification time.

## Validation

- CI release workflow asserts that `codesign --verify --deep --strict` passes on macOS artifacts before upload.
- CI asserts that `cosign verify-blob` succeeds on each published binary using the expected OIDC identity.
- A smoke test in the release workflow starts the binary with `--help` on each platform target and asserts exit code 0.
- SBOM is linted with `spdx-tools validate` as a CI gate before artifact upload.
- Homebrew CI (`brew audit --strict archanjo/substrate/substrate`) runs on each formula update PR.

## Cross-References

- ADR-0014 — Build reproducibility (reproducible builds feed the signing pipeline)
- ADR-0021 — Minimum supported OS versions (constrains Darwin SDK and musl target selection)
- ADR-0023 — CI/CD pipeline (release workflow orchestration)
