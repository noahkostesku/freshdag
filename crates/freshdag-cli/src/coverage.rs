//! `freshdag coverage` — how much of a session did FreshDAG actually see?
//!
//! `docs/BUILD_PLAN.md` W12 calls this "the deliverable", and names the
//! risk it exists to expose: *"The honest outcome may be 'we saw 20% of
//! it.' That is the point."*
//!
//! # Why this refuses to print most of the metrics
//!
//! `docs/EVALUATION.md §3` lists six Tier-1 metrics. Exactly one of them
//! is computable from a store today. Rather than approximate the other
//! five, this command names each one and says what is missing, because a
//! coverage report that quietly substitutes a computable number for an
//! uncomputable one is the same class of lie as a certificate that
//! reports `unknown` evidence as `valid`.
//!
//! # What it does compute
//!
//! The store already carries the whole picture, and none of it is
//! inferred here:
//!
//! - `ComputationNode::obligations` — every `bash`/`task` call, which the
//!   adapter is structurally blind inside.
//! - `ComputationNode::excluded` — observations the store saw, could
//!   identify, and refused to promote to a dependency, each with a
//!   reason.
//! - `ComputationNode::dependencies` — what actually reached the graph.
//! - `ComputationCoverage` — who declared what, and which producers have
//!   no manifest at all.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use freshdag_core::ir::EventKind;
use freshdag_store::{DerivedGraph, ExclusionReason, Store, StoreError, LOG_FILE_NAME};

use crate::exit::Exit;

/// Everything that stops `coverage` from producing a report.
#[derive(Debug)]
pub enum CoverageError {
    /// The path given is not a FreshDAG store.
    NoStore { path: PathBuf },
    /// The store could not be read.
    Store(StoreError),
    /// The report could not be serialized for `--json`.
    Json(serde_json::Error),
}

impl std::fmt::Display for CoverageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoStore { path } => write!(
                f,
                "no FreshDAG store at `{}` (expected `{LOG_FILE_NAME}` inside it)",
                path.display()
            ),
            Self::Store(e) => write!(f, "store error: {e}"),
            Self::Json(e) => write!(f, "the report could not be serialized: {e}"),
        }
    }
}

impl CoverageError {
    /// Always `> 2`. A failure to report coverage is a tool error, and
    /// says nothing about any artifact.
    pub fn exit(&self) -> Exit {
        match self {
            Self::NoStore { .. } => Exit::Usage,
            Self::Store(_) | Self::Json(_) => Exit::StoreError,
        }
    }
}

/// The measured picture of one store.
#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    /// Events in the canonical log.
    pub events: usize,
    /// Computations the log describes.
    pub computations: usize,
    /// Is the replay reproducible from any physical input order?
    pub deterministic: bool,
    /// Producers with a registered manifest, `producer/version`.
    pub registered_producers: Vec<String>,
    /// Producers that emitted events but registered no manifest. Their
    /// silences cannot be interpreted at all.
    pub unregistered_producers: Vec<String>,
    /// Tool invocations by `tool_kind`.
    pub tool_calls: BTreeMap<String, usize>,
    /// Tool calls the adapter is structurally blind inside
    /// (`bash`/`task`).
    pub blind_tool_calls: usize,
    /// Total tool calls.
    pub total_tool_calls: usize,
    /// `bash`/`task` observation obligations raised.
    pub obligations: usize,
    /// Can any registered producer discharge them? Requires an
    /// `Observer`-role producer declaring fs coverage.
    pub obligations_dischargeable: bool,
    /// Dependency edges that reached the graph.
    pub dependencies: usize,
    /// Observations seen, identified, and deliberately not promoted,
    /// by reason.
    pub excluded: BTreeMap<String, usize>,
    /// Excluded observations that still imply an unproven dependency.
    pub unproven: usize,
    /// Artifacts recorded.
    pub artifacts: usize,
    /// Tier-1 metric: fraction of dependency edges whose contributing
    /// producers declared `partial` coverage for the kind that produced
    /// them. `None` when there are no edges to divide by.
    pub coverage_silence_rate: Option<f64>,
}

