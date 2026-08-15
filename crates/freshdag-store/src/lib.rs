//! FreshDAG storage layer.
//!
//! Owns the append-only canonical observation log and the derived-state
//! materializations (graph, indices). Derived state must be reconstructable
//! from the canonical log — see architectural invariant #4 in
//! `ARCHITECTURE.md`.

#![warn(missing_docs)]
