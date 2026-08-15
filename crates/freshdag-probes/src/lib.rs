//! External-state probes.
//!
//! Cheap freshness queries against mutable external sources. Probes
//! return trust-classified freshness signals per
//! `docs/contracts/probe-contract.md`.
//!
//! W5.1 ships the `file://` probe; W5.2 adds the `https://` probe.
//! Other schemes (`attio`, `mcp`, `postgres`, …) land in follow-up
//! workstreams.

#![warn(missing_docs)]

pub mod file;
pub mod https;

pub use file::FileProbe;
pub use https::{ContentHashFallback, HttpsProbe, HttpsProbeConfig};

#[cfg(test)]
mod tests;
