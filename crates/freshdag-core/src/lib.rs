//! FreshDAG core domain model.
//!
//! This crate defines the vocabulary FreshDAG is built on: `Dependency`,
//! `Fingerprint`, `Validity`, `Artifact`, `Computation`. It has no
//! runtime, no I/O, and no dependency on any specific agent framework.
//!
//! See `ARCHITECTURE.md` and `docs/contracts/` for the contracts that govern
//! how these types are used across the workspace.

#![warn(missing_docs)]

pub mod ir;

/// Dependency, Fingerprint, Validity, and the trust classes.
///
/// Types land with S1 (see `docs/BUILD_PLAN.md §2`).
pub mod dependency {}

/// Artifact identity and validity certificates.
pub mod artifact {}

/// Computation identity and the observed-inputs/produced-outputs relation.
pub mod computation {}

/// Equivalence comparators (exact, json-structural, set, numeric, judge, ...).
///
/// Interfaces land with S1; implementations are deferred to the
/// recomputation workstream (see `docs/BUILD_PLAN.md §7`).
pub mod equivalence {}
