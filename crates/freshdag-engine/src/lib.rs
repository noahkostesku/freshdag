//! FreshDAG graph engine.
//!
//! Consumes canonical observations from the store, materializes the
//! dependency graph, evaluates validity, and emits certificates.
//!
//! # This crate exists because of invariant #7
//!
//! *Unknown dependency state must never silently become "fresh."* Every
//! other subsystem produces evidence; this crate is where evidence
//! becomes a claim. Three properties hold by construction rather than by
//! convention:
//!
//! - **`Validity::aggregate` is the only source of a status value.** The
//!   engine never computes `status.value` by hand.
//! - **Every certificate passes through [`seal`](crate::seal), whose
//!   only path to a `Certificate` runs
//!   [`Certificate::check_coverage_deficit`].** See that module's header
//!   for the three mechanisms enforcing it.
//! - **"No probe ran" and "a probe ran and could not decide" are
//!   different codes.** `NoProbeAvailable` vs `ProbeUnknown`; conflating
//!   them makes `freshdag why` state something false.
//!
//! # Shape
//!
//! - [`ProbeRegistry`] — registration and arbitration by
//!   `(scheme, host_pattern, priority)`. Ties fail loudly.
//! - [`TrustLedger`] — the anti-thrash protocol: N=2 consecutive
//!   observations before a trust escalation is adopted, explicit
//!   demotion via `probe.trust_demoted`.
//! - [`Engine`] — per-edge probe dispatch and aggregation.
//! - [`EvalClock`] — injected; the engine never reads the wall clock
//!   ambiently, so TTL expiry is deterministic.
//! - [`ScriptedProbe`] — fixture-only probe whose answers are supplied
//!   rather than observed.
//!
//! # Out of scope in v0
//!
//! Recomputation, comparators, equivalence, and early cutoff. v0 is
//! **detection**.
//!
//! [`Certificate::check_coverage_deficit`]: freshdag_core::certificate::Certificate::check_coverage_deficit

#![warn(missing_docs)]

pub mod antithrash;
pub mod clock;
pub mod coverage;
mod engine;
mod error;
pub mod registry;
pub mod scripted;
mod seal;

#[cfg(test)]
mod tests;

pub use antithrash::{TrustLedger, TrustTransition, ESCALATION_THRESHOLD};
pub use clock::{EvalClock, FrozenClock, SystemClock};
pub use engine::{
    is_usable, CheckOutcome, Engine, EngineBuilder, DEFAULT_MAX_VOLATILE_TTL, ENGINE_PRODUCER,
    ENGINE_VERSION, MAX_CLOCK_SKEW,
};
pub use error::EngineError;
pub use registry::{NoProbe, ProbeIdentity, ProbeRegistry, RegistrationError, Selected};
pub use scripted::ScriptedProbe;
pub use seal::CoverageAuthority;
