package substrate.no_subprocess

import rego.v1

# ---------------------------------------------------------------------------
# Tests for Rule 1: std::process::Command forbidden in shipped source
# ---------------------------------------------------------------------------

test_shipped_source_without_command_allowed if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-fs-query/src/lib.rs": {
                "content": "use nix::fcntl::OFlag;",
            },
        },
        "cargo_toml_deps": [],
    }
}

test_shipped_source_with_std_command_denied if {
    deny["forbidden std::process::Command in crates/substrate-fs-query/src/lib.rs — per ADR-0044"] with input as {
        "files": {
            "crates/substrate-fs-query/src/lib.rs": {
                "content": "let _ = std::process::Command::new(\"ls\");",
            },
        },
        "cargo_toml_deps": [],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 3: tokio::process::Command forbidden in shipped source
# ---------------------------------------------------------------------------

test_shipped_source_with_tokio_command_denied if {
    deny["forbidden tokio::process::Command in crates/substrate-text/src/search.rs — per ADR-0044"] with input as {
        "files": {
            "crates/substrate-text/src/search.rs": {
                "content": "tokio::process::Command::new(\"grep\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

test_command_in_test_file_allowed if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-fs-query/tests/integration.rs": {
                "content": "std::process::Command::new(\"ls\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 4: forbidden crates in Cargo.toml dependencies
# ---------------------------------------------------------------------------

test_forbidden_crate_in_deps_denied if {
    deny["forbidden crate 'duct' in Cargo.toml dependencies — per ADR-0044"] with input as {
        "files": {},
        "cargo_toml_deps": ["tokio", "duct"],
    }
}

test_clean_deps_allowed if {
    count(deny) == 0 with input as {
        "files": {},
        "cargo_toml_deps": ["tokio", "serde", "anyhow"],
    }
}

# ---------------------------------------------------------------------------
# Tests for Rule 5: build.rs Command requires justification comment
# ---------------------------------------------------------------------------

test_build_rs_command_without_justification_denied if {
    deny["build.rs uses std::process::Command without a no-subprocess-justification comment in crates/substrate-config/build.rs — per ADR-0044"] with input as {
        "files": {
            "crates/substrate-config/build.rs": {
                "content": "std::process::Command::new(\"pkg-config\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

test_build_rs_command_with_justification_allowed if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-config/build.rs": {
                "content": "// no-subprocess-justification: queries platform header version; no pure-Rust alternative exists\nstd::process::Command::new(\"pkg-config\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

# ---------------------------------------------------------------------------
# Tests for ADR-0052 amendment: substrate-subprocess crate whitelist
# ---------------------------------------------------------------------------

# PASS — tokio::process::Command in substrate-subprocess shipped source is allowed.
# This is the single permitted exception to Rule 3 per ADR-0052.
test_substrate_subprocess_crate_whitelisted_for_tokio_process if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-subprocess/src/spawn.rs": {
                "content": "tokio::process::Command::new(\"echo\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

# FAIL — std::process::Command in substrate-subprocess is still denied.
# The ADR-0052 exception is scoped to tokio::process::Command only.
test_substrate_subprocess_std_command_still_denied if {
    deny["forbidden std::process::Command in crates/substrate-subprocess/src/spawn.rs — per ADR-0044"] with input as {
        "files": {
            "crates/substrate-subprocess/src/spawn.rs": {
                "content": "std::process::Command::new(\"echo\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

# FAIL — tokio::process::Command in a non-subprocess crate remains denied.
# The whitelist is strictly bounded to crates/substrate-subprocess/.
test_tokio_command_in_other_crate_still_denied if {
    deny["forbidden tokio::process::Command in crates/substrate-process/src/spawn.rs — per ADR-0044"] with input as {
        "files": {
            "crates/substrate-process/src/spawn.rs": {
                "content": "tokio::process::Command::new(\"ls\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

# ---------------------------------------------------------------------------
# Tests for ADR-0063/0068 amendment: substrate-launch supervisor self-fork (Rule 3b)
# ---------------------------------------------------------------------------

# PASS — tokio::process::Command in substrate-launch WITH the supervise-fork
# justification comment is the single permitted launch exception.
test_substrate_launch_supervise_fork_allowed if {
    count(deny) == 0 with input as {
        "files": {
            "crates/substrate-launch/src/supervise.rs": {
                "content": "// supervise-fork-justification: re-execs the same binary as a detached --supervise supervisor per ADR-0068\ntokio::process::Command::new(\"substrate\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

# FAIL — tokio::process::Command in substrate-launch WITHOUT the justification is denied.
test_substrate_launch_tokio_without_justification_denied if {
    deny["forbidden tokio::process::Command in crates/substrate-launch/src/run.rs — per ADR-0044"] with input as {
        "files": {
            "crates/substrate-launch/src/run.rs": {
                "content": "tokio::process::Command::new(\"node\")",
            },
        },
        "cargo_toml_deps": [],
    }
}

# FAIL — std::process::Command in substrate-launch stays globally forbidden even with the comment.
test_substrate_launch_std_command_still_denied if {
    deny["forbidden std::process::Command in crates/substrate-launch/src/supervise.rs — per ADR-0044"] with input as {
        "files": {
            "crates/substrate-launch/src/supervise.rs": {
                "content": "// supervise-fork-justification: x\nstd::process::Command::new(\"substrate\")",
            },
        },
        "cargo_toml_deps": [],
    }
}
