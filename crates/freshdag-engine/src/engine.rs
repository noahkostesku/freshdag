//! Validity evaluation: from an artifact id to a certificate.
//!
//! The dispatch order for one dependency edge, and why it is that order:
//!
//! 1. **The `volatile` TTL gate, first and positive.** If the trust
//!    class is `volatile` and the declared TTL is absent,
//!    unrepresentable, longer than `max_volatile_ttl`, measured from a
//!    future `observed_at`, or elapsed, the edge is `Unknown` with
//!    [`ReasonCode::TtlExpired`] and no probe is consulted. §7 requires
//!    this be a *positive* branch keyed on the trust class and evaluated
//!    before arbitration, so that registering an unrelated probe for the
//!    scheme cannot silently change the verdict. Every outcome here is a
//!    function of the recorded dependency alone.
//!
//!    **Passing the gate does not decide the edge** — §7 disambiguates
//!    "before probe arbitration" as scoping the gate, not the verdict.
//!    Steps 2-4 then run normally, and the verdict is resolved by
//!    [`volatile_within_ttl_verdict`].
//! 2. **Probe removal.** If the probe that last answered for this key is
//!    no longer registered, the edge is `Unknown` with
//!    [`ReasonCode::NoProbeAvailable`]. The engine does NOT fall through
//!    to another probe for the same scheme (probe-contract §Anti-thrash,
//!    "Probe removal").
//! 3. **Arbitration.** Highest priority wins; a tie fails loudly with a
//!    diagnostic and yields `NoProbeAvailable`.
//! 4. **Dispatch**, then anti-thrash folding, then the trust-class table
//!    from `ARCHITECTURE.md §7`.
//!
//! [`ReasonCode::TtlExpired`]: freshdag_core::dependency::ReasonCode::TtlExpired
//! [`ReasonCode::NoProbeAvailable`]: freshdag_core::dependency::ReasonCode::NoProbeAvailable

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use freshdag_core::artifact::{Artifact, ArtifactId};
use freshdag_core::certificate::{Certificate, Comparator, CoverageEntry, ProducedBy};
use freshdag_core::computation::ComputationId;
use freshdag_core::dependency::{
    Dependency, EdgeVerdict, ReasonCode, TrustClass, ValidityReason, ValidityStatus,
};
use freshdag_core::ir::{EventKind, EventKindPattern, Hash, IrEvent, ProducerRole};
use freshdag_core::probe::ProbeResult;
use freshdag_store::{CoverageRegistry, DerivedGraph};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::antithrash::{TrustLedger, TrustTransition};
use crate::clock::{Clock, SystemClock};
use crate::coverage;
use crate::error::EngineError;
use crate::registry::{NoProbe, ProbeIdentity, ProbeRegistry};
use crate::seal::{seal, CoverageAuthority, EdgeOutcome, SealInput};

/// The engine's producer identity in the execution IR.
pub const ENGINE_PRODUCER: &str = "freshdag-engine";
/// The engine's producer version.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Maximum length of a probe-supplied `detail` string, per
/// certificate-contract §The `detail` field ("SHOULD be under 512
/// bytes").
const MAX_DETAIL_BYTES: usize = 512;

/// Longest `ttl_seconds` a producer can declare and still have it
/// treated as evidence (ADR 0009, Amendment; `ARCHITECTURE.md §7`).
///
/// §7 licenses "volatile inside TTL → likely-valid" on the grounds that
/// the producer's declared lifetime is *present evidence*. That argument
/// holds only while the declaration is bounded: without a ceiling,
/// `ttl_seconds: 1000000000` (~31 years) behaves exactly like
/// `ttl_seconds: 3600`, and a producer buys unlimited freshness with a
/// large integer. The RFC 9111 analogy does not rescue it — HTTP's
/// `max-age` arrives from an origin over an authenticated channel and
/// §4.2.3 requires the cache validate a `Date` before computing age;
/// FreshDAG has neither.
///
/// 24 hours is the architect's conservative judgment call, not a derived
/// quantity: a `volatile` dependency is by definition one with no
/// trustworthy freshness signal, so a day is already generous. It is
/// injectable via [`EngineBuilder::max_volatile_ttl`] precisely so a
/// design partner with legitimately longer-lived volatile dependencies
/// can raise it deliberately rather than by editing this constant.
pub const DEFAULT_MAX_VOLATILE_TTL: std::time::Duration =
    std::time::Duration::from_secs(24 * 60 * 60);

/// How far into the future a recorded `observed_at` may sit before the
/// engine stops believing it (ADR 0009, Amendment).
///
/// A `probe.checked` dated 2099 satisfies `now > expires_at == false`
/// forever, so a future timestamp is an unbounded TTL wearing a
/// disguise. Some tolerance is required because the producing host and
/// the checking host are not the same machine and need not share a
/// clock.
///
/// One minute: NTP-disciplined hosts stay within milliseconds of each
/// other and even badly-configured ones rarely exceed seconds, so a
/// minute absorbs ordinary skew while leaving a decade-away timestamp a
/// hard failure. Deliberately not injectable — unlike a TTL ceiling,
/// there is no legitimate workload that needs a larger one, and a knob
/// here would be a knob for disabling the check.
pub const MAX_CLOCK_SKEW: std::time::Duration = std::time::Duration::from_secs(60);

/// What one `check` produced.
#[derive(Debug, Clone)]
pub struct CheckOutcome {
    /// The certificate. Immutable; re-checking produces a successor.
    pub certificate: Certificate,
    /// IR events the check itself generated — `probe.checked` for every
    /// probe dispatch and `diagnostic` for arbitration ties and trust
    /// demotions. The caller appends these to the log; certificates
    /// explain, the log schedules.
    pub events: Vec<IrEvent>,
}

