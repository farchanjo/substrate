//! Figment-based configuration loader per ADR-0006.
//!
//! Sources applied in priority order (later wins):
//!   1. Built-in defaults via `RuntimeConfig::default()`.
//!   2. `$XDG_CONFIG_HOME/substrate/config.toml` (or `~/.config/substrate/config.toml`).
//!   3. `./substrate.toml` (project-local override; intended for dev).
//!   4. Environment variables prefixed `SUBSTRATE_` with `__` as the nesting separator
//!      (e.g. `SUBSTRATE_LOGGING__LEVEL=debug` sets `logging.level`).

use std::path::PathBuf;

use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};

use crate::model::RuntimeConfig;

// ---- Error type --------------------------------------------------------------

/// Error type for configuration loading failures.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required config file exists but could not be parsed.
    #[error("config parse error: {0}")]
    Parse(#[from] figment::Error),

    /// Post-parse validation rejected the assembled configuration.
    #[error("config validation error: {message}")]
    Validation {
        /// Human-readable description of the violated constraint.
        message: String,
    },
}

// ---- Public surface ----------------------------------------------------------

/// Loads `RuntimeConfig` from the default source chain.
///
/// Reads from (in priority order, highest first):
///   - `SUBSTRATE_*` environment variables
///   - `./substrate.toml` (if present)
///   - `$XDG_CONFIG_HOME/substrate/config.toml` (or `~/.config/substrate/config.toml`, if present)
///   - Built-in defaults
///
/// # Errors
///
/// Returns [`ConfigError::Parse`] when a TOML file exists but is malformed or
/// contains unknown keys.  Returns [`ConfigError::Validation`] when post-parse
/// invariants are violated.
#[expect(
    clippy::result_large_err,
    reason = "figment::Error is large; boxing would add indirection for no functional benefit in this startup-path function"
)]
pub fn load() -> Result<RuntimeConfig, ConfigError> {
    load_with(default_paths())
}

/// Loads `RuntimeConfig` with an explicit ordered list of TOML file paths.
///
/// Paths are merged in the order given (last wins within the list).  Absent
/// files are silently skipped.  The env-var layer always wins over all file
/// layers.
///
/// # Errors
///
/// See [`load`].
#[expect(
    clippy::result_large_err,
    reason = "figment::Error is large; boxing would add indirection for no functional benefit in this startup-path function"
)]
pub fn load_with(paths: Vec<PathBuf>) -> Result<RuntimeConfig, ConfigError> {
    let mut fig = Figment::from(Serialized::defaults(RuntimeConfig::default()));

    for p in paths {
        if p.exists() {
            tracing::debug!(path = %p.display(), "merging config file");
            fig = fig.merge(Toml::file(p));
        }
    }

    // `SUBSTRATE_LOGGING__LEVEL=debug` maps to `logging.level = "debug"`.
    fig = fig.merge(Env::prefixed("SUBSTRATE_").split("__"));

    let cfg: RuntimeConfig = fig.extract()?;

    validate(&cfg)?;
    Ok(cfg)
}

// ---- Internals ---------------------------------------------------------------

/// Returns the default config-file search path (operator file + project-local override).
fn default_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();

    // XDG_CONFIG_HOME / ~/.config / %APPDATA%
    if let Some(p) = operator_config_path() {
        v.push(p);
    }

    // Project-local override (dev convenience; not loaded in read-only contexts).
    v.push(PathBuf::from("substrate.toml"));

    v
}

/// Resolves the operator config path respecting the XDG Base Directory specification.
#[expect(
    clippy::needless_return,
    reason = "cfg-gated arms require explicit return to avoid type errors when multiple cfg blocks are present"
)]
fn operator_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("substrate").join("config.toml"));
    }

    #[cfg(unix)]
    {
        return std::env::var("HOME").ok().map(|h| {
            PathBuf::from(h)
                .join(".config")
                .join("substrate")
                .join("config.toml")
        });
    }

    #[cfg(windows)]
    {
        return std::env::var("APPDATA")
            .ok()
            .map(|a| PathBuf::from(a).join("substrate").join("config.toml"));
    }

    #[cfg(not(any(unix, windows)))]
    None
}

