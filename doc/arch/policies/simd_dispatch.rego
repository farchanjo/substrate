# package substrate.simd_dispatch
#
# Enforces the SIMD dispatch discipline for the substrate crate workspace.
# Per ADR-0043: unsafe SIMD intrinsic calls MUST be confined to simd_impl
# modules that are properly scoped to target-architecture cfg guards and that
# consult the process-global SimdTier cache (OnceLock<Capabilities>) rather
# than re-probing CPUID.
#
# Input shape (provided by the CI conftest adapter):
#   {
#     "files": {
#       "<path>": {
#         "content": "<file content as string>"
#       }
#     }
#   }
#
# Where files is a map of relative crates/ paths to textual file content.
# The policy performs textual analysis; it cannot inspect the Rust AST.
# AST-level guarantees (e.g. that cfg guards are well-formed) require
# rustc --emit=mir or cargo-expand; those checks are out of scope here and
# enforced by the CI lint matrix (cargo check --target aarch64-apple-darwin
# must not compile x86 intrinsic paths, per ADR-0043 validation).
#
# Test vectors (inline):
#
#   PASS — SIMD intrinsic in a correctly-named simd_impl module path
#   input = {"files":{"crates/substrate-text/src/simd_impl/search.rs":{"content":"use std::arch::x86_64::*;"}}}
#
#   FAIL — SIMD intrinsic outside simd_impl path
#   input = {"files":{"crates/substrate-text/src/search.rs":{"content":"use std::arch::x86_64::*;"}}}
#   expected deny: "std::arch::x86_64 intrinsic outside simd_impl module in crates/substrate-text/src/search.rs — per ADR-0043"
#
#   FAIL — target_cpu=native in release RUSTFLAGS
#   input = {"files":{"crates/substrate-mcp-server/.cargo/config.toml":{"content":"[profile.release]\nrustflags = [\"-C\", \"target-cpu=native\"]"}}}
#   expected deny: "target-cpu=native found in release profile in crates/substrate-mcp-server/.cargo/config.toml — per ADR-0043 (forbidden for distributed builds)"

package substrate.simd_dispatch

import rego.v1

# ---------------------------------------------------------------------------
# Classification helpers
# ---------------------------------------------------------------------------

# True when the file path is inside a simd_impl or simd directory,
# which is the only location where SIMD unsafe wrappers may reside.
# Per ADR-0043: unsafe blocks must be localized to simd_impl modules inside
# each adapter crate.
_is_simd_impl_path(path) if contains(path, "/simd_impl/")

_is_simd_impl_path(path) if contains(path, "/simd/")

# True when the file is a Rust source file (not a config or TOML).
_is_rust_source(path) if endswith(path, ".rs")

# True when the file is a Cargo config or workspace TOML.
_is_cargo_config(path) if endswith(path, ".cargo/config.toml")

_is_cargo_config(path) if endswith(path, ".cargo/config")

# ---------------------------------------------------------------------------
# Rule 1: std::arch::x86_64 intrinsics MUST reside in simd_impl paths
# Per ADR-0043: x86-64 intrinsic imports outside simd_impl modules indicate
# that SIMD code has escaped the narrow unsafe scope.
# ---------------------------------------------------------------------------

deny contains msg if {
    some path
    input.files[path].content
    _is_rust_source(path)
    not _is_simd_impl_path(path)
    contains(input.files[path].content, "std::arch::x86_64")
    msg := sprintf(
        "std::arch::x86_64 intrinsic outside simd_impl module in %s — per ADR-0043",
        [path],
    )
}

# ---------------------------------------------------------------------------
# Rule 2: std::arch::aarch64 intrinsics MUST reside in simd_impl paths
# Per ADR-0043: aarch64 NEON intrinsic imports outside simd_impl modules
# violate the narrow unsafe scope discipline.
# ---------------------------------------------------------------------------

deny contains msg if {
    some path
    input.files[path].content
    _is_rust_source(path)
    not _is_simd_impl_path(path)
    contains(input.files[path].content, "std::arch::aarch64")
    msg := sprintf(
        "std::arch::aarch64 intrinsic outside simd_impl module in %s — per ADR-0043",
        [path],
    )
}

# ---------------------------------------------------------------------------
# Rule 3: simd_impl files MUST contain an architecture cfg guard
# Per ADR-0043: each simd_impl file that uses arch-specific intrinsics must
# be wrapped in a cfg(target_arch = "...") guard. We enforce the presence of
# a cfg(target_arch annotation as a textual invariant.
# Note: this is a best-effort textual check; the CI cargo check --target
# matrix is the definitive enforcement per ADR-0043 validation section.
# ---------------------------------------------------------------------------

deny contains msg if {
    some path
    input.files[path].content
    _is_rust_source(path)
    _is_simd_impl_path(path)
    contains(input.files[path].content, "std::arch::")
    not contains(input.files[path].content, "cfg(target_arch")
    msg := sprintf(
        "simd_impl file uses std::arch intrinsics without a cfg(target_arch guard in %s — per ADR-0043",
        [path],
    )
}

# ---------------------------------------------------------------------------
# Rule 4: target-cpu=native MUST NOT appear in release profile RUSTFLAGS
# Per ADR-0043: target-cpu=native is forbidden for distributed release builds.
# It may appear only in the dev profile. The policy flags any release profile
# occurrence regardless of profile name; CI reviewers validate exceptions.
# ---------------------------------------------------------------------------

deny contains msg if {
    some path
    input.files[path].content
    _is_cargo_config(path)
    contains(input.files[path].content, "target-cpu=native")
    contains(input.files[path].content, "[profile.release]")
    msg := sprintf(
        "target-cpu=native found in release profile in %s — per ADR-0043 (forbidden for distributed builds)",
        [path],
    )
}

# ---------------------------------------------------------------------------
# Rule 5: SIMD crate probing via subprocess is forbidden
# Per ADR-0043 + ADR-0044: CPU feature detection MUST use
# std::is_x86_feature_detected! or is_aarch64_feature_detected! only.
# If any simd_impl file references /proc/cpuinfo parsing via Command,
# that is a double violation (subprocess + simd policy).
# ---------------------------------------------------------------------------

deny contains msg if {
    some path
    input.files[path].content
    _is_rust_source(path)
    _is_simd_impl_path(path)
    contains(input.files[path].content, "proc/cpuinfo")
    msg := sprintf(
        "SIMD detection via /proc/cpuinfo parsing in %s — use is_x86_feature_detected! instead, per ADR-0043",
        [path],
    )
}

# ---------------------------------------------------------------------------
# allow — true only when deny is empty
# ---------------------------------------------------------------------------

default allow := false

allow if count(deny) == 0
