//! External-state probes.
//!
//! Cheap freshness queries against mutable external sources. Probes
//! return trust-classified freshness signals per
//! `docs/contracts/probe-contract.md`.
//!
//! W5.1 ships the `file://` probe. Other schemes (`https`, `attio`,
//! `mcp`, `postgres`, …) land in follow-up workstreams.

#![warn(missing_docs)]

pub mod file;

pub use file::FileProbe;

#[cfg(test)]
mod tests;
