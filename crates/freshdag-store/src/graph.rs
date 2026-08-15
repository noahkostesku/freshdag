//! The derived dependency graph.
//!
//! This module is invariant #5 made executable: everything here is a
//! *projection* of the canonical append-only log. Delete it, replay the
//! log, get the same bytes back. Nothing in this module is a source of
//! truth and nothing in it may be written to except by replay.
//!
//! # The three views
//!
//! 1. [`ComputationNode`] — per-computation dependency edges, outputs,
//!    artifacts, coverage obligations, and the events the store
//!    deliberately did *not* turn into edges.
//! 2. [`ReverseIndexEntry`] — the blast-radius query: which artifacts
//!    consume a given [`DependencyId`].
//! 3. [`ComputationCoverage`] — which registered producers actually
//!    emitted for a computation, i.e. the attribution that feeds
//!    `Certificate.observation_coverage`.
//!
//! # Edge-derivation rules
//!
//! These are deliberately conservative. Anything the store cannot
//! *prove* is an edge is recorded as an [`ExcludedEdge`] with a reason —
//! never dropped. Invariant #7: unknown is not fresh, and unknown is
//! also not "didn't happen."
//!
//! | Event | Becomes |
//! | --- | --- |
//! | `fs.read` with a `hash` | an `Exact` `file://` dependency |
//! | `fs.read` with no `hash` | [`ExclusionReason::NoFingerprint`] |
//! | `fs.read` flagged `impure` | [`ExclusionReason::Impure`] |
//! | `fs.read` of a path this computation already wrote | [`ExclusionReason::ReadAfterOwnWrite`] |
//! | `fs.write` | an **output**, never a dependency |
//! | `probe.checked` with a parseable fingerprint | a dependency at the probe's declared trust class |
//! | `probe.checked`, `volatile`, no `ttl_seconds` | [`ExclusionReason::NakedVolatile`] |
//! | `artifact.produced` | an artifact of the computation |
//! | `tool.invoked` of kind `bash`/`task` | a [`CoverageObligation`], **not** an edge |
//! | everything else | counted in [`ComputationNode::unmodeled`] |
//!
//! ## Read-after-own-write
//!
//! A read of a path the same computation previously wrote — *previously*
//! in the canonical linearization from `docs/contracts/execution-ir.md
//! §Ordering* — is internal state, not an external dependency. It is
//! excluded. A read that happens *before* the first write to that path
//! (read-modify-write) **is** an external dependency and is kept. This
//! is precisely why the derivation must run over `linearize`d events and
//! never over physical file order.
//!
//! Two events at the same instant from different producers are ordered
//! by producer name. That is deterministic but arbitrary with respect to
//! true causality, so a same-instant write/read pair may classify either
//! way. It classifies the *same* way on every replay, which is what
//! invariant #5 requires; producers that care must stamp distinct
//! timestamps or emit `causal_inputs`.
//!
//! ## `bash` / `task` do not create edges but do create obligations
//!
//! An adapter is blind inside a subprocess. Recording a `bash`
//! invocation as "no dependencies" would be the exact silence-equals-
//! absence conflation invariant #7 forbids, so every such invocation is
//! recorded as a [`CoverageObligation`] on the node. The store does not
//! decide whether the obligation is discharged — that is the engine's
//! call under `docs/contracts/certificate-contract.md §Coverage-Deficit
//! Rule` — but the fact is never lost.
//!
//! ## `net.fetch` deliberately creates no edge
//!
//! Assigning a trust class to an HTTP response is the probe contract's
//! job (`docs/contracts/probe-contract.md §Trust-class Semantics`), not
//! the store's. Network dependencies enter the graph through
//! `probe.checked`. `net.fetch` is counted in `unmodeled` so a consumer
//! can see that network I/O happened without the store pretending to
//! have classified it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use freshdag_core::artifact::ArtifactId;
use freshdag_core::certificate::CoverageEntry;
use freshdag_core::computation::ComputationId;
use freshdag_core::dependency::{Dependency, DependencyId, Fingerprint, TrustClass};
use freshdag_core::ir::{EventKind, IrEvent, TypedPayload};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::coverage::{CoverageRegistry, ProducerKey};
use crate::order::{linearize_checked, DuplicateEventId};

/// Event kinds that are required by `docs/contracts/execution-ir.md
/// §Event Envelope` to carry a `computation_id`, because they may
/// contribute to a dependency edge or a coverage obligation.
///
/// An event of one of these kinds with no `computation_id` is a producer
/// bug: it cannot be attributed to any computation and would otherwise
/// vanish from the graph. It is reported as
/// [`GraphDefect::OrphanEdgeEvent`].
pub const EDGE_BEARING_KINDS: &[EventKind] = &[
    EventKind::FsRead,
    EventKind::FsWrite,
    EventKind::FsStat,
    EventKind::FsRename,
    EventKind::FsUnlink,
    EventKind::FsDirlist,
    EventKind::NetFetch,
    EventKind::ProbeChecked,
    EventKind::ArtifactProduced,
    EventKind::ToolInvoked,
];

