//! FreshDAG graph engine.
//!
//! Consumes canonical observations from the store, materializes the
//! dependency graph, evaluates validity, and decides what needs
//! recomputation. Early cutoff via equivalence lives here.

#![warn(missing_docs)]