impl Report {
    /// Of the actions this session took, what fraction were ones this
    /// adapter can see into at all?
    ///
    /// **Not the same as "produced usable dependency evidence"**, which
    /// is what this doc comment used to claim. Only `Read` can yield a
    /// dependency edge — an `fs.write` is an output, never a dependency
    /// — so the fraction of calls able to contribute evidence is
    /// strictly smaller than this number, and at the first dogfood
    /// measurement it was 2/60 rather than 6/60. This is the weaker,
    /// more generous quantity; treat it as an upper bound.
    ///
    /// This is the number the observer decision turns on, and it is
    /// deliberately pessimistic: a `bash` call that touched nothing
    /// still counts against it, because the store cannot know that it
    /// touched nothing — that is exactly the blindness being measured.
    #[must_use]
    pub fn observable_fraction(&self) -> Option<f64> {
        if self.total_tool_calls == 0 {
            return None;
        }
        let seen = self.total_tool_calls - self.blind_tool_calls;
        // `as` on small counts is exact; clippy's cast lint is about
        // precision loss that cannot occur at these magnitudes.
        Some(
            f64::from(u32::try_from(seen).unwrap_or(u32::MAX))
                / f64::from(u32::try_from(self.total_tool_calls).unwrap_or(u32::MAX)),
        )
    }
}