// --------------------------------------------------------------- edges

/// Why an observation that *looks* like a dependency was not turned into
/// one.
///
/// Every variant is a deliberate, documented decision. An excluded edge
/// is retained on the node so a consumer can tell "the store saw this and
/// declined to call it a dependency" apart from "the store never saw
/// anything."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// A read of a path this same computation had already written. This
    /// is internal state, not an external dependency.
    ReadAfterOwnWrite,
    /// The producer flagged the read impure (clock, `/dev/urandom`, …).
    /// An impure source has no meaningful fingerprint and no meaningful
    /// blast radius; it is a reproducibility problem, which is the
    /// engine's to reason about.
    Impure,
    /// No fingerprint was observed, so no [`Dependency`] can be
    /// constructed without fabricating evidence. Invariant #7.
    NoFingerprint,
    /// A `volatile` observation arrived without a `ttl_seconds`.
    /// `Dependency::is_consistent` forbids naked-volatile, so emitting
    /// this edge would put a self-inconsistent dependency in the graph.
    NakedVolatile,
    /// The payload did not match the shape its kind requires.
    MalformedPayload(String),
}

impl ExclusionReason {
    /// Does this exclusion still imply that a dependency *exists* at this
    /// key, merely unproven?
    ///
    /// Load-bearing for blast radius. Under-reporting ("nothing breaks if
    /// this changes") is the dangerous direction, so an unproven
    /// observation still places the computation in
    /// [`ReverseIndexEntry::unproven_computations`].
    ///
    /// [`Self::ReadAfterOwnWrite`] and [`Self::Impure`] are *not*
    /// unproven: they are positive findings that no external dependency
    /// exists at that key for that computation.
    pub const fn is_unproven_dependency(&self) -> bool {
        match self {
            Self::NoFingerprint | Self::NakedVolatile | Self::MalformedPayload(_) => true,
            Self::ReadAfterOwnWrite | Self::Impure => false,
        }
    }
}

/// An observation the store saw, could identify, and deliberately did not
/// promote to a [`Dependency`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedEdge {
    /// Event that was excluded.
    pub event_id: Uuid,
    /// Kind of the excluded event.
    pub kind: EventKind,
    /// Best-effort dependency key, when one could be recovered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Why it was excluded.
    pub reason: ExclusionReason,
}

/// The same dependency observed twice within one computation with a
/// different fingerprint or trust class.
///
/// This is evidence the world moved underneath the computation. The store
/// keeps the first canonical observation as the edge and records the
/// divergence here; deciding what it means for validity is W4's job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeConflict {
    /// The dependency observed twice.
    pub dependency: DependencyId,
    /// Event that produced the retained edge.
    pub first_event_id: Uuid,
    /// Fingerprint on the retained edge.
    pub first_fingerprint: Fingerprint,
    /// Event whose observation disagreed.
    pub conflicting_event_id: Uuid,
    /// The disagreeing fingerprint.
    pub conflicting_fingerprint: Fingerprint,
}

/// A file this computation wrote. Outputs are not dependencies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRecord {
    /// Event that recorded the write.
    pub event_id: Uuid,
    /// Canonicalized absolute path written.
    pub path: PathBuf,
    /// Content hash, if the producer computed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// A `bash` or `task` invocation: an observation obligation the adapter
/// cannot discharge by itself.
///
/// See `docs/contracts/execution-ir.md §Tool interaction` and
/// `docs/contracts/certificate-contract.md §Coverage-Deficit Rule`. The
/// store records the obligation; only the engine decides whether it was
/// met.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageObligation {
    /// The `tool.invoked` event that created the obligation.
    pub event_id: Uuid,
    /// Producer that reported the invocation.
    pub producer: String,
    /// Tool name as normalized by the adapter.
    pub tool_name: String,
    /// Raw `tool_kind` string — `bash` or `task`. Kept as a string
    /// rather than a typed enum so a payload whose other fields fail to
    /// decode still yields the obligation.
    pub tool_kind: String,
}

// ------------------------------------------------------------- defects

/// A structural problem in the log that the derived graph must not hide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphDefect {
    /// An event of an [`EDGE_BEARING_KINDS`] kind carried no
    /// `computation_id`, so it belongs to no node.
    OrphanEdgeEvent {
        /// The orphaned event.
        event_id: Uuid,
        /// Its kind.
        kind: EventKind,
        /// Producer that emitted it.
        producer: String,
    },
    /// An `artifact.produced` payload named a `produced_by` computation
    /// different from its envelope's `computation_id`.
    ArtifactProducerMismatch {
        /// The offending event.
        event_id: Uuid,
        /// Envelope `computation_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        envelope: Option<String>,
        /// Payload `produced_by`.
        payload: String,
    },
    /// A producer emitted the same `event_id` twice with different
    /// content, so the canonical linearization cannot separate the
    /// copies and this replay is not reproducible.
    DivergentDuplicate {
        /// The repeated identifier.
        event_id: Uuid,
        /// Producer that repeated it.
        producer: String,
    },
}