/// Validity evaluation over a derived graph.
#[derive(Debug)]
pub struct Engine {
    events: Vec<IrEvent>,
    graph: DerivedGraph,
    registry: ProbeRegistry,
    clock: Arc<dyn Clock>,
    ledger: Mutex<TrustLedger>,
    max_volatile_ttl: std::time::Duration,
}

impl Engine {
    /// Start building an engine.
    #[must_use]
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// The derived graph this engine evaluates over.
    #[must_use]
    pub fn graph(&self) -> &DerivedGraph {
        &self.graph
    }

    /// The probe registry.
    #[must_use]
    pub fn registry(&self) -> &ProbeRegistry {
        &self.registry
    }

    /// Remove a probe registration, modelling uninstall.
    ///
    /// Dependencies this identity previously answered for become
    /// `Unknown` with `NoProbeAvailable` on their next check.
    pub fn deregister_probe(&mut self, identity: &ProbeIdentity) -> bool {
        self.registry.deregister(identity)
    }

    /// Evaluate an artifact's validity and emit a certificate.
    ///
    /// # Errors
    ///
    /// Every [`EngineError`] means no certificate is emitted. See that
    /// type: an evaluation outcome downgrades a status, a structural
    /// defect emits nothing.
    #[allow(clippy::too_many_lines)] // one linear procedure; splitting it hides the order
    pub fn check(&self, artifact_id: &ArtifactId) -> Result<CheckOutcome, EngineError> {
        // The store's own documentation: a non-deterministic replay
        // "must not be presented as if it were sound".
        if !self.graph.is_deterministic() {
            return Err(EngineError::NondeterministicReplay);
        }

        let computation = self.computation_for(artifact_id)?;
        let node =
            self.graph
                .computation(&computation)
                .ok_or_else(|| EngineError::UnknownArtifact {
                    artifact: artifact_id.clone(),
                })?;

        let recorded_events: Vec<IrEvent> = self
            .events
            .iter()
            .filter(|e| e.computation_id.as_deref() == Some(computation.0.as_str()))
            .cloned()
            .collect();

        // --- observation_coverage, assembled by the engine ------------
        let attribution = self.graph.attribution(&computation);
        let mut observation_coverage: Vec<CoverageEntry> =
            attribution.map(|a| a.entries.clone()).unwrap_or_default();
        let accounted_missing: BTreeSet<String> = attribution
            .map(|a| a.unregistered.iter().map(|k| k.producer.clone()).collect())
            .unwrap_or_default();
        observation_coverage.push(engine_coverage_entry());

        let now = self.clock.now();
        let mut emitted: Vec<IrEvent> = Vec::new();
        let mut sequence: u16 = 0;

        // --- per-edge evaluation --------------------------------------
        let mut ledger = self
            .ledger
            .lock()
            .map_err(|_| EngineError::InternalInconsistency {
                message: "trust ledger lock poisoned".to_string(),
            })?;
        let mut edges: Vec<EdgeOutcome> = Vec::with_capacity(node.dependencies.len());
        for dep in &node.dependencies {
            edges.push(self.evaluate_edge(
                &computation,
                dep,
                now,
                &mut ledger,
                &mut emitted,
                &mut sequence,
            ));
        }
        drop(ledger);

        // The store recorded that the same dependency was observed twice
        // within one computation with different fingerprints: the input
        // changed while the agent was reading it.
        //
        // The recorded fingerprint is therefore not a statement about
        // what the computation consumed — it is one of at least two, and
        // nothing in the log says which the agent actually used. So the
        // edge's verdict is forced to `Unknown` regardless of what the
        // probe just said: a probe confirming the *recorded* fingerprint
        // still matches proves nothing about the *other* observation.
        //
        // ADR 0009: an artifact with a mid-computation conflict can
        // never be `Valid`. Until W9.1 this surfaced only as a
        // `graph.edge_conflict` diagnostic, because no reason code
        // existed and adding one is a contract change — so the conflict
        // was visible in the log while the certificate reported `valid`.
        for conflict in &node.conflicts {
            emitted.push(self.diagnostic(
                &computation,
                now,
                &mut sequence,
                "graph.edge_conflict",
                serde_json::json!({
                    "dependency": conflict.dependency.to_string(),
                    "first_fingerprint": conflict.first_fingerprint.to_string(),
                    "conflicting_fingerprint": conflict.conflicting_fingerprint.to_string(),
                }),
            ));

            let key = conflict.dependency.to_string();
            let Some(edge) = edges.iter_mut().find(|e| e.dependency.key == key) else {
                // ADR 0009: an artifact with a mid-computation conflict
                // can never be `valid`. If the conflict names an edge
                // this evaluation does not have, that guarantee has no
                // way to attach — and a silent miss here is a silent
                // `valid`, because no reason is produced and `seal`'s
                // self-audit has nothing to disagree with.
                //
                // Sound today: `classify_probe` builds the key as
                // `id.0`, so it equals `Dependency::id().to_string()`.
                // One normalization change away from not being, which
                // is exactly why this refuses rather than shrugs.
                return Err(EngineError::InternalInconsistency {
                    message: format!(
                        "store recorded a conflict on `{key}`, which is not among the \
                         edges evaluated for this computation; the ADR 0009 guarantee \
                         that a conflicted artifact cannot be valid has nothing to attach to"
                    ),
                });
            };

            edge.verdict = EdgeVerdict::Unknown;
            // The store raises a conflict on a differing fingerprint OR
            // a differing trust class. Only the first is "the input
            // changed while the agent was reading it"; the second is the
            // same bytes classified two ways, and rendering
            // `observed=X then=X` at a user while telling them the
            // contents differed would be false.
            let detail = if conflict.first_fingerprint == conflict.conflicting_fingerprint {
                format!(
                    "same-fingerprint trust-class conflict on `{key}`; fingerprint={}",
                    conflict.first_fingerprint
                )
            } else {
                format!(
                    "observed={} then={}",
                    conflict.first_fingerprint, conflict.conflicting_fingerprint
                )
            };
            edge.reason = Some((ReasonCode::DependencyChangedDuringComputation, Some(detail)));
        }

        // --- artifact-scoped coverage reasons -------------------------
        let mut artifact_reasons: Vec<ValidityReason> = Vec::new();
        if !accounted_missing.is_empty() {
            let names: Vec<&str> = accounted_missing.iter().map(String::as_str).collect();
            artifact_reasons.push(ValidityReason {
                dependency_key: String::new(),
                reason: ReasonCode::ProducerMissingFromCoverage,
                detail: Some(format!("producer={}", names.join(","))),
            });
        }
        let mut deficit_details: Vec<String> = Vec::new();
        let effect_deficit = coverage::effect_deficit(&recorded_events, &observation_coverage);
        if !effect_deficit.is_empty() {
            deficit_details.push(coverage::effect_deficit_detail(&effect_deficit));
        }
        if !node.obligations.is_empty() && !coverage::has_fs_covered_observer(&observation_coverage)
        {
            let kinds: BTreeSet<String> = node
                .obligations
                .iter()
                .map(|o| o.tool_kind.clone())
                .collect();
            deficit_details.push(coverage::obligation_detail(&kinds));
        }
        if !deficit_details.is_empty() {
            artifact_reasons.push(ValidityReason {
                dependency_key: String::new(),
                reason: ReasonCode::CoverageDeficit,
                detail: Some(deficit_details.join("; ")),
            });
        }

        // --- provenance ------------------------------------------------
        let artifact = self.artifact_of(&computation, artifact_id)?;
        let produced_by = self.produced_by(&computation, &recorded_events, &observation_coverage);
        let comparator = self.comparator_of(&computation, artifact_id);

        let mut gate_events = recorded_events;
        gate_events.extend(emitted.iter().cloned());

        let certificate = seal(SealInput {
            artifact,
            produced_by,
            edges,
            artifact_reasons,
            coverage: observation_coverage,
            events: &gate_events,
            authority: CoverageAuthority::EngineAssembled,
            accounted_missing_producers: accounted_missing,
            checked_at: now,
            comparator,
        })?;

        Ok(CheckOutcome {
            certificate,
            events: emitted,
        })
    }

