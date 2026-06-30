//! Launch Profile value objects parsed from `.substrate.toml`.
//!
//! Mirrors `#LaunchProfile`, `#LaunchService`, `#LaunchOperatorConfig`, and
//! `#LaunchChannelBounds` in `docs/arch/schemas/launch.cue`. A `LaunchProfile`
//! is immutable once loaded and trusted (ADR-0063). `RestartPolicy` and
//! `HealthProbe` are reused verbatim from the subprocess BC (ADR-0056).
//!
//! References: ADR-0063 (Profile/Service/Stack), ADR-0064 (command argv form),
//! ADR-0065 (dependency DAG), ADR-0067 (channel bounds).

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::launch::errors::LaunchError;
use crate::launch::state::DisconnectPolicy;
use crate::subprocess::supervisor::{HealthProbe, RestartPolicy};

/// Operator-supplied alias for a Service within a Profile.
///
/// Mirrors `#ServiceName` (`^[a-z0-9-]{1,64}$`). Kept as `String` for MVP so it
/// can serve directly as a `BTreeMap` key, giving deterministic topological
/// ordering. Validation of the character set is enforced by [`LaunchProfile::validate`].
pub type ServiceName = String;

/// Selects whether a Service restarts when one of its dependencies is restarted.
///
/// Mirrors `#LaunchService.on_dependency_restart` (`"restart" | "ignore"`,
/// default `restart`) per ADR-0065.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DependencyRestartMode {
    /// Restart this Service when a dependency is restarted. Default.
    #[default]
    Restart,
    /// Leave this Service running when a dependency is restarted.
    Ignore,
}

/// Output multiplexing mode for a Service's stdout/stderr channels.
///
/// Mirrors `#LaunchService.streams` (`"multiplexed" | "separate"`, default
/// `multiplexed`) per ADR-0067.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StreamMux {
    /// A single tagged channel multiplexes stdout and stderr. Default.
    #[default]
    Multiplexed,
    /// Separate per-Service output channels.
    Separate,
}

/// The executable invocation for a Service.
///
/// Mirrors `#LaunchService.command`, which is `[string, ...string]` (argv form).
/// A bare string is rejected per ADR-0064 to remove the argument-injection
/// surface. Deserialization is permissive (it accepts either shape) so that the
/// rejection surfaces as a structured [`LaunchError::InvalidProfile`] from
/// [`LaunchProfile::validate`] rather than as an opaque deserialize failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommandSpec {
    /// The valid argv form: `command[0]` is the binary, the rest are arguments.
    Argv(Vec<String>),
    /// The rejected bare-string form, retained only so [`LaunchProfile::validate`]
    /// can report it as `InvalidProfile` instead of failing at deserialize time.
    Shell(String),
}

impl CommandSpec {
    /// Returns the argv slice when this is the valid array form.
    ///
    /// # Errors
    ///
    /// Returns [`LaunchError::InvalidProfile`] when the command was declared as a
    /// bare string (`Shell` variant) or as an empty array.
    pub fn argv(&self) -> Result<&[String], LaunchError> {
        match self {
            Self::Argv(v) if !v.is_empty() => Ok(v),
            Self::Argv(_) => Err(LaunchError::InvalidProfile {
                msg: "command array must be non-empty".to_owned(),
            }),
            Self::Shell(_) => Err(LaunchError::InvalidProfile {
                msg: "command must be an array of strings (argv form); a bare string is rejected"
                    .to_owned(),
            }),
        }
    }
}

