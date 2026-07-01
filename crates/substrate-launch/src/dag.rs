//! Topological order helpers and restart closure for the launch DAG (ADR-0065).
//!
//! Thin adapter wrappers over
//! [`substrate_domain::launch::profile::LaunchProfile::topological_order`] that
//! add reverse-order (for `down()`) and the transitive restart closure (for the
//! `reload()` cascade). The forward order is dependency-first; the reverse order
//! is teardown-first.
//!
//! References: ADR-0065 §"dependency DAG", ADR-0063 §"reload reconciler".

use std::collections::BTreeSet;

use substrate_domain::launch::errors::LaunchError;
use substrate_domain::launch::profile::{DependencyRestartMode, LaunchProfile, ServiceName};

/// Returns the Services in dependency-first topological order.
///
/// A thin delegation to [`LaunchProfile::topological_order`]; a Service appears
/// only after every Service it `depends_on`.
///
/// # Errors
///
/// Returns [`LaunchError::CycleDetected`] when the `depends_on` edges do not form
/// a DAG.
pub fn topo_order(profile: &LaunchProfile) -> Result<Vec<ServiceName>, LaunchError> {
    profile.topological_order()
}

/// Returns the Services in teardown order (reverse topological).
///
/// Used by `down()` so a Service is stopped only after every Service that
/// depends on it has already been stopped.
///
/// # Errors
///
/// Returns [`LaunchError::CycleDetected`] when the `depends_on` edges do not form
/// a DAG.
pub fn reverse_topo(profile: &LaunchProfile) -> Result<Vec<ServiceName>, LaunchError> {
    let mut order = profile.topological_order()?;
    order.reverse();
    Ok(order)
}

/// Computes the transitive set of Services that must restart when `changed`
/// Services restart.
///
/// Walks the reverse dependency edges breadth-first: a Service `y` is added when
/// it `depends_on` a member of the working set **and** its
/// [`DependencyRestartMode`] is [`Restart`](DependencyRestartMode::Restart). A
/// dependent declaring [`Ignore`](DependencyRestartMode::Ignore) is a cut point —
/// neither it nor anything reachable only through it is restarted.
///
/// The originally `changed` Services are not themselves included unless they are
/// reachable as a dependent of another changed Service. The result is returned in
/// forward topological order for deterministic bring-up; if the graph contains a
/// cycle the result falls back to ascending-name order.
#[must_use]
pub fn restart_closure(profile: &LaunchProfile, changed: &[ServiceName]) -> Vec<ServiceName> {
    let mut frontier: Vec<&str> = changed.iter().map(String::as_str).collect();
    let mut closure: BTreeSet<String> = BTreeSet::new();

    while let Some(node) = frontier.pop() {
        for (name, service) in &profile.services {
            if service.on_dependency_restart != DependencyRestartMode::Restart {
                continue;
            }
            if !service.depends_on.iter().any(|d| d == node) {
                continue;
            }
            if closure.insert(name.clone()) {
                frontier.push(name.as_str());
            }
        }
    }

    // Emit in forward topological order when possible for deterministic bring-up.
    profile.topological_order().map_or_else(
        |_| closure.iter().cloned().collect(),
        |order| order.into_iter().filter(|n| closure.contains(n)).collect(),
    )
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: panics are the correct failure mode"
)]
mod tests {
    use std::collections::BTreeMap;

    use substrate_domain::launch::profile::{CommandSpec, LaunchService, StreamMux};
    use substrate_domain::launch::state::DisconnectPolicy;

    use super::*;

    fn svc(deps: &[&str], mode: DependencyRestartMode) -> LaunchService {
        LaunchService {
            command: CommandSpec::Argv(vec!["bin".to_owned()]),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            depends_on: deps.iter().map(|s| (*s).to_owned()).collect(),
            required: true,
            restart_policy: None,
            health_probe: None,
            env_file: Vec::new(),
            on_dependency_restart: mode,
            error_patterns: Vec::new(),
            redact: Vec::new(),
            streams: StreamMux::Multiplexed,
        }
    }

    /// Builds the chain `a <- b <- c` with the given restart mode on b and c.
    fn chain(mode: DependencyRestartMode) -> LaunchProfile {
        let mut services = BTreeMap::new();
        services.insert("a".to_owned(), svc(&[], DependencyRestartMode::Restart));
        services.insert("b".to_owned(), svc(&["a"], mode));
        services.insert("c".to_owned(), svc(&["b"], mode));
        LaunchProfile {
            version: 1,
            on_client_disconnect: DisconnectPolicy::Shutdown,
            orphan_ttl_secs: 3600,
            services,
        }
    }

    #[test]
    fn topo_order_is_dependency_first() {
        let p = chain(DependencyRestartMode::Restart);
        let order = topo_order(&p).unwrap();
        assert_eq!(order, vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]);
    }

    #[test]
    fn reverse_topo_is_teardown_first() {
        let p = chain(DependencyRestartMode::Restart);
        let order = reverse_topo(&p).unwrap();
        assert_eq!(order, vec!["c".to_owned(), "b".to_owned(), "a".to_owned()]);
    }

    #[test]
    fn restart_closure_is_transitive_when_all_restart() {
        let p = chain(DependencyRestartMode::Restart);
        let closure = restart_closure(&p, &["a".to_owned()]);
        assert_eq!(closure, vec!["b".to_owned(), "c".to_owned()]);
    }

    #[test]
    fn restart_closure_stops_at_ignore_cut_point() {
        // b declares Ignore, so changing a restarts nothing reachable through b.
        let p = chain(DependencyRestartMode::Ignore);
        let closure = restart_closure(&p, &["a".to_owned()]);
        assert!(closure.is_empty(), "Ignore on b must cut the cascade; got {closure:?}");
    }

    #[test]
    fn restart_closure_empty_when_changed_has_no_dependents() {
        let p = chain(DependencyRestartMode::Restart);
        let closure = restart_closure(&p, &["c".to_owned()]);
        assert!(closure.is_empty(), "leaf change restarts nothing; got {closure:?}");
    }

    #[test]
    fn reverse_topo_propagates_cycle_error() {
        let mut services = BTreeMap::new();
        services.insert("x".to_owned(), svc(&["y"], DependencyRestartMode::Restart));
        services.insert("y".to_owned(), svc(&["x"], DependencyRestartMode::Restart));
        let p = LaunchProfile {
            version: 1,
            on_client_disconnect: DisconnectPolicy::Shutdown,
            orphan_ttl_secs: 3600,
            services,
        };
        assert!(matches!(reverse_topo(&p), Err(LaunchError::CycleDetected { .. })));
    }
}