    // ------------------------------------------------------- internals

    fn computation_for(&self, artifact_id: &ArtifactId) -> Result<ComputationId, EngineError> {
        let claimants: Vec<ComputationId> = self
            .graph
            .computations()
            .filter(|n| n.artifacts.contains(artifact_id))
            .map(|n| n.computation_id.clone())
            .collect();
        match claimants.len() {
            0 => Err(EngineError::UnknownArtifact {
                artifact: artifact_id.clone(),
            }),
            1 => Ok(claimants.into_iter().next().expect("len == 1")),
            _ => Err(EngineError::AmbiguousArtifact {
                artifact: artifact_id.clone(),
                computations: claimants,
            }),
        }
    }

    #[allow(clippy::too_many_lines)] // one linear decision procedure; splitting it hides the order
    fn evaluate_edge(
        &self,
        computation: &ComputationId,
        dep: &Dependency,
        now: OffsetDateTime,
        ledger: &mut TrustLedger,
        emitted: &mut Vec<IrEvent>,
        sequence: &mut u16,
    ) -> EdgeOutcome {
        let unknown = |code: ReasonCode, detail: String| EdgeOutcome {
            dependency: dep.clone(),
            verdict: EdgeVerdict::Unknown,
            reason: Some((code, Some(detail))),
        };

        // 1. The TTL gate: positive, keyed on trust class, before probe
        //    removal and arbitration (ARCHITECTURE §7 step 1).
        //
        //    "Before probe arbitration" scopes *this gate*, not the
        //    verdict — §7 disambiguates that explicitly, because the
        //    first implementation read it the other way and stopped
        //    consulting probes on volatile edges at all. The gate
        //    short-circuits only *downward*: every outcome here is
        //    `Unknown`, and every one of them is a function of the
        //    recorded dependency alone, which is what removes the
        //    scheme-dependence the gate was added for. A dependency's
        //    verdict must not turn on which schemes happen to have
        //    probes installed.
        //
        //    A declared TTL is evidence only where it is *bounded* and
        //    its timestamp is *real* (ADR 0009, Amendment 1). Four
        //    guards: the declaration must exist, be representable, be
        //    within `max_volatile_ttl`, and be measured from an
        //    `observed_at` that is not in the future — then it must not
        //    have elapsed. The class is bounded, never discarded, so
        //    `volatile` stays usable for the dependencies (`time.now()`)
        //    that no probe can ever answer for.
        let volatile_within_ttl = matches!(dep.trust_class, TrustClass::Volatile);
        if volatile_within_ttl {
            let Some(secs) = dep.ttl_seconds else {
                return unknown(ReasonCode::TtlExpired, "ttl_seconds=absent".to_string());
            };
            // A TTL we cannot turn into an instant is not evidence that
            // the window is open; it is the absence of a usable
            // declaration. Invariant #7: treat it as an elapsed TTL.
            let Some(expires_at) = ttl_expiry(dep.observed_at, secs) else {
                return unknown(
                    ReasonCode::TtlExpired,
                    format!("ttl_seconds={secs} not-representable"),
                );
            };
            // A producer must not be able to buy freshness with a large
            // integer.
            let max = self.max_volatile_ttl.as_secs();
            if secs > max {
                return unknown(
                    ReasonCode::TtlExpired,
                    format!("ttl_seconds={secs} exceeds max_volatile_ttl={max}"),
                );
            }
            // A future `observed_at` is an unbounded TTL in disguise:
            // `now > expires_at` is false forever. TTL arithmetic must
            // not go negative-fresh.
            if now
                .checked_add(skew_tolerance())
                .is_some_and(|horizon| dep.observed_at > horizon)
            {
                return unknown(
                    ReasonCode::TtlExpired,
                    format!(
                        "observed_at-in-future skew_tolerance_seconds={}",
                        MAX_CLOCK_SKEW.as_secs()
                    ),
                );
            }
            if now > expires_at {
                return unknown(ReasonCode::TtlExpired, format!("ttl_seconds={secs}"));
            }
            // Passing the gate does NOT decide the edge (§7 step 2). It
            // establishes only that the declared lifetime is bounded,
            // real, and open. Fall through to removal, arbitration and
            // dispatch like every other class.
        }

        // 2. Probe removal. Never fall through to another probe.
        if let Some(previous) = ledger.last_probe_for(&dep.key) {
            if !self.registry.contains(previous) {
                let reason = NoProbe::ProbeRemoved {
                    previous: previous.clone(),
                };
                emitted.push(self.diagnostic(
                    computation,
                    now,
                    sequence,
                    "probe.removed",
                    serde_json::json!({
                        "dependency_key": dep.key,
                        "previous_probe": previous.as_str(),
                    }),
                ));
                if volatile_within_ttl {
                    return volatile_within_ttl_verdict(dep, None);
                }
                return unknown(ReasonCode::NoProbeAvailable, reason.detail());
            }
        }

        // 3. Arbitration.
        let selected = match self.registry.select(&dep.scheme, &dep.key) {
            Ok(selected) => selected,
            Err(no_probe) => {
                if let NoProbe::PriorityTie {
                    scheme,
                    priority,
                    candidates,
                } = &no_probe
                {
                    emitted.push(self.diagnostic(
                        computation,
                        now,
                        sequence,
                        "probe.arbitration_tie",
                        serde_json::json!({
                            "scheme": scheme,
                            "priority": priority,
                            "candidates": candidates
                                .iter()
                                .map(ProbeIdentity::as_str)
                                .collect::<Vec<_>>(),
                        }),
                    ));
                }
                if volatile_within_ttl {
                    return volatile_within_ttl_verdict(dep, None);
                }
                return unknown(ReasonCode::NoProbeAvailable, no_probe.detail());
            }
        };

        // 4. Dispatch.
        let ttl_hint = dep.ttl_seconds.map(std::time::Duration::from_secs);
        let result = selected.probe.check(&dep.key, &dep.fingerprint, ttl_hint);

        // 4a. Bookkeeping, identical for every trust class. What the
        //     probe said reaches the log and the ledger regardless of
        //     how the verdict is then computed.
        //
        //     The `probe.checked` payload carries *inputs only* (ADR
        //     0007, Amendment P1): `trust_class` is the class the store
        //     recorded on the dependency, `observed_trust_class` is what
        //     the probe saw this time. Neither is derived. Writing the
        //     ledger's *adopted* class here — which this code used to
        //     do, with a comment claiming the opposite — is wrong twice
        //     over. It is a silent escalation across a process boundary:
        //     after N=2 the adopted class is the escalated one, so a
        //     replay of the log yields `Valid` where the store recorded
        //     `Heuristic` (invariants #7 and #8). And because ADR 0007
        //     makes the ledger a *fold over* `probe.checked`, writing
        //     the fold's own output into the events being folded makes
        //     the projection non-idempotent — derived state whose value
        //     depends on how many times it has been derived is not
        //     reconstructable in the sense invariant #5 means.
        //
        //     Adoption stays auditable: it is reconstructable from the
        //     `observed_trust_class` sequence, which is what the
        //     anti-thrash protocol was always defined over, and the
        //     transitions surface as `probe.trust_escalated` and
        //     `probe.trust_demoted` diagnostics.
        emitted.push(self.probe_checked_for(computation, now, sequence, dep, &result));
        let transition = match &result {
            ProbeResult::Match {
                observed_trust_class,
                ..
            } => Some(ledger.observe(
                &dep.key,
                &selected.identity,
                dep.trust_class,
                *observed_trust_class,
                now,
            )),
            // A probe that ran owns the key for probe-removal purposes,
            // whatever it said (probe-contract §Anti-thrash, "Probe
            // removal" — never fall through to another probe). Only
            // `Match` carries an observed class for the ledger to fold.
            ProbeResult::Drift { .. } | ProbeResult::Unknown { .. } => {
                ledger.note_probe(&dep.key, &selected.identity);
                None
            }
        };
        if let Some(TrustTransition::Escalated { from, to }) = transition {
            emitted.push(self.diagnostic(
                computation,
                now,
                sequence,
                "probe.trust_escalated",
                serde_json::json!({
                    "dependency_key": dep.key,
                    "probe_identity": selected.identity.as_str(),
                    "from_trust_class": trust_wire(from),
                    "to_trust_class": trust_wire(to),
                }),
            ));
        }
        if let Some(TrustTransition::Demoted { from, to }) = transition {
            emitted.push(self.trust_demoted(
                computation,
                now,
                sequence,
                dep,
                &selected.identity,
                from,
                to,
            ));
        }

        // 4b. Verdict.
        if volatile_within_ttl {
            return volatile_within_ttl_verdict(dep, Some(&result));
        }

        match result {
            ProbeResult::Match {
                observed_fp,
                observed_trust_class,
            } => {
                if let Some(TrustTransition::Demoted { from, to }) = transition {
                    return unknown(
                        ReasonCode::ProbeTrustDemoted,
                        format!("from={} to={}", trust_wire(from), trust_wire(to)),
                    );
                }
                EdgeOutcome {
                    dependency: dep.clone(),
                    verdict: EdgeVerdict::Match {
                        // The RECORDED class, never the observed one.
                        // Escalations live in the ledger until a
                        // persistence design lands; letting one raise a
                        // certificate's status here would be exactly the
                        // silent promotion the protocol exists to stop.
                        recorded_trust_class: dep.trust_class,
                        observed_trust_class,
                        observed_fp,
                    },
                    reason: caps_reason(dep.trust_class),
                }
            }
            ProbeResult::Drift { .. } => EdgeOutcome {
                dependency: dep.clone(),
                verdict: EdgeVerdict::Drift,
                reason: Some((ReasonCode::Drift, None)),
            },
            // ADR 0010: `Unknown` maps to `ProbeUnknown` regardless of
            // `retryable`, and never demotes.
            //
            // `retryable` answers a scheduling question; demotion
            // answers an evidentiary one. Treating the first as the
            // second told a user who deleted a dependency that the
            // reason was `probe-trust-demoted` — a true detail under a
            // false code, since the trust class of nothing was demoted
            // and a file is gone. `FileProbe` returns `Unknown {
            // retryable: false }` for a missing file, a malformed
            // recorded fingerprint, and a fingerprint kind it cannot
            // verify; none of those observed a weaker validator. The one
            // case that does — an endpoint that stopped serving `ETag` —
            // arrives as `Match`/`Drift` with a lower
            // `observed_trust_class` and is folded by
            // `TrustLedger::observe` above.
            //
            // `retryable` still reaches the log, where the scheduler
            // wants it. Certificates explain; the log schedules.
            ProbeResult::Unknown { reason, .. } => {
                unknown(ReasonCode::ProbeUnknown, sanitize_detail(&reason))
            }
        }
    }