// ---------------------------------------------------------- attribution

/// What the absence of events of some kind means for one computation.
///
/// There is deliberately no variant meaning "definitely did not happen."
/// [`Self::ObservedAbsent`] is as strong as the store will ever put it,
/// and even that is bounded by the producer's declared fidelity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SilenceMeaning {
    /// No producer that contributed to this computation declares
    /// coverage for this kind — or one of them has no registered
    /// manifest at all. Absence means **unobserved**. Invariant #7.
    Unobserved,
    /// A contributing producer declares coverage but flags it partial.
    /// Treat with the same suspicion as [`Self::Unobserved`].
    PartiallyObserved(Vec<String>),
    /// At least one contributing producer declares full coverage for
    /// this kind and emitted nothing. Absence is evidence of absence, to
    /// the limit of that producer's fidelity.
    ObservedAbsent,
}

/// Which registered producers actually emitted events for a computation.
///
/// This is the per-computation attribution that feeds
/// `Certificate.observation_coverage`. It reuses
/// [`CoverageEntry`] (and its `From<&CoverageManifest>`) rather than
/// inventing a parallel type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputationCoverage {
    /// The computation this attribution is for.
    pub computation_id: ComputationId,
    /// Registered producers that emitted at least one event for it, in
    /// `(producer, version)` order.
    pub entries: Vec<CoverageEntry>,
    /// Producers that emitted for it but have no registered manifest.
    /// Their silences cannot be interpreted at all.
    pub unregistered: Vec<ProducerKey>,
    /// Partial-coverage notes declared by contributing producers, keyed
    /// by event-kind wire string.
    pub partial_notes: BTreeMap<String, Vec<String>>,
}

impl ComputationCoverage {
    /// Does any contributing producer declare coverage for `kind`?
    pub fn covers(&self, kind: EventKind) -> bool {
        self.entries.iter().any(|e| e.covers(kind))
    }

    /// Interpret the absence of events of `kind` for this computation.
    ///
    /// Callers must only consult this when the computation actually has
    /// zero events of that kind; [`DerivedGraph::silence`] does that
    /// check for you.
    pub fn interpret_silence(&self, kind: EventKind) -> SilenceMeaning {
        if !self.unregistered.is_empty() || !self.covers(kind) {
            return SilenceMeaning::Unobserved;
        }
        if let Some(notes) = self.partial_notes.get(kind.as_wire_str()) {
            if !notes.is_empty() {
                return SilenceMeaning::PartiallyObserved(notes.clone());
            }
        }
        SilenceMeaning::ObservedAbsent
    }
}

// ---------------------------------------------------------------- node

/// Per-event accounting for one computation.
///
/// The store's guarantee is that **every** event carrying this
/// computation's `computation_id` lands in exactly one bucket. If
/// [`Self::is_balanced`] is false, some observation went missing, which
/// is a bug in this module rather than a fact about the world.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accounting {
    /// Events that yielded a dependency edge (including duplicate and
    /// conflicting re-observations of an edge already recorded).
    pub edge_events: usize,
    /// `fs.write` events.
    pub outputs: usize,
    /// `artifact.produced` events.
    pub artifacts: usize,
    /// `bash`/`task` obligations.
    pub obligations: usize,
    /// Events recognized but deliberately excluded from the graph.
    pub excluded: usize,
    /// Events of kinds the store does not model as graph material.
    pub unmodeled: usize,
    /// Total events carrying this computation's id.
    pub total: usize,
}

impl Accounting {
    /// Does every event fall into exactly one bucket?
    pub fn is_balanced(&self) -> bool {
        self.edge_events + self.outputs + self.artifacts + self.obligations + self.excluded
            + self.unmodeled
            == self.total
    }
}

/// One computation's derived state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputationNode {
    /// Which computation this is.
    pub computation_id: ComputationId,
    /// Dependency edges, in canonical observation order, unique by
    /// [`DependencyId`] (first canonical observation wins).
    pub dependencies: Vec<Dependency>,
    /// Files this computation wrote. Outputs, not dependencies.
    pub outputs: Vec<OutputRecord>,
    /// Artifacts this computation produced.
    pub artifacts: Vec<ArtifactId>,
    /// `bash`/`task` observation obligations.
    pub obligations: Vec<CoverageObligation>,
    /// Observations recognized but not promoted to edges.
    pub excluded: Vec<ExcludedEdge>,
    /// Re-observations of an existing edge that disagreed with it.
    pub conflicts: Vec<EdgeConflict>,
    /// Counts of every event kind seen for this computation, keyed by
    /// wire string. Used to tell silence from absence.
    pub kind_counts: BTreeMap<String, usize>,
    /// Counts of kinds the store does not model as graph material.
    pub unmodeled: BTreeMap<String, usize>,
    /// Producers that emitted for this computation, in sorted order.
    pub contributors: Vec<ProducerKey>,
    /// Per-event accounting.
    pub accounting: Accounting,
}