/// One entry in a Profile catalog; materializes to a single supervised child.
///
/// Mirrors `#LaunchService` in `launch.cue` (ADR-0063 / ADR-0065).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchService {
    /// The executable plus arguments in argv form (`command[0]` is the binary).
    pub command: CommandSpec,

    /// Extra arguments appended after `command[1..]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    /// Explicit `key=value` overrides for the child environment.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,

    /// Absolute working directory for the child, validated by `PathJail`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Services that must reach `Ready` before this Service starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<ServiceName>,

    /// When `false`, a missing or failed dependency is a warning, not a blocker.
    /// Defaults to `true` per ADR-0065.
    #[serde(default = "default_true")]
    pub required: bool,

    /// Supervisor re-spawn policy on this Service's own exit (reused from ADR-0056).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<RestartPolicy>,

    /// Readiness probe gating the `Starting -> Ready` transition (reused from ADR-0056).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_probe: Option<HealthProbe>,

    /// Whether this Service restarts when a dependency restarts. Default `restart`.
    #[serde(default)]
    pub on_dependency_restart: DependencyRestartMode,

    /// Regex applied to output to distil semantic-plane events (ADR-0066).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub error_patterns: Vec<String>,

    /// Per-Service redaction patterns applied at the source (ADR-0066).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact: Vec<String>,

    /// Output multiplexing mode. Default `multiplexed`.
    #[serde(default)]
    pub streams: StreamMux,
}

/// The catalog of Services plus Stack-level defaults parsed from `.substrate.toml`.
///
/// Mirrors `#LaunchProfile` in `launch.cue` (ADR-0063). Immutable once loaded
/// and trusted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchProfile {
    /// Profile schema version; reserved for forward migration. Must be `>= 1`.
    #[serde(default = "default_version")]
    pub version: u32,

    /// Stack-level default disconnect policy (ADR-0068).
    #[serde(default)]
    pub on_client_disconnect: DisconnectPolicy,

    /// Seconds a detached Stack may run with no client before auto-down.
    /// Default 3600; range `0..=86400` (0 disables detached survival).
    #[serde(default = "default_orphan_ttl")]
    pub orphan_ttl_secs: u32,

    /// The Service catalog keyed by Service name.
    pub services: BTreeMap<ServiceName, LaunchService>,
}

/// Maximum value permitted for [`LaunchProfile::orphan_ttl_secs`] (24 hours).
const ORPHAN_TTL_MAX_SECS: u32 = 86_400;
/// Maximum length of a `#ServiceName`.
const SERVICE_NAME_MAX_LEN: usize = 64;

const fn default_true() -> bool {
    true
}

const fn default_version() -> u32 {
    1
}

const fn default_orphan_ttl() -> u32 {
    3600
}

/// Returns `true` when `name` matches the `#ServiceName` pattern `^[a-z0-9-]{1,64}$`.
fn is_valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= SERVICE_NAME_MAX_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl LaunchProfile {
    /// Validates the Profile structurally without any I/O.
    ///
    /// Checks: `version >= 1`; `orphan_ttl_secs <= 86400`; every Service name
    /// matches `^[a-z0-9-]{1,64}$`; every `command` is a non-empty argv array
    /// (a bare string is rejected per ADR-0064); every `depends_on` target
    /// references an existing Service.
    ///
    /// # Errors
    ///
    /// Returns [`LaunchError::InvalidProfile`] on any structural violation.
    pub fn validate(&self) -> Result<(), LaunchError> {
        if self.version < 1 {
            return Err(LaunchError::InvalidProfile {
                msg: format!("version must be >= 1; got {}", self.version),
            });
        }
        if self.orphan_ttl_secs > ORPHAN_TTL_MAX_SECS {
            return Err(LaunchError::InvalidProfile {
                msg: format!(
                    "orphan_ttl_secs must be <= {ORPHAN_TTL_MAX_SECS}; got {}",
                    self.orphan_ttl_secs
                ),
            });
        }
        for (name, service) in &self.services {
            if !is_valid_service_name(name) {
                return Err(LaunchError::InvalidProfile {
                    msg: format!("invalid service name '{name}'; must match ^[a-z0-9-]{{1,64}}$"),
                });
            }
            // Rejects bare-string commands and empty argv arrays.
            service.command.argv()?;
            for dep in &service.depends_on {
                if !self.services.contains_key(dep) {
                    return Err(LaunchError::InvalidProfile {
                        msg: format!("service '{name}' depends_on unknown service '{dep}'"),
                    });
                }
            }
        }
        Ok(())
    }

    /// Returns the Services in topological (dependency-first) order via Kahn's algorithm.
    ///
    /// A Service appears only after all the Services it `depends_on`. The order
    /// among independent Services is deterministic (ascending name) because the
    /// catalog is a `BTreeMap`.
    ///
    /// # Errors
    ///
    /// Returns [`LaunchError::CycleDetected`] when the `depends_on` edges do not
    /// form a DAG. The `nodes` field lists the Services still unresolved when the
    /// queue drained (the members of one or more cycles).
    pub fn topological_order(&self) -> Result<Vec<ServiceName>, LaunchError> {
        // In-degree of a Service = number of dependencies it is waiting on
        // (counting only edges to Services that exist in the catalog).
        let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
        for (name, service) in &self.services {
            let deps_present = service
                .depends_on
                .iter()
                .filter(|d| self.services.contains_key(d.as_str()))
                .count();
            in_degree.insert(name.as_str(), deps_present);
        }

        // Seed the queue with Services that depend on nothing, ascending by name.
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|&(_, &d)| d == 0)
            .map(|(&n, _)| n)
            .collect();

        let mut order: Vec<ServiceName> = Vec::with_capacity(self.services.len());
        while let Some(node) = queue.pop_front() {
            order.push(node.to_owned());
            // Releasing `node` decrements the in-degree of every dependent.
            for (dependent, service) in &self.services {
                if !service.depends_on.iter().any(|d| d == node) {
                    continue;
                }
                if let Some(d) = in_degree.get_mut(dependent.as_str()) {
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(dependent.as_str());
                    }
                }
            }
        }

        if order.len() == self.services.len() {
            Ok(order)
        } else {
            // The unresolved Services are exactly those still carrying in-degree > 0.
            let mut nodes: Vec<String> = self
                .services
                .keys()
                .filter(|n| !order.contains(*n))
                .cloned()
                .collect();
            nodes.sort();
            Err(LaunchError::CycleDetected { nodes })
        }
    }
}

