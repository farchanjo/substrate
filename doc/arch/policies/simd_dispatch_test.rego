package substrate.simd_dispatch

import rego.v1

# ---------------------------------------------------------------------------
# Tests for Rule 1: std::arch::x86_64 intrinsics MUST reside in simd_impl paths
# ---------------------------------------------------------------------------

test_x86_intrinsic_in_simd_impl_allowed if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-text/src/simd_impl/search.rs": {
                "content": "use std::arch::x86_64::*;\n#[cfg(target_arch = \"x86_64\")]\nfn search() {}",
            },
        },
    }
}

test_x86_intrinsic_outside_simd_impl_denied if {
    deny["std::arch::x86_64 intrinsic outside simd_impl module in crates/substrate-text/src/search.rs — per ADR-0043"] with input as {
        "files": {
            "crates/substrate-text/src/search.rs": {
                "content": "use std::arch::x86_64::*;",
            },
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 2: std::arch::aarch64 intrinsics MUST reside in simd_impl paths
# ---------------------------------------------------------------------------

test_aarch64_intrinsic_outside_simd_impl_denied if {
    deny["std::arch::aarch64 intrinsic outside simd_impl module in crates/substrate-text/src/lib.rs — per ADR-0043"] with input as {
        "files": {
            "crates/substrate-text/src/lib.rs": {
                "content": "use std::arch::aarch64::*;",
            },
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 3: simd_impl files MUST contain a cfg(target_arch guard
# ---------------------------------------------------------------------------

test_simd_impl_without_cfg_guard_denied if {
    deny["simd_impl file uses std::arch intrinsics without a cfg(target_arch guard in crates/substrate-text/src/simd_impl/search.rs — per ADR-0043"] with input as {
        "files": {
            "crates/substrate-text/src/simd_impl/search.rs": {
                "content": "use std::arch::x86_64::*;",
            },
        },
    }
}

test_simd_impl_with_cfg_guard_allowed if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-text/src/simd_impl/search.rs": {
                "content": "#[cfg(target_arch = \"x86_64\")]\nuse std::arch::x86_64::*;",
            },
        },
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 4: target-cpu=native MUST NOT appear in release RUSTFLAGS
# ---------------------------------------------------------------------------

test_target_cpu_native_in_release_denied if {
    deny["target-cpu=native found in release profile in crates/substrate-mcp-server/.cargo/config.toml — per ADR-0043 (forbidden for distributed builds)"] with input as {
        "files": {
            "crates/substrate-mcp-server/.cargo/config.toml": {
                "content": "[profile.release]\nrustflags = [\"-C\", \"target-cpu=native\"]",
            },
        },
    }
}

test_clean_cargo_config_allowed if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-mcp-server/.cargo/config.toml": {
                "content": "[profile.release]\nopt-level = 3",
            },
        },
    }
}
