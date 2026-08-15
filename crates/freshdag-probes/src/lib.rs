//! External-state probes.
//!
//! Cheap freshness queries against mutable external sources (HTTP ETag/HEAD,
//! versioned API endpoints, file mtime, database change tokens). Probes
//! return trust-classified freshness signals — see
//! `docs/contracts/probe-contract.md`.

#![warn(missing_docs)]