/// User-scope launch operator policy loaded from `~/.config/substrate/launch.toml`.
///
/// Mirrors `#LaunchOperatorConfig` in `launch.cue` (ADR-0064). It lives outside
/// any repository so a cloned Profile cannot authorize its own blessing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchOperatorConfig {
    /// Absolute canonical path prefixes for which `launch.up` may auto-bless.
    /// Empty (default) means every new Profile needs an explicit `launch.trust`.
    #[serde(default)]
    pub auto_bless_paths: Vec<String>,
}

/// Configurable bounds for the lock-free messaging fabric.
///
/// Mirrors `#LaunchChannelBounds` in `launch.cue` (ADR-0067 / ADR-0066 / ADR-0065).
/// All fields default via [`Default`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchChannelBounds {
    /// Per-Service stdout/stderr reader channel capacity. Default 1024.
    #[serde(default = "default_stdout_capacity")]
    pub stdout_mpsc_capacity: u32,
    /// Per-Stack broadcast bus capacity. Default 256.
    #[serde(default = "default_broadcast_capacity")]
    pub event_broadcast_capacity: u32,
    /// Semantic-event emission cap per Service per second. Default 5.
    #[serde(default = "default_notify_rate")]
    pub notify_rate_per_sec: u32,
    /// Reconciler/cascade restart cap per Service per minute. Default 60.
    #[serde(default = "default_restart_rate")]
    pub orchestrated_restart_per_min: u32,
}

const fn default_stdout_capacity() -> u32 {
    1024
}

const fn default_broadcast_capacity() -> u32 {
    256
}

const fn default_notify_rate() -> u32 {
    5
}

const fn default_restart_rate() -> u32 {
    60
}