/// Post-parse validation of assembled configuration.
#[expect(
    clippy::result_large_err,
    reason = "figment::Error is large; boxing would add indirection for no functional benefit in this startup-path function"
)]
fn validate(cfg: &RuntimeConfig) -> Result<(), ConfigError> {
    // shutdown_drain_secs must be in [1, 120] per CUE schema constraint.
    if cfg.shutdown_drain_secs == 0 || cfg.shutdown_drain_secs > 120 {
        return Err(ConfigError::Validation {
            message: format!(
                "shutdown_drain_secs must be in [1, 120], got {}",
                cfg.shutdown_drain_secs
            ),
        });
    }

    // global timeout must be >= 1.
    if cfg.timeouts.global_default_seconds == 0 {
        return Err(ConfigError::Validation {
            message: "timeouts.global_default_seconds must be >= 1".into(),
        });
    }

    // Per-tool timeouts must all be >= 1.
    for (tool, &secs) in &cfg.timeouts.per_tool {
        if secs == 0 {
            return Err(ConfigError::Validation {
                message: format!("timeouts.per_tool[{tool}] must be >= 1"),
            });
        }
    }

    // default_page_size must be <= max_page_size.
    if cfg.protocol.default_page_size > cfg.protocol.max_page_size {
        return Err(ConfigError::Validation {
            message: format!(
                "protocol.default_page_size ({}) must not exceed protocol.max_page_size ({})",
                cfg.protocol.default_page_size, cfg.protocol.max_page_size
            ),
        });
    }

    // max_in_memory_buffer_bytes hard ceiling: 32 MiB per ADR-0016.
    // The const is declared at the top of the validate fn body to avoid the
    // `clippy::items_after_statements` lint (items must precede all statements).
    #[expect(
        clippy::items_after_statements,
        reason = "const belongs near the guard that uses it; hoisting would obscure intent"
    )]
    const MAX_BUFFER_HARD_CEILING: u64 = 32 * 1_024 * 1_024;
    if cfg.protocol.max_in_memory_buffer_bytes > MAX_BUFFER_HARD_CEILING {
        return Err(ConfigError::Validation {
            message: format!(
                "protocol.max_in_memory_buffer_bytes ({}) exceeds hard ceiling of {} bytes (32 MiB)",
                cfg.protocol.max_in_memory_buffer_bytes, MAX_BUFFER_HARD_CEILING,
            ),
        });
    }

    // When logging target is "file", file_path must be set and absolute.
    if matches!(cfg.logging.target, crate::model::LogTarget::File) {
        match &cfg.logging.file_path {
            None => {
                return Err(ConfigError::Validation {
                    message: "logging.file_path is required when logging.target = \"file\"".into(),
                });
            },
            Some(p) if !p.is_absolute() => {
                return Err(ConfigError::Validation {
                    message: format!(
                        "logging.file_path must be an absolute path, got \"{}\"",
                        p.display()
                    ),
                });
            },
            Some(_) => {},
        }
    }

    // subprocess.tmp_root, when set, must be under one of policy.roots.
    // This is a structural check only; PathJail enforcement happens at composition.
    // We do not pre-resolve the fallback (policy.roots[0]) here because that
    // resolution requires canonicalization which may block in async context.
    if let Some(ref subprocess_cfg) = cfg.subprocess
        && let Some(ref tmp_root) = subprocess_cfg.tmp_root
    {
        if !tmp_root.is_absolute() {
            return Err(ConfigError::Validation {
                message: format!(
                    "subprocess.tmp_root must be an absolute path, got \"{}\"",
                    tmp_root.display()
                ),
            });
        }
        let under_any_root = cfg
            .policy
            .roots
            .iter()
            .any(|root| tmp_root.starts_with(root));
        if !under_any_root && !cfg.policy.roots.is_empty() {
            return Err(ConfigError::Validation {
                message: format!(
                    "subprocess.tmp_root \"{}\" must be under one of policy.roots: [{}]",
                    tmp_root.display(),
                    cfg.policy
                        .roots
                        .iter()
                        .map(|r| r.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    }

    Ok(())
}

// ---- Tests -------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::result_large_err,
    reason = "test module — panics on assertion failure are the intended behavior; result_large_err suppressed for test helper mirroring production API"
)]
mod tests {
    use super::*;
    use std::io::Write as _;

    // Helper: write a TOML fragment to a temp file and load it.
    fn load_toml(content: &str) -> Result<RuntimeConfig, ConfigError> {
        let mut f = tempfile::NamedTempFile::new().expect("tempfile");
        f.write_all(content.as_bytes()).expect("write");
        load_with(vec![f.path().to_owned()])
    }

    #[test]
    fn empty_toml_loads_with_defaults() {
        let cfg = load_toml("").expect("empty TOML should be valid");
        assert_eq!(cfg.shutdown_drain_secs, 5);
        assert!(cfg.security.refuse_degraded_jail);
        assert_eq!(cfg.protocol.max_in_flight_requests, 32);
        assert_eq!(cfg.protocol.max_page_size, 500);
        assert_eq!(cfg.timeouts.global_default_seconds, 30);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let err = load_toml("nonexistent_key = true").expect_err("must reject unknown key");
        assert!(
            matches!(err, ConfigError::Parse(_)),
            "expected Parse, got: {err:?}"
        );
    }

    #[test]
    fn shutdown_drain_defaults_to_5() {
        let cfg = load_toml("").expect("valid");
        assert_eq!(cfg.shutdown_drain_secs, 5);
    }

    #[test]
    fn refuse_degraded_jail_defaults_to_true() {
        let cfg = load_toml("").expect("valid");
        assert!(cfg.security.refuse_degraded_jail);
    }

    #[test]
    fn env_var_overrides_log_level() {
        // SUBSTRATE_LOGGING__LEVEL=debug should set logging.level to Debug.
        //
        // The Figment chain must be built AFTER the env var is set so that the
        // Env provider snapshots the current environment.  Building the chain
        // before setting the var and storing it causes a stale snapshot in
        // figment 0.10 — the env is captured at merge time, not at extract time.
        //
        // SAFETY: tests run under cargo-nextest with --test-threads=1 (see
        // .cargo/nextest.toml); no other thread observes env mutations during
        // this window. Edition 2024 made set_var/remove_var unsafe.
        #[allow(unsafe_code, reason = "test-only serial env mutation; SAFETY above")]
        unsafe {
            std::env::set_var("SUBSTRATE_LOGGING__LEVEL", "debug");
        }
        let result = load_with(vec![]);
        #[allow(unsafe_code, reason = "test-only serial env mutation; SAFETY above")]
        unsafe {
            std::env::remove_var("SUBSTRATE_LOGGING__LEVEL");
        }

        let cfg = result.expect("env override must succeed");
        assert_eq!(cfg.logging.level, crate::model::LogLevel::Debug);
    }

    #[test]
    fn shutdown_drain_out_of_range_fails_validation() {
        let err =
            load_toml("shutdown_drain_secs = 0").expect_err("zero drain secs must fail validation");
        assert!(
            matches!(err, ConfigError::Validation { .. }),
            "expected Validation, got: {err:?}"
        );
    }

    #[test]
    fn shutdown_drain_at_maximum_is_valid() {
        let cfg = load_toml("shutdown_drain_secs = 120").expect("120 is the max allowed");
        assert_eq!(cfg.shutdown_drain_secs, 120);
    }

    #[test]
    fn buffer_ceiling_exceeded_fails_validation() {
        // 33 MiB > hard ceiling of 32 MiB.
        let toml = format!(
            "[protocol]\nmax_in_memory_buffer_bytes = {}",
            33 * 1_024 * 1_024_u64
        );
        let err = load_toml(&toml).expect_err("must reject buffer > 32 MiB");
        assert!(matches!(err, ConfigError::Validation { .. }));
    }

    #[test]
    fn log_target_file_without_path_fails_validation() {
        let err = load_toml("[logging]\ntarget = \"file\"")
            .expect_err("file target without path must fail");
        assert!(matches!(err, ConfigError::Validation { .. }));
    }

    #[test]
    fn index_section_parsed() {
        let cfg = load_toml("[index]\nenabled = true\nttl_secs = 120").expect("valid index");
        let idx = cfg.index.expect("index section present");
        assert!(idx.enabled);
        assert_eq!(idx.ttl_secs, 120);
    }
}