impl ComputationNode {
    fn new(computation_id: ComputationId) -> Self {
        Self {
            computation_id,
            dependencies: Vec::new(),
            outputs: Vec::new(),
            artifacts: Vec::new(),
            obligations: Vec::new(),
            excluded: Vec::new(),
            conflicts: Vec::new(),
            kind_counts: BTreeMap::new(),
            unmodeled: BTreeMap::new(),
            contributors: Vec::new(),
            accounting: Accounting::default(),
        }
    }

    /// Number of events of `kind` observed for this computation.
    pub fn count_of(&self, kind: EventKind) -> usize {
        self.kind_counts
            .get(kind.as_wire_str())
            .copied()
            .unwrap_or(0)
    }

    /// Does this computation carry an undischargeable-by-the-adapter
    /// observation obligation?
    ///
    /// This is a *factual* predicate — "a `bash` or `task` ran" — not a
    /// validity verdict. Whether the obligation was met is decided by
    /// the engine.
    pub fn has_coverage_obligation(&self) -> bool {
        !self.obligations.is_empty()
    }
}

// ------------------------------------------------------- reverse index

/// The blast-radius entry for one dependency: everything that breaks —
/// or might break — if it changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverseIndexEntry {
    /// The dependency being depended on.
    pub dependency: DependencyId,
    /// Its scheme (`file`, `https`, `attio`, …).
    pub scheme: String,
    /// Computations with a fingerprinted edge on it.
    pub consuming_computations: Vec<ComputationId>,
    /// Artifacts produced by [`Self::consuming_computations`]. Empty
    /// with a non-empty computation list means "consumed, but nothing
    /// produced yet" — not "nothing depends on this."
    pub consumers: Vec<ArtifactId>,
    /// Computations that observed this key without producing a usable
    /// edge (no fingerprint, naked volatile, malformed payload). The
    /// dependency probably exists; the store cannot prove its state.
    pub unproven_computations: Vec<ComputationId>,
    /// Artifacts produced by [`Self::unproven_computations`].
    pub unproven_consumers: Vec<ArtifactId>,
}

impl ReverseIndexEntry {
    /// Is every consumer of this dependency backed by a fingerprinted
    /// edge?
    ///
    /// When false, a blast-radius answer computed from
    /// [`Self::consumers`] alone under-reports, and per invariant #7 the
    /// caller must treat [`Self::unproven_consumers`] as possibly
    /// affected rather than as unaffected.
    pub fn is_fully_proven(&self) -> bool {
        self.unproven_computations.is_empty()
    }
}

// --------------------------------------------------------------- graph

/// The materialized derived graph.
///
/// Built only by replaying the canonical log. Construction always
/// linearizes internally — there is no constructor that accepts a
/// caller-supplied order, because materializing from physical order is
/// the bug invariant #5 exists to prevent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedGraph {
    computations: BTreeMap<ComputationId, ComputationNode>,
    reverse_index: BTreeMap<DependencyId, ReverseIndexEntry>,
    attribution: BTreeMap<ComputationId, ComputationCoverage>,
    defects: Vec<GraphDefect>,
    duplicate_event_ids: Vec<Uuid>,
    event_count: usize,
    deterministic: bool,
}

impl DerivedGraph {
    /// Materialize the graph by replaying `events`.
    ///
    /// The iterator may be in any order; it is canonically linearized
    /// first. `coverage` supplies the producer manifests used for
    /// attribution and for silence interpretation.
    pub fn replay(
        events: impl IntoIterator<Item = IrEvent>,
        coverage: &CoverageRegistry,
    ) -> Self {
        let linearization = linearize_checked(events.into_iter());
        let deterministic = linearization.is_deterministic();
        let duplicates: Vec<DuplicateEventId> = linearization.duplicate_event_ids().to_vec();
        let events = linearization.into_events();

        let mut graph = Self {
            deterministic,
            event_count: events.len(),
            duplicate_event_ids: duplicates.iter().map(|d| d.event_id).collect(),
            ..Self::default()
        };
        for d in duplicates.iter().filter(|d| d.divergent) {
            graph.defects.push(GraphDefect::DivergentDuplicate {
                event_id: d.event_id,
                producer: d.producer.clone(),
            });
        }

        let artifact_paths = collect_artifact_paths(&events);
        graph.build_nodes(&events, &artifact_paths);
        graph.build_attribution(&events, coverage);
        graph.build_reverse_index();
        graph
    }

    /// Every computation node, in `ComputationId` order.
    pub fn computations(&self) -> impl Iterator<Item = &ComputationNode> {
        self.computations.values()
    }

    /// One computation's node.
    pub fn computation(&self, id: &ComputationId) -> Option<&ComputationNode> {
        self.computations.get(id)
    }

