//! In-process supervisor for bring-up and teardown of a Stack (ADR-0063, ADR-0068).
//!
//! The supervisor drives the topo-ordered spawn loop (calling [`SubprocessPort::spawn`]
//! for each Service) and the reverse-topo teardown loop (calling [`SubprocessPort::cancel`]).
//! It is **in-process only** for the MVP — the detached supervisor (`substrate --supervise`
//! self-fork, control FIFO, mio reactor, pidfd/kqueue) is Milestone 2.
//!
//! # Phase status
//!
//! **Phase 4 stub.** Bring-up, readiness gating, restart, reload, and event-ring
//! logic will be added in Phase 4. See the build plan for full signatures.
//!
//! References: ADR-0063 §"in-process supervisor", ADR-0065 §"readiness gating",
//! ADR-0068 §"detached supervisor (Milestone 2)".
