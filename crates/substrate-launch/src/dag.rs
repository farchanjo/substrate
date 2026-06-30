//! Topological order helpers and restart closure for the launch DAG (ADR-0065).
//!
//! Thin adapter wrappers over [`substrate_domain::launch::profile::LaunchProfile::topological_order`]
//! that add reverse-order (for `down()`) and the transitive restart closure
//! (for `reload()` cascade).
//!
//! # Phase status
//!
//! **Phase 3 stub.** The following public functions will be added in Phase 3:
//!
//! - `topo_order(p: &LaunchProfile) -> Result<Vec<ServiceName>, LaunchError>`
//! - `reverse_topo(p: &LaunchProfile) -> Result<Vec<ServiceName>, LaunchError>`
//! - `restart_closure(p: &LaunchProfile, changed: &[ServiceName]) -> Vec<ServiceName>`
//!
//! References: ADR-0065 §"dependency DAG", ADR-0063 §"reload reconciler".