    /// One computation's dependency edges. An unknown computation has no
    /// edges — which is *not* the same as a known computation with none,
    /// so prefer [`Self::computation`] when the difference matters.
    pub fn dependencies_of(&self, id: &ComputationId) -> &[Dependency] {
        self.computations
            .get(id)
            .map_or(&[][..], |n| &n.dependencies)
    }

    /// The blast radius of a dependency: what breaks, or might break, if
    /// it changes.
    pub fn blast_radius(&self, dependency: &DependencyId) -> Option<&ReverseIndexEntry> {
        self.reverse_index.get(dependency)
    }

    /// Every reverse-index entry, in `DependencyId` order.
    pub fn reverse_index(&self) -> impl Iterator<Item = &ReverseIndexEntry> {
        self.reverse_index.values()
    }

    /// Per-computation producer attribution.
    pub fn attribution(&self, id: &ComputationId) -> Option<&ComputationCoverage> {
        self.attribution.get(id)
    }

    /// Every attribution record, in `ComputationId` order.
    pub fn attributions(&self) -> impl Iterator<Item = &ComputationCoverage> {
        self.attribution.values()
    }

    /// What does the absence of `kind` mean for this computation?
    ///
    /// `None` means the computation is not silent about that kind (it has
    /// events of it) or the computation is unknown.
    pub fn silence(&self, id: &ComputationId, kind: EventKind) -> Option<SilenceMeaning> {
        let node = self.computations.get(id)?;
        if node.count_of(kind) > 0 {
            return None;
        }
        Some(
            self.attribution
                .get(id)
                .map_or(SilenceMeaning::Unobserved, |c| c.interpret_silence(kind)),
        )
    }

    /// Structural problems found while replaying.
    pub fn defects(&self) -> &[GraphDefect] {
        &self.defects
    }

    /// `event_id`s that appeared more than once in the log.
    pub fn duplicate_event_ids(&self) -> &[Uuid] {
        &self.duplicate_event_ids
    }

    /// Is this replay reproducible from any physical log order?
    ///
    /// False when a producer emitted the same `event_id` twice with
    /// different content. A false here means the derived state must not
    /// be presented as if it were sound.
    pub fn is_deterministic(&self) -> bool {
        self.deterministic
    }

    /// Number of events replayed.
    pub fn event_count(&self) -> usize {
        self.event_count
    }

    /// Does every event with a `computation_id` fall into exactly one
    /// accounting bucket on its node?
    pub fn accounting_is_balanced(&self) -> bool {
        self.computations
            .values()
            .all(|n| n.accounting.is_balanced())
    }

    // ------------------------------------------------------- internals

    fn build_nodes(&mut self, events: &[IrEvent], artifact_paths: &[ArtifactAt]) {
        // Per-computation dependency-id → (index into `node.dependencies`,
        // the event that produced the retained edge).
        let mut edge_index: BTreeMap<ComputationId, BTreeMap<DependencyId, (usize, Uuid)>> =
            BTreeMap::new();
        // Per-computation set of paths already written.
        let mut written: BTreeMap<ComputationId, BTreeSet<PathBuf>> = BTreeMap::new();
        let mut contributors: BTreeMap<ComputationId, BTreeSet<ProducerKey>> = BTreeMap::new();
        let mut defects: Vec<GraphDefect> = Vec::new();

        for (position, event) in events.iter().enumerate() {
            let Some(raw_id) = event.computation_id.clone() else {
                if EDGE_BEARING_KINDS.contains(&event.kind) {
                    defects.push(GraphDefect::OrphanEdgeEvent {
                        event_id: event.event_id,
                        kind: event.kind,
                        producer: event.producer.clone(),
                    });
                }
                continue;
            };
            let comp = ComputationId(raw_id);
            contributors
                .entry(comp.clone())
                .or_default()
                .insert(ProducerKey::of_event(event));

            let outcome = classify(
                event,
                position,
                written.entry(comp.clone()).or_default(),
                artifact_paths,
                &comp,
            );

            let node = self
                .computations
                .entry(comp.clone())
                .or_insert_with(|| ComputationNode::new(comp.clone()));
            node.accounting.total += 1;
            *node
                .kind_counts
                .entry(event.kind.as_wire_str().to_string())
                .or_default() += 1;

            match outcome {
                Outcome::Edge(dep) => {
                    node.accounting.edge_events += 1;
                    let index = edge_index.entry(comp.clone()).or_default();
                    let id = dep.id();
                    match index.get(&id) {
                        None => {
                            index.insert(id, (node.dependencies.len(), event.event_id));
                            node.dependencies.push(*dep);
                        }
                        Some(&(existing, origin)) => {
                            let first = &node.dependencies[existing];
                            if first.fingerprint != dep.fingerprint
                                || first.trust_class != dep.trust_class
                            {
                                node.conflicts.push(EdgeConflict {
                                    dependency: id,
                                    first_event_id: origin,
                                    first_fingerprint: first.fingerprint.clone(),
                                    conflicting_event_id: event.event_id,
                                    conflicting_fingerprint: dep.fingerprint,
                                });
                            }
                        }
                    }
                }
                Outcome::Output(output) => {
                    node.accounting.outputs += 1;
                    node.outputs.push(output);
                }
                Outcome::Artifact(artifact, defect) => {
                    node.accounting.artifacts += 1;
                    node.artifacts.push(artifact);
                    if let Some(d) = defect {
                        defects.push(d);
                    }
                }
                Outcome::Obligation(obligation) => {
                    node.accounting.obligations += 1;
                    node.obligations.push(obligation);
                }
                Outcome::Excluded(excluded) => {
                    node.accounting.excluded += 1;
                    node.excluded.push(excluded);
                }
                Outcome::Unmodeled => {
                    node.accounting.unmodeled += 1;
                    *node
                        .unmodeled
                        .entry(event.kind.as_wire_str().to_string())
                        .or_default() += 1;
                }
            }
        }

        for (comp, node) in &mut self.computations {
            node.contributors = contributors
                .remove(comp)
                .map(|s| s.into_iter().collect())
                .unwrap_or_default();
        }
        self.defects.extend(defects);
    }

