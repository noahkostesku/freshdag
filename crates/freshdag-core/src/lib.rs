//! FreshDAG core domain model.
//!
//! This crate defines the vocabulary FreshDAG is built on:
//! [`dependency::Dependency`], [`dependency::Fingerprint`],
//! [`dependency::TrustClass`], [`dependency::Validity`],
//! [`artifact::Artifact`], [`computation::Computation`],
//! [`certificate::Certificate`]. It has no runtime, no I/O, and no
//! dependency on any specific agent framework.
//!
//! See `ARCHITECTURE.md` and `docs/contracts/` for the contracts that
//! govern how these types are used across the workspace.

#![warn(missing_docs)]

pub mod artifact;
pub mod certificate;
pub mod computation;
pub mod dependency;
pub mod ir;
pub mod probe;

/// Equivalence comparators — interface only in v0. See
/// `docs/contracts/comparator-contract.md` and
/// `docs/BUILD_PLAN.md §7`.
pub mod equivalence {}

#[cfg(test)]
mod tests;
