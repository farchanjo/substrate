//! substrate-config — figment-based TOML + env runtime config loader per ADR-0006.
//!
//! Sources applied in priority order (later wins):
//!   1. Built-in `Default` values.
//!   2. Operator TOML file (`$XDG_CONFIG_HOME/substrate/config.toml`
//!      or `~/.config/substrate/config.toml`).
//!   3. Project-local override (`./substrate.toml`; dev only).
//!   4. Environment variables prefixed `SUBSTRATE_` with `__` as the nesting
//!      separator (e.g. `SUBSTRATE_LOGGING__LEVEL=debug`).
//!
//! All config structs use `#[serde(deny_unknown_fields)]` so that operator TOML
//! typos are detected at load time rather than silently ignored.

#![cfg_attr(not(test), forbid(unsafe_code))]
#![warn(missing_docs)]

mod loader;
mod model;

pub use loader::{ConfigError, load, load_with};
pub use model::{
    CapabilitiesSection, IndexConfig, LogLevel, LogTarget, LogWriteErrorPolicy, LoggingConfig,
    PolicyConfig, ProtocolConfig, RuntimeConfig, SecurityRuntime, SemaphoreCaps, SimdConfig,
    SubprocessConfig, Timeouts,
};