    fn build_attribution(&mut self, events: &[IrEvent], registry: &CoverageRegistry) {
        for comp in self.computations.keys() {
            let lookup = registry.coverage_for_computation(events, &comp.0);
            let mut partial_notes: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for entry in &lookup.entries {
                let key = ProducerKey::new(entry.producer.clone(), entry.version.clone());
                if let Some(manifest) = registry.manifest(&key) {
                    for (kind, note) in &manifest.partial {
                        partial_notes
                            .entry(kind.clone())
                            .or_default()
                            .push(note.clone());
                    }
                }
            }
            for notes in partial_notes.values_mut() {
                notes.sort();
                notes.dedup();
            }
            self.attribution.insert(
                comp.clone(),
                ComputationCoverage {
                    computation_id: comp.clone(),
                    entries: lookup.entries,
                    unregistered: lookup.unregistered,
                    partial_notes,
                },
            );
        }
    }

    fn build_reverse_index(&mut self) {
        let mut proven: BTreeMap<DependencyId, (String, BTreeSet<ComputationId>)> = BTreeMap::new();
        let mut unproven: BTreeMap<DependencyId, (String, BTreeSet<ComputationId>)> =
            BTreeMap::new();

        for node in self.computations.values() {
            for dep in &node.dependencies {
                proven
                    .entry(dep.id())
                    .or_insert_with(|| (dep.scheme.clone(), BTreeSet::new()))
                    .1
                    .insert(node.computation_id.clone());
            }
            for excluded in &node.excluded {
                if !excluded.reason.is_unproven_dependency() {
                    continue;
                }
                let Some(key) = &excluded.key else { continue };
                let id = DependencyId(key.clone());
                let scheme = key.split_once("://").map_or("", |(s, _)| s).to_string();
                unproven
                    .entry(id)
                    .or_insert_with(|| (scheme, BTreeSet::new()))
                    .1
                    .insert(node.computation_id.clone());
            }
        }

        let mut ids: BTreeSet<DependencyId> = proven.keys().cloned().collect();
        ids.extend(unproven.keys().cloned());

        let mut index = BTreeMap::new();
        for id in ids {
            let (scheme, consuming) = proven
                .get(&id)
                .cloned()
                .unwrap_or_else(|| (String::new(), BTreeSet::new()));
            let (unproven_scheme, unproven_comps) = unproven
                .get(&id)
                .cloned()
                .unwrap_or_else(|| (String::new(), BTreeSet::new()));
            let scheme = if scheme.is_empty() {
                unproven_scheme
            } else {
                scheme
            };
            let consumers = artifacts_of(&self.computations, &consuming);
            let unproven_consumers = artifacts_of(&self.computations, &unproven_comps);
            index.insert(
                id.clone(),
                ReverseIndexEntry {
                    dependency: id,
                    scheme,
                    consuming_computations: consuming.into_iter().collect(),
                    consumers,
                    unproven_computations: unproven_comps.into_iter().collect(),
                    unproven_consumers,
                },
            );
        }
        self.reverse_index = index;
    }
}

fn artifacts_of(
    computations: &BTreeMap<ComputationId, ComputationNode>,
    comps: &BTreeSet<ComputationId>,
) -> Vec<ArtifactId> {
    let mut out: BTreeSet<ArtifactId> = BTreeSet::new();
    for comp in comps {
        if let Some(node) = computations.get(comp) {
            out.extend(node.artifacts.iter().cloned());
        }
    }
    out.into_iter().collect()
}

// ---------------------------------------------------------- derivation