impl Default for LaunchChannelBounds {
    fn default() -> Self {
        Self {
            stdout_mpsc_capacity: default_stdout_capacity(),
            event_broadcast_capacity: default_broadcast_capacity(),
            notify_rate_per_sec: default_notify_rate(),
            orchestrated_restart_per_min: default_restart_rate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(deps: &[&str]) -> LaunchService {
        LaunchService {
            command: CommandSpec::Argv(vec!["bin".to_owned()]),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            depends_on: deps.iter().map(|s| (*s).to_owned()).collect(),
            required: true,
            restart_policy: None,
            health_probe: None,
            on_dependency_restart: DependencyRestartMode::Restart,
            error_patterns: Vec::new(),
            redact: Vec::new(),
            streams: StreamMux::Multiplexed,
        }
    }

    fn profile(services: BTreeMap<ServiceName, LaunchService>) -> LaunchProfile {
        LaunchProfile {
            version: 1,
            on_client_disconnect: DisconnectPolicy::Shutdown,
            orphan_ttl_secs: 3600,
            services,
        }
    }

    #[test]
    fn topological_order_resolves_dag() {
        let mut services = BTreeMap::new();
        services.insert("db".to_owned(), svc(&[]));
        services.insert("api".to_owned(), svc(&["db"]));
        services.insert("web".to_owned(), svc(&["api"]));
        let p = profile(services);

        #[expect(
            clippy::unwrap_used,
            reason = "test: a valid DAG always yields an order and contains every node"
        )]
        {
            let order = p.topological_order().unwrap();
            let db = order.iter().position(|n| n == "db").unwrap();
            let api = order.iter().position(|n| n == "api").unwrap();
            let web = order.iter().position(|n| n == "web").unwrap();
            assert!(db < api, "db must precede api");
            assert!(api < web, "api must precede web");
        }
    }

    #[test]
    fn topological_order_detects_cycle() {
        let mut services = BTreeMap::new();
        services.insert("a".to_owned(), svc(&["b"]));
        services.insert("b".to_owned(), svc(&["a"]));
        let p = profile(services);
        let result = p.topological_order();
        assert!(
            matches!(&result, Err(LaunchError::CycleDetected { nodes }) if nodes == &["a".to_owned(), "b".to_owned()]),
            "expected CycleDetected with [a, b], got {result:?}"
        );
    }

    #[test]
    fn validate_rejects_command_as_string() {
        let mut services = BTreeMap::new();
        let mut s = svc(&[]);
        s.command = CommandSpec::Shell("echo hi".to_owned());
        services.insert("web".to_owned(), s);
        let p = profile(services);
        assert!(matches!(p.validate(), Err(LaunchError::InvalidProfile { .. })));
    }

    #[test]
    fn command_string_deserializes_then_rejected_by_validate() {
        // A bare-string command must parse permissively (no serde panic) and only
        // be rejected by validate(), per launch-command-string-rejected.feature.
        let json = r#"{"command":"echo hi"}"#;
        #[expect(clippy::unwrap_used, reason = "test: fixed JSON literal deserializes")]
        let service: LaunchService = serde_json::from_str(json).unwrap();
        assert!(matches!(service.command, CommandSpec::Shell(_)));
        assert!(matches!(
            service.command.argv(),
            Err(LaunchError::InvalidProfile { .. })
        ));
    }

    #[test]
    fn validate_rejects_empty_command() {
        let mut services = BTreeMap::new();
        let mut s = svc(&[]);
        s.command = CommandSpec::Argv(Vec::new());
        services.insert("web".to_owned(), s);
        let p = profile(services);
        assert!(matches!(p.validate(), Err(LaunchError::InvalidProfile { .. })));
    }

    #[test]
    fn validate_rejects_unknown_dependency() {
        let mut services = BTreeMap::new();
        services.insert("api".to_owned(), svc(&["ghost"]));
        let p = profile(services);
        assert!(matches!(p.validate(), Err(LaunchError::InvalidProfile { .. })));
    }

    #[test]
    fn validate_rejects_orphan_ttl_out_of_range() {
        let mut p = profile(BTreeMap::new());
        p.orphan_ttl_secs = ORPHAN_TTL_MAX_SECS + 1;
        assert!(matches!(p.validate(), Err(LaunchError::InvalidProfile { .. })));
    }

    #[test]
    fn validate_accepts_minimal_valid_profile() {
        let mut services = BTreeMap::new();
        services.insert("web".to_owned(), svc(&[]));
        let p = profile(services);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn channel_bounds_defaults_match_cue() {
        let b = LaunchChannelBounds::default();
        assert_eq!(b.stdout_mpsc_capacity, 1024);
        assert_eq!(b.event_broadcast_capacity, 256);
        assert_eq!(b.notify_rate_per_sec, 5);
        assert_eq!(b.orchestrated_restart_per_min, 60);
    }
}
