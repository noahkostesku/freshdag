//! FreshDAG storage layer.
//!
//! Owns the append-only canonical observation log and the derived graph
//! materialized from it. Derived state is reconstructable from the log
//! alone — architectural invariants #4 (append-only) and #5 (derived
//! state is disposable) in `ARCHITECTURE.md`, and
//! `docs/adr/0005-append-only-observations.md`.
//!
//! # Shape
//!
//! - [`JsonlSink`] appends [`IrEvent`]s as newline-delimited JSON. It
//!   never rewrites history, and it never drops an event silently: when a
//!   byte budget is exhausted it drops the **newest** event, records a
//!   `diagnostic`, and reports [`AppendOutcome::DroppedNewest`] to the
//!   caller.
//! - [`read_log`] / [`scan_log`] read the log back. Neither one ever
//!   skips a record quietly; a truncated trailing record (a process
//!   killed mid-append) is an error in the strict path and a
//!   [`LogDefect`] in the tolerant one.
//! - [`ProducerStreams`] gives the per-producer view: totally ordered by
//!   `event_id` within each producer, as `docs/contracts/execution-ir.md
//!   §Ordering` specifies.
//! - [`linearize`] gives the canonical cross-producer total order
//!   `(ts, producer, event_id)`. This is the determinism guarantee
//!   invariant #5 rests on.
//! - [`CoverageRegistry`] records which producers declared what, so the
//!   engine can populate `Certificate.observation_coverage`.
//! - [`DerivedGraph`] is the replay of the log into three derived views:
//!   per-computation dependency edges, the reverse `depends_on` index
//!   (blast radius), and per-computation producer attribution. It is
//!   built *only* by replaying, and it materializes under `derived/`,
//!   which may be deleted at any time.
//! - [`Store`] is the directory-level facade over the two canonical
//!   files and the disposable `derived/` directory.
//!
//! # Three orders, deliberately distinct
//!
//! | Order | Source | Use |
//! | --- | --- | --- |
//! | physical | [`read_log`] | forensics, "what actually hit the disk" |
//! | producer | [`ProducerStreams`] | per-producer causality |
//! | canonical | [`linearize`] | deterministic replay into derived state |
//!
//! Only the canonical order is reproducible across runs. Materializing
//! derived state from any other order is a bug.
//!
//! [`IrEvent`]: freshdag_core::ir::IrEvent

#![warn(missing_docs)]

mod coverage;
mod derived;
mod error;
mod graph;
mod order;
mod reader;
mod sink;
mod store;

#[cfg(test)]
mod testing;

pub use coverage::{CoverageLookup, CoverageRegistration, CoverageRegistry, ProducerKey};
pub use derived::{
    drop_derived, source_digest, DerivedManifest, LoadedDerived, DERIVED_DIR_NAME, DERIVED_FILES,
    DERIVED_FORMAT,
};
pub use error::StoreError;
pub use graph::{
    Accounting, ComputationCoverage, ComputationNode, CoverageObligation, DerivedGraph,
    EdgeConflict, ExcludedEdge, ExclusionReason, GraphDefect, OutputRecord, ReverseIndexEntry,
    SilenceMeaning, EDGE_BEARING_KINDS,
};
pub use order::{
    linearize, linearize_checked, DuplicateEventId, Linearization, ProducerOrderViolation,
    ProducerStreams,
};
pub use reader::{read_log, scan_log, LogDefect, LogScan};
pub use sink::{AppendOutcome, JsonlSink, SinkOptions, STORE_PRODUCER};
pub use store::{Store, COVERAGE_FILE_NAME, LOG_FILE_NAME};