/// Build the coverage report for the store at `store_root`.
///
/// # Errors
///
/// See [`CoverageError`].
pub fn run(store_root: &Path) -> Result<Report, CoverageError> {
    if !store_root.join(LOG_FILE_NAME).is_file() {
        return Err(CoverageError::NoStore {
            path: store_root.to_path_buf(),
        });
    }

    let store = Store::open(store_root).map_err(CoverageError::Store)?;
    let events = store.read_log().map_err(CoverageError::Store)?;
    let registry = store.coverage().clone();
    let graph = DerivedGraph::replay(events.clone(), &registry);

    let mut report = Report {
        events: events.len(),
        deterministic: graph.is_deterministic(),
        ..Report::default()
    };

    for (key, _) in registry.manifests() {
        report
            .registered_producers
            .push(format!("{}/{}", key.producer, key.version));
    }

    let mut partial_backed = 0usize;
    let mut total_edges = 0usize;

    for node in graph.computations() {
        report.computations += 1;
        report.dependencies += node.dependencies.len();
        report.obligations += node.obligations.len();
        report.artifacts += node.artifacts.len();

        for excluded in &node.excluded {
            let label = match &excluded.reason {
                ExclusionReason::ReadAfterOwnWrite => "read-after-own-write",
                ExclusionReason::Impure => "impure",
                ExclusionReason::NoFingerprint => "no-fingerprint",
                ExclusionReason::NakedVolatile => "naked-volatile",
                ExclusionReason::MalformedPayload(_) => "malformed-payload",
            };
            *report.excluded.entry(label.to_string()).or_default() += 1;
            if excluded.reason.is_unproven_dependency() {
                report.unproven += 1;
            }
        }

        if let Some(attribution) = graph.attribution(&node.computation_id) {
            for key in &attribution.unregistered {
                let name = format!("{}/{}", key.producer, key.version);
                if !report.unregistered_producers.contains(&name) {
                    report.unregistered_producers.push(name);
                }
            }
            if freshdag_engine::coverage::has_fs_covered_observer(&attribution.entries) {
                report.obligations_dischargeable = true;
            }
            // Tier-1 "coverage silence rate": an edge is silence-backed
            // when a contributing producer declared partial coverage for
            // the kind that produces edges.
            let fs_read_partial = attribution
                .partial_notes
                .get(EventKind::FsRead.as_wire_str())
                .is_some_and(|notes| !notes.is_empty());
            total_edges += node.dependencies.len();
            if fs_read_partial {
                partial_backed += node.dependencies.len();
            }
        }
    }

    // Tool calls, read straight off the log: `tool_kind` is on the
    // `tool.invoked` payload and the store does not model it as graph
    // material.
    for event in &events {
        if event.kind != EventKind::ToolInvoked {
            continue;
        }
        let kind = event
            .payload
            .get("tool_kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        *report.tool_calls.entry(kind.to_string()).or_default() += 1;
        report.total_tool_calls += 1;
        if matches!(kind, "bash" | "task") {
            report.blind_tool_calls += 1;
        }
    }

    report.coverage_silence_rate = (total_edges > 0).then(|| {
        f64::from(u32::try_from(partial_backed).unwrap_or(u32::MAX))
            / f64::from(u32::try_from(total_edges).unwrap_or(u32::MAX))
    });

    Ok(report)
}

/// Serialize a report for `--json`.
///
/// # Errors
///
/// [`CoverageError::Json`] if serialization fails.
pub fn to_json(report: &Report) -> Result<String, CoverageError> {
    serde_json::to_string_pretty(report).map_err(CoverageError::Json)
}

/// Render the report for a human.
#[must_use]
pub fn render(report: &Report) -> String {
    let mut out = String::new();

    out.push_str("Store\n");
    let _ = writeln!(out, "  events            {}", report.events);
    let _ = writeln!(out, "  computations      {}", report.computations);
    let _ = writeln!(
        out,
        "  replay            {}",
        if report.deterministic {
            "deterministic"
        } else {
            "NOT REPRODUCIBLE — a duplicate event_id carried different content"
        }
    );
    for producer in &report.registered_producers {
        let _ = writeln!(out, "  producer          {producer}");
    }
    for producer in &report.unregistered_producers {
        let _ = writeln!(
            out,
            "  producer          {producer}  (NO MANIFEST — its silence means nothing)"
        );
    }

    out.push_str("\nWhat FreshDAG could see\n");
    if report.total_tool_calls == 0 {
        out.push_str("  no tool calls recorded\n");
    } else {
        for (kind, count) in &report.tool_calls {
            let blind = if matches!(kind.as_str(), "bash" | "task") {
                "  <- blind inside"
            } else {
                ""
            };
            let _ = writeln!(out, "  {kind:<16}  {count}{blind}");
        }
        if let Some(fraction) = report.observable_fraction() {
            let _ = writeln!(
                out,
                "\n  {} of {} tool calls were ones this adapter can see into ({:.0}%).",
                report.total_tool_calls - report.blind_tool_calls,
                report.total_tool_calls,
                fraction * 100.0
            );
        }
    }

    out.push_str("\nWhat reached the graph\n");
    let _ = writeln!(out, "  dependency edges  {}", report.dependencies);
    let _ = writeln!(out, "  artifacts         {}", report.artifacts);
    if report.excluded.is_empty() {
        out.push_str("  excluded          none\n");
    } else {
        for (reason, count) in &report.excluded {
            let _ = writeln!(out, "  excluded          {count} x {reason}");
        }
        let _ = writeln!(
            out,
            "  of those, {} still imply a dependency exists but unproven",
            report.unproven
        );
    }

    out.push_str("\nObservation obligations (bash/task)\n");
    let _ = writeln!(out, "  raised            {}", report.obligations);
    let _ = writeln!(
        out,
        "  dischargeable     {}",
        if report.obligations_dischargeable {
            "yes — an fs-covering observer contributed to this computation"
        } else {
            "NO — no Observer-role producer declares fs coverage here"
        }
    );

    out.push_str("\nTier-1 metrics (docs/EVALUATION.md §3)\n");
    match report.coverage_silence_rate {
        Some(rate) => {
            let _ = writeln!(out, "  coverage silence rate      {:.0}%", rate * 100.0);
        }
        None => out.push_str("  coverage silence rate      n/a (no dependency edges)\n"),
    }
    // Naming what cannot be computed, rather than approximating it. A
    // report that substitutes a computable number for an uncomputable
    // one is the same class of lie as reporting `unknown` as `valid`.
    out.push_str(
        "  cache hit rate             not computable — `check` does not record its own \
         results yet (BUILD_PLAN W10)\n",
    );
    out.push_str(
        "  wall time saved            not computable — the adapter reports \
         duration_ms: 0 (duration_observed: false)\n",
    );
    out.push_str(
        "  $ saved                    not computable — no token-cost signal reaches \
         the IR\n",
    );
    out.push_str(
        "  replay determinism rate    fixture-suite metric, not a store metric; this \
         store's own replay is reported above\n",
    );
    out.push_str(
        "  undeclared-dep catch count not computable — requires an observer to \
         contradict the adapter\n",
    );

    out
}