    fn artifact_of(
        &self,
        computation: &ComputationId,
        artifact_id: &ArtifactId,
    ) -> Result<Artifact, EngineError> {
        let event = self
            .artifact_event(computation, artifact_id)
            .ok_or_else(|| EngineError::UnknownArtifact {
                artifact: artifact_id.clone(),
            })?;
        let content_hash = event
            .payload
            .get("content_hash")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Hash::from_str(s).ok())
            .ok_or(EngineError::MalformedArtifactEvent {
                artifact: artifact_id.clone(),
                field: "content_hash",
            })?;
        let kind = event
            .payload
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or(EngineError::MalformedArtifactEvent {
                artifact: artifact_id.clone(),
                field: "kind",
            })?;
        Ok(Artifact {
            id: artifact_id.clone(),
            path: event
                .payload
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            kind: kind.to_string(),
            content_hash,
            size: event
                .payload
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        })
    }

    fn artifact_event(
        &self,
        computation: &ComputationId,
        artifact_id: &ArtifactId,
    ) -> Option<&IrEvent> {
        self.events.iter().find(|e| {
            e.kind == EventKind::ArtifactProduced
                && e.computation_id.as_deref() == Some(computation.0.as_str())
                && e.payload
                    .get("artifact_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(artifact_id.0.as_str())
        })
    }

    fn comparator_of(
        &self,
        computation: &ComputationId,
        artifact_id: &ArtifactId,
    ) -> Option<Comparator> {
        self.artifact_event(computation, artifact_id)
            .and_then(|e| e.payload.get("comparator"))
            .and_then(serde_json::Value::as_str)
            .map(|name| Comparator {
                name: name.to_string(),
                config: None,
            })
    }

    #[allow(clippy::unused_self)]
    fn produced_by(
        &self,
        computation: &ComputationId,
        events: &[IrEvent],
        coverage: &[CoverageEntry],
    ) -> ProducedBy {
        let started_event = events
            .iter()
            .find(|e| e.kind == EventKind::ComputationStarted);
        let ended_event = events
            .iter()
            .find(|e| e.kind == EventKind::ComputationEnded);

        let started = started_event
            .map(|e| e.ts)
            .or_else(|| events.iter().map(|e| e.ts).min())
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let ended = ended_event
            .map(|e| e.ts)
            .or_else(|| events.iter().map(|e| e.ts).max())
            .unwrap_or(started);

        // Identity of the adapter, in the contract's
        // `freshdag-adapter-<name>/<version>` form. Prefer the producer
        // of `computation.started`; otherwise the first adapter-role
        // producer in coverage order. Both are deterministic.
        let adapter = started_event
            .map(|e| format!("{}/{}", e.producer, e.producer_version))
            .or_else(|| {
                coverage
                    .iter()
                    .find(|c| matches!(c.role, ProducerRole::Adapter))
                    .map(|c| format!("{}/{}", c.producer, c.version))
            })
            .unwrap_or_else(|| format!("{ENGINE_PRODUCER}/{ENGINE_VERSION}"));

        ProducedBy {
            computation_id: computation.clone(),
            recipe: started_event
                .and_then(|e| e.payload.get("recipe_id"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            recipe_hash: started_event
                .and_then(|e| e.payload.get("recipe_hash"))
                .and_then(serde_json::Value::as_str)
                .and_then(|s| Hash::from_str(s).ok()),
            adapter,
            started: started.to_offset(time::UtcOffset::UTC),
            ended: ended.to_offset(time::UtcOffset::UTC),
        }
    }

    // ------------------------------------------------- event emission

    /// The `probe.checked` event for a probe result, whatever it said.
    ///
    /// One site, so the three results cannot drift apart in what they
    /// record — `retryable` is REQUIRED on `unknown` and forbidden
    /// elsewhere (execution-ir.md), and `trust_class` is the recorded
    /// class on all three (ADR 0007, Amendment P1).
    fn probe_checked_for(
        &self,
        computation: &ComputationId,
        now: OffsetDateTime,
        sequence: &mut u16,
        dep: &Dependency,
        result: &ProbeResult,
    ) -> IrEvent {
        match result {
            ProbeResult::Match {
                observed_fp,
                observed_trust_class,
            } => self.probe_checked(
                computation,
                now,
                sequence,
                dep,
                "match",
                Some(&observed_fp.to_string()),
                dep.trust_class,
                *observed_trust_class,
                None,
            ),
            ProbeResult::Drift {
                observed_fp,
                observed_trust_class,
            } => self.probe_checked(
                computation,
                now,
                sequence,
                dep,
                "drift",
                Some(&observed_fp.to_string()),
                dep.trust_class,
                *observed_trust_class,
                None,
            ),
            ProbeResult::Unknown { retryable, .. } => self.probe_checked(
                computation,
                now,
                sequence,
                dep,
                "unknown",
                None,
                dep.trust_class,
                dep.trust_class,
                Some(*retryable),
            ),
        }
    }

    #[allow(clippy::too_many_arguments, clippy::unused_self)]
    fn probe_checked(
        &self,
        computation: &ComputationId,
        now: OffsetDateTime,
        sequence: &mut u16,
        dep: &Dependency,
        result: &str,
        observed_fp: Option<&str>,
        trust_class: TrustClass,
        observed_trust_class: TrustClass,
        retryable: Option<bool>,
    ) -> IrEvent {
        let mut payload = serde_json::Map::new();
        payload.insert("scheme".into(), dep.scheme.clone().into());
        payload.insert("key".into(), dep.key.clone().into());
        if let Some(fp) = observed_fp {
            payload.insert("observed_fingerprint".into(), fp.into());
        }
        payload.insert("trust_class".into(), trust_wire(trust_class).into());
        payload.insert(
            "observed_trust_class".into(),
            trust_wire(observed_trust_class).into(),
        );
        payload.insert("result".into(), result.into());
        if let Some(ttl) = dep.ttl_seconds {
            payload.insert("ttl_seconds".into(), ttl.into());
        }
        // execution-ir.md: REQUIRED when result is "unknown", absent
        // otherwise. Certificates explain; the log schedules.
        match retryable {
            Some(value) => {
                debug_assert_eq!(result, "unknown", "retryable is unknown-only");
                payload.insert("retryable".into(), value.into());
            }
            None => debug_assert_ne!(result, "unknown", "unknown result must carry retryable"),
        }
        self.event(
            computation,
            now,
            sequence,
            EventKind::ProbeChecked,
            serde_json::Value::Object(payload),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn trust_demoted(
        &self,
        computation: &ComputationId,
        now: OffsetDateTime,
        sequence: &mut u16,
        dep: &Dependency,
        identity: &ProbeIdentity,
        from: TrustClass,
        to: TrustClass,
    ) -> IrEvent {
        self.diagnostic(
            computation,
            now,
            sequence,
            "probe.trust_demoted",
            serde_json::json!({
                "dependency_key": dep.key,
                "probe_identity": identity.as_str(),
                "from_trust_class": trust_wire(from),
                "to_trust_class": trust_wire(to),
            }),
        )
    }

    #[allow(clippy::unused_self)]
    fn diagnostic(
        &self,
        computation: &ComputationId,
        now: OffsetDateTime,
        sequence: &mut u16,
        message: &str,
        mut extra: serde_json::Value,
    ) -> IrEvent {
        if let Some(map) = extra.as_object_mut() {
            map.insert("message".into(), message.into());
        }
        self.event(computation, now, sequence, EventKind::Diagnostic, extra)
    }

    #[allow(clippy::unused_self)]
    fn event(
        &self,
        computation: &ComputationId,
        now: OffsetDateTime,
        sequence: &mut u16,
        kind: EventKind,
        payload: serde_json::Value,
    ) -> IrEvent {
        let event_id = engine_event_id(now, *sequence);
        *sequence = sequence.saturating_add(1);
        IrEvent {
            event_id,
            producer: ENGINE_PRODUCER.to_string(),
            producer_version: ENGINE_VERSION.to_string(),
            session_id: format!("check:{computation}"),
            computation_id: Some(computation.0.clone()),
            parent_id: None,
            causal_inputs: None,
            ts: now,
            kind,
            payload,
        }
    }
}

/// The engine's own coverage entry.
///
/// The engine dispatches probes, so it contributed evidence and belongs
/// in `observation_coverage` (certificate contract: "lists every
/// producer that contributed"). Role `Probe`, which by construction
/// cannot discharge a `bash`/`task` observation obligation.
fn engine_coverage_entry() -> CoverageEntry {
    CoverageEntry {
        producer: ENGINE_PRODUCER.to_string(),
        version: ENGINE_VERSION.to_string(),
        role: ProducerRole::Probe,
        emits: vec![
            EventKindPattern::new("probe.checked"),
            EventKindPattern::new("diagnostic"),
        ],
        // Empty, and deliberately so: the engine emits a `probe.checked`
        // for every probe it dispatches and a `diagnostic` for every
        // fault it handles, so neither kind is partial *within its
        // scope*. What the engine cannot see — external state with no
        // registered probe — is not a partial declaration but the
        // `known_limitations` entry below, because it is a limit on the
        // evidence available, not on the fidelity of what it emits.
        partial: BTreeMap::new(),
        known_limitations: vec!["sees external state only through registered probes".to_string()],
    }
}

/// Deterministic, well-formed UUIDv7 for an engine-emitted event.
///
/// Derived from the injected clock plus a per-check sequence number
/// rather than randomness, so two runs over the same inputs produce the
/// same log. `.claude/rules/testing.md` forbids nondeterministic tests,
/// and a producer whose ids differ per run cannot be diffed.
fn engine_event_id(at: OffsetDateTime, sequence: u16) -> Uuid {
    let millis = u64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or(0);
    let ms = millis & 0x0000_FFFF_FFFF_FFFF;
    let mut bytes = [0u8; 16];
    bytes[0..6].copy_from_slice(&ms.to_be_bytes()[2..8]);
    // version 7 in the high nibble, sequence in the remaining 12 bits.
    bytes[6] = 0x70 | u8::try_from((sequence >> 8) & 0x0F).unwrap_or(0);
    bytes[7] = u8::try_from(sequence & 0x00FF).unwrap_or(0);
    // RFC 4122 variant.
    bytes[8] = 0x80;
    Uuid::from_bytes(bytes)
}

/// The instant a `volatile` dependency's declared TTL elapses, if that
/// instant is representable.
///
/// `ttl_seconds` is an unvalidated `u64` read off the log, and
/// `OffsetDateTime + Duration` is `checked_add(..).expect(..)` — it
/// panics on overflow. Two entirely plausible inputs reach that panic:
/// `u64::MAX`, the obvious spelling of "never expires", and a
/// seconds/nanoseconds unit confusion such as `300000000000`. A producer
/// is not a trusted party; a malformed number in the log must degrade a
/// verdict, never abort the process.
///
/// Returns `None` when the TTL exceeds `i64` seconds or when adding it
/// to `observed_at` leaves `OffsetDateTime`'s representable range. The
/// caller treats `None` as an elapsed TTL: an expiry we cannot compute
/// is indistinguishable from no meaningful TTL having been recorded, and
/// invariant #7 says what we cannot prove is not fresh.
fn ttl_expiry(observed_at: OffsetDateTime, ttl_seconds: u64) -> Option<OffsetDateTime> {
    let seconds = i64::try_from(ttl_seconds).ok()?;
    observed_at.checked_add(time::Duration::seconds(seconds))
}

/// [`MAX_CLOCK_SKEW`] as a `time::Duration`, for comparing against the
/// difference of two [`OffsetDateTime`]s.
fn skew_tolerance() -> time::Duration {
    time::Duration::try_from(MAX_CLOCK_SKEW).unwrap_or(time::Duration::ZERO)
}

/// The `depends_on[].trust_class` wire spelling.
fn trust_wire(class: TrustClass) -> &'static str {
    match class {
        TrustClass::Exact => "exact",
        TrustClass::Versioned => "versioned",
        TrustClass::Heuristic => "heuristic",
        TrustClass::Volatile => "volatile",
    }
}

/// The verdict for a `volatile` edge that passed the TTL gate
/// (ARCHITECTURE §7 step 3).
///
/// **Total in the probe outcome, and that totality is the point.**
/// `None` covers every path that produced no probe result — no probe
/// registered for the scheme, an arbitration tie, a probe uninstalled
/// since it last answered — and the three `Some` arms cover the rest.
/// Written as one exhaustive match rather than as guards sprinkled
/// through `evaluate_edge`, because the defect this replaces was
/// precisely a reader deciding that one of these paths deserved
/// different treatment.
///
/// The invariant it expresses: **inside a validated TTL, a probe may
/// only make a `volatile` verdict stricter.** The baseline is
/// `LikelyValid`, licensed by the declared TTL that step 1 already
/// validated; a probe result can lower it to `Stale` and can do nothing
/// else. It cannot raise it — the recorded class stays `Volatile`, which
/// `Validity::aggregate` caps at `LikelyValid` and
/// `Certificate::check_invariants` refuses outright at `Valid`. And it
/// cannot lower it to `Unknown`.
///
/// That last clause is the one worth guarding. Probe `Unknown` lands
/// here alongside "no probe registered" because `Unknown` from a probe
/// is the *absence* of evidence, and the validated TTL is the evidence
/// that survives in both cases. Mapping it to `EdgeVerdict::Unknown`
/// would restore the verifier's original reproduction verbatim:
/// `web.search://` unprobed at `likely-valid` and `file:///`
/// probed-and-undecided at `unknown`, same trust class, same TTL, same
/// absent resource, differing only in which scheme happens to have a
/// probe installed.
///
/// `Drift` is different in kind. A trust class bounds how strongly
/// FreshDAG may assert something is *unchanged*; it says nothing about
/// its ability to observe that something *changed*. `volatile` means
/// "no trustworthy signal that this is the same", not "no signal at
/// all" — the `https://` probe classifies `Cache-Control: no-store` as
/// `volatile` while still holding a comparable `ETag` or content hash,
/// so a `no-store` resource that demonstrably moved is observably
/// `Drift`. Discarding that to preserve `LikelyValid` would report a
/// verdict stronger than the evidence supports, which is invariant #7's
/// harm arriving from the opposite direction, and invariant #15 settles
/// it: correctness beats cache hit rate.
///
/// Known remainder, deliberately open (ARCHITECTURE §7, `BUILD_PLAN`
/// §7): a probe that reported `Drift` and is then uninstalled lands on
/// the `None` arm, so the edge returns to `LikelyValid`. Consuming prior
/// drift observations from the log is a store-projection question, not
/// an evaluation-order one.
fn volatile_within_ttl_verdict(dep: &Dependency, probed: Option<&ProbeResult>) -> EdgeOutcome {
    match probed {
        Some(ProbeResult::Drift { .. }) => EdgeOutcome {
            dependency: dep.clone(),
            verdict: EdgeVerdict::Drift,
            reason: Some((ReasonCode::Drift, None)),
        },
        // A probe ran and agreed. The cap is a trust-class ceiling on a
        // checked result.
        Some(ProbeResult::Match { .. }) => EdgeOutcome {
            dependency: dep.clone(),
            verdict: EdgeVerdict::Match {
                recorded_trust_class: TrustClass::Volatile,
                observed_trust_class: TrustClass::Volatile,
                observed_fp: dep.fingerprint.clone(),
            },
            reason: caps_reason(TrustClass::Volatile),
        },
        // A probe RAN and could not decide. Same verdict, and the same
        // evidence value — none — but a different fact about the world,
        // and `probe-unknown` is the code that already means exactly
        // this: certificate-contract says it "asserts a probe
        // executed", against `no-probe-available` which asserts one did
        // not.
        //
        // It lands on `Match` rather than edge `Unknown` because the
        // validated TTL is what survives when a probe supplies nothing
        // (ADR 0009 Amendment 2); mapping it to `Unknown` would restore
        // the scheme-dependence that amendment removed. What it must
        // NOT do is borrow `volatile-within-ttl-unprobed`, whose §Decision
        // 2 emission condition is "no probe was consulted" and whose
        // wire name says `unprobed`. An earlier revision did exactly
        // that, and the CLI then told users "NOTHING CHECKED THIS
        // DEPENDENCY. No probe is registered for its scheme" about an
        // edge whose probe had just answered.
        Some(ProbeResult::Unknown { .. }) => EdgeOutcome {
            dependency: dep.clone(),
            verdict: EdgeVerdict::Match {
                recorded_trust_class: TrustClass::Volatile,
                observed_trust_class: TrustClass::Volatile,
                observed_fp: dep.fingerprint.clone(),
            },
            reason: Some((ReasonCode::ProbeUnknown, None)),
        },
        // Nothing was consulted at all: no probe registered for the
        // scheme, arbitration tied, or the probe that recorded the
        // fingerprint was removed. This is ADR 0009 §Decision 2's
        // emission condition, verbatim, and the only evidence is the
        // producer's declared TTL.
        None => EdgeOutcome {
            dependency: dep.clone(),
            verdict: EdgeVerdict::Match {
                recorded_trust_class: TrustClass::Volatile,
                observed_trust_class: TrustClass::Volatile,
                observed_fp: dep.fingerprint.clone(),
            },
            reason: Some((ReasonCode::VolatileWithinTtlUnprobed, None)),
        },
    }
}

/// The edge-scoped reason a *matching* edge contributes, if any.
///
/// This mirrors `Validity::aggregate`'s own table; the engine's
/// self-audit in `seal` fails loudly if the two ever disagree. Every arm
/// has a producer: the `Volatile` one is reached through
/// [`volatile_within_ttl_verdict`].
fn caps_reason(recorded: TrustClass) -> Option<(ReasonCode, Option<String>)> {
    match recorded {
        TrustClass::Heuristic => Some((ReasonCode::TrustClassHeuristicCapsAtLikelyValid, None)),
        TrustClass::Volatile => Some((ReasonCode::TrustClassVolatileCapsAtLikelyValid, None)),
        TrustClass::Exact | TrustClass::Versioned => None,
    }
}

/// Bound a probe-supplied `detail` string.
///
/// Determinism and secret-freedom are the probe's contractual duty
/// (probe-contract §Failure Modes); length is enforced here because it
/// is the one property the engine can check cheaply. Truncation is on a
/// char boundary so the result is always valid UTF-8 and always the
/// same for the same input.
fn sanitize_detail(reason: &str) -> String {
    if reason.len() <= MAX_DETAIL_BYTES {
        return reason.to_string();
    }
    let mut end = MAX_DETAIL_BYTES;
    while end > 0 && !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_string()
}

/// Builder for [`Engine`].
#[derive(Debug, Default)]
pub struct EngineBuilder {
    events: Vec<IrEvent>,
    coverage: CoverageRegistry,
    registry: ProbeRegistry,
    clock: Option<Arc<dyn Clock>>,
    max_volatile_ttl: Option<std::time::Duration>,
}

impl EngineBuilder {
    /// Supply the canonical observation log. Order does not matter; the
    /// derived graph linearizes internally.
    #[must_use]
    pub fn events(mut self, events: impl IntoIterator<Item = IrEvent>) -> Self {
        self.events.extend(events);
        self
    }

    /// Supply the producer coverage registry.
    #[must_use]
    pub fn coverage(mut self, coverage: CoverageRegistry) -> Self {
        self.coverage = coverage;
        self
    }

    /// Supply the probe registry.
    #[must_use]
    pub fn probes(mut self, registry: ProbeRegistry) -> Self {
        self.registry = registry;
        self
    }

    /// Supply the clock. Defaults to [`SystemClock`]; tests MUST inject
    /// a [`FixedClock`](crate::FixedClock).
    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Cap how long a declared `volatile` TTL may be and still count as
    /// evidence. Defaults to [`DEFAULT_MAX_VOLATILE_TTL`] (24h).
    ///
    /// Raising this weakens `ARCHITECTURE.md §7`'s "the declared TTL is
    /// present evidence" argument in proportion, which is why it is a
    /// deliberate call at the call site rather than a constant someone
    /// edits.
    #[must_use]
    pub fn max_volatile_ttl(mut self, max: std::time::Duration) -> Self {
        self.max_volatile_ttl = Some(max);
        self
    }

    /// Materialize the derived graph and build the engine.
    #[must_use]
    pub fn build(self) -> Engine {
        let graph = DerivedGraph::replay(self.events.clone(), &self.coverage);
        Engine {
            events: self.events,
            graph,
            registry: self.registry,
            clock: self.clock.unwrap_or_else(|| Arc::new(SystemClock)),
            ledger: Mutex::new(TrustLedger::new()),
            max_volatile_ttl: self.max_volatile_ttl.unwrap_or(DEFAULT_MAX_VOLATILE_TTL),
        }
    }
}

/// Does a status mean "no drift and no missing evidence"?
///
/// `Valid` and `LikelyValid` differ in strength but both qualify;
/// `Unknown` never does. Invariant #7 stated as a predicate.
#[must_use]
pub fn is_usable(status: ValidityStatus) -> bool {
    matches!(status, ValidityStatus::Valid | ValidityStatus::LikelyValid)
}