/// An `artifact.produced` observation located in canonical order, used to
/// link a later `file://` read back to the artifact that produced it
/// (invariant #9).
#[derive(Debug)]
struct ArtifactAt {
    position: usize,
    path: PathBuf,
    artifact: ArtifactId,
    computation: Option<String>,
}

fn collect_artifact_paths(events: &[IrEvent]) -> Vec<ArtifactAt> {
    let mut out = Vec::new();
    for (position, event) in events.iter().enumerate() {
        if event.kind != EventKind::ArtifactProduced {
            continue;
        }
        let (Some(id), Some(path)) = (
            event.payload.get("artifact_id").and_then(|v| v.as_str()),
            event.payload.get("path").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        out.push(ArtifactAt {
            position,
            path: PathBuf::from(path),
            artifact: ArtifactId(id.to_string()),
            computation: event.computation_id.clone(),
        });
    }
    out
}

/// The artifact (from a *different* computation) that most recently
/// produced `path` strictly before `position`, if any.
fn producing_artifact(
    artifacts: &[ArtifactAt],
    path: &Path,
    position: usize,
    comp: &ComputationId,
) -> Option<ArtifactId> {
    artifacts
        .iter()
        .filter(|a| {
            a.position < position
                && a.path.as_path() == path
                && a.computation.as_deref() != Some(&comp.0)
        })
        .next_back()
        .map(|a| a.artifact.clone())
}

enum Outcome {
    Edge(Box<Dependency>),
    Output(OutputRecord),
    Artifact(ArtifactId, Option<GraphDefect>),
    Obligation(CoverageObligation),
    Excluded(ExcludedEdge),
    Unmodeled,
}

fn classify(
    event: &IrEvent,
    position: usize,
    written: &mut BTreeSet<PathBuf>,
    artifacts: &[ArtifactAt],
    comp: &ComputationId,
) -> Outcome {
    match event.kind {
        EventKind::FsRead => classify_read(event, position, written, artifacts, comp),
        EventKind::FsWrite => classify_write(event, written),
        EventKind::ProbeChecked => classify_probe(event),
        EventKind::ArtifactProduced => classify_artifact(event),
        EventKind::ToolInvoked => classify_tool(event),
        _ => Outcome::Unmodeled,
    }
}

fn classify_read(
    event: &IrEvent,
    position: usize,
    written: &BTreeSet<PathBuf>,
    artifacts: &[ArtifactAt],
    comp: &ComputationId,
) -> Outcome {
    // `freshdag_core::ir::DecodeError` is not re-exported from
    // `freshdag_core::ir`, so it cannot be matched by name here. Its
    // `Display` is sufficient — the store only needs to record *that*
    // the payload was unusable, verbatim, for a human to chase.
    let read = match event.decode_payload() {
        Ok(TypedPayload::FsRead(r)) => r,
        Ok(_) => {
            return Outcome::Excluded(ExcludedEdge {
                event_id: event.event_id,
                kind: event.kind,
                key: None,
                reason: ExclusionReason::MalformedPayload(
                    "fs.read payload did not decode as FsRead".to_string(),
                ),
            })
        }
        Err(e) => {
            return Outcome::Excluded(ExcludedEdge {
                event_id: event.event_id,
                kind: event.kind,
                key: event
                    .payload
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(|p| format!("file://{p}")),
                reason: ExclusionReason::MalformedPayload(e.to_string()),
            })
        }
    };
    let key = format!("file://{}", read.path.to_string_lossy());

    if written.contains(&read.path) {
        return Outcome::Excluded(ExcludedEdge {
            event_id: event.event_id,
            kind: event.kind,
            key: Some(key),
            reason: ExclusionReason::ReadAfterOwnWrite,
        });
    }
    if read.impure {
        return Outcome::Excluded(ExcludedEdge {
            event_id: event.event_id,
            kind: event.kind,
            key: Some(key),
            reason: ExclusionReason::Impure,
        });
    }
    let Some(hash) = read.hash.as_ref() else {
        return Outcome::Excluded(ExcludedEdge {
            event_id: event.event_id,
            kind: event.kind,
            key: Some(key),
            reason: ExclusionReason::NoFingerprint,
        });
    };

    let mut dep = freshdag_core::dependency::exact_file(
        &read.path.to_string_lossy(),
        hash,
        observed_at(event.ts),
    );
    dep.produced_by = producing_artifact(artifacts, &read.path, position, comp);
    Outcome::Edge(Box::new(dep))
}

fn classify_write(event: &IrEvent, written: &mut BTreeSet<PathBuf>) -> Outcome {
    match event.decode_payload() {
        Ok(TypedPayload::FsWrite(w)) => {
            written.insert(w.path.clone());
            Outcome::Output(OutputRecord {
                event_id: event.event_id,
                path: w.path,
                hash: w.hash.map(|h| h.to_string()),
            })
        }
        _ => {
            // A write we cannot decode still tells us the computation
            // wrote *something*; record the path if it is recoverable so
            // read-after-own-write does not silently regress.
            if let Some(path) = event.payload.get("path").and_then(|v| v.as_str()) {
                written.insert(PathBuf::from(path));
            }
            Outcome::Excluded(ExcludedEdge {
                event_id: event.event_id,
                kind: event.kind,
                key: event
                    .payload
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(|p| format!("file://{p}")),
                reason: ExclusionReason::MalformedPayload(
                    "fs.write payload did not decode as FsWrite".to_string(),
                ),
            })
        }
    }
}

fn classify_probe(event: &IrEvent) -> Outcome {
    let scheme = event.payload.get("scheme").and_then(|v| v.as_str());
    let raw_key = event.payload.get("key").and_then(|v| v.as_str());
    let (Some(scheme), Some(raw_key)) = (scheme, raw_key) else {
        return Outcome::Excluded(ExcludedEdge {
            event_id: event.event_id,
            kind: event.kind,
            key: None,
            reason: ExclusionReason::MalformedPayload(
                "probe.checked payload lacks `scheme` or `key`".to_string(),
            ),
        });
    };
    let key = DependencyId::from_scheme_key(scheme, raw_key).0;

    let trust_class = match event.payload.get("trust_class").and_then(|v| v.as_str()) {
        Some("exact") => TrustClass::Exact,
        Some("versioned") => TrustClass::Versioned,
        Some("heuristic") => TrustClass::Heuristic,
        Some("volatile") => TrustClass::Volatile,
        other => {
            return Outcome::Excluded(ExcludedEdge {
                event_id: event.event_id,
                kind: event.kind,
                key: Some(key),
                reason: ExclusionReason::MalformedPayload(format!(
                    "probe.checked trust_class is {other:?}"
                )),
            })
        }
    };

    let Some(raw_fp) = event
        .payload
        .get("observed_fingerprint")
        .and_then(|v| v.as_str())
    else {
        return Outcome::Excluded(ExcludedEdge {
            event_id: event.event_id,
            kind: event.kind,
            key: Some(key),
            reason: ExclusionReason::NoFingerprint,
        });
    };
    let fingerprint: Fingerprint = match raw_fp.parse() {
        Ok(f) => f,
        Err(e) => {
            return Outcome::Excluded(ExcludedEdge {
                event_id: event.event_id,
                kind: event.kind,
                key: Some(key),
                reason: ExclusionReason::MalformedPayload(format!(
                    "unparseable observed_fingerprint: {e}"
                )),
            })
        }
    };

    let ttl_seconds = event.payload.get("ttl_seconds").and_then(|v| v.as_u64());
    if matches!(trust_class, TrustClass::Volatile) && ttl_seconds.is_none() {
        return Outcome::Excluded(ExcludedEdge {
            event_id: event.event_id,
            kind: event.kind,
            key: Some(key),
            reason: ExclusionReason::NakedVolatile,
        });
    }

    Outcome::Edge(Box::new(Dependency {
        key,
        scheme: scheme.to_string(),
        trust_class,
        fingerprint,
        observed_at: observed_at(event.ts),
        produced_by: None,
        ttl_seconds,
    }))
}

fn classify_artifact(event: &IrEvent) -> Outcome {
    let Some(id) = event.payload.get("artifact_id").and_then(|v| v.as_str()) else {
        return Outcome::Excluded(ExcludedEdge {
            event_id: event.event_id,
            kind: event.kind,
            key: None,
            reason: ExclusionReason::MalformedPayload(
                "artifact.produced payload lacks `artifact_id`".to_string(),
            ),
        });
    };
    let declared = event.payload.get("produced_by").and_then(|v| v.as_str());
    let defect = match declared {
        Some(d) if Some(d) != event.computation_id.as_deref() => {
            Some(GraphDefect::ArtifactProducerMismatch {
                event_id: event.event_id,
                envelope: event.computation_id.clone(),
                payload: d.to_string(),
            })
        }
        _ => None,
    };
    Outcome::Artifact(ArtifactId(id.to_string()), defect)
}

fn classify_tool(event: &IrEvent) -> Outcome {
    // Read `tool_kind` off the raw payload rather than the typed
    // decode: an obligation must survive a payload whose other fields
    // are malformed. This mirrors
    // `Certificate::check_coverage_deficit` in freshdag-core.
    let tool_kind = event.payload.get("tool_kind").and_then(|v| v.as_str());
    match tool_kind {
        Some(kind @ ("bash" | "task")) => Outcome::Obligation(CoverageObligation {
            event_id: event.event_id,
            producer: event.producer.clone(),
            tool_name: event
                .payload
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            tool_kind: kind.to_string(),
        }),
        _ => Outcome::Unmodeled,
    }
}

/// Dependencies record `observed_at` in UTC so the serialized graph does
/// not vary with the offset spelling a producer happened to use. The
/// instant is unchanged.
fn observed_at(ts: OffsetDateTime) -> OffsetDateTime {
    ts.to_offset(time::UtcOffset::UTC)
}
