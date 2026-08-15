//! Validity evaluation: from an artifact id to a certificate.
//!
//! The dispatch order for one dependency edge, and why it is that order:
//!
//! 1. **TTL first.** A `volatile` edge whose declared TTL has elapsed is
//!    `Unknown` with [`ReasonCode::TtlExpired`] before any probe runs.
//!    Asking a probe about an expired window and believing its answer
//!    would be the promotion invariant #7 forbids.
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

use std::collections::BTreeSet;
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
        // within one computation with different fingerprints. The engine
        // has no reason code for it (adding one is a contract change), so
        // it surfaces in the log rather than vanishing.
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

        // 1. TTL, before anything else (ARCHITECTURE §7).
        if matches!(dep.trust_class, TrustClass::Volatile) {
            match dep.ttl_seconds {
                None => {
                    return unknown(ReasonCode::TtlExpired, "ttl_seconds=absent".to_string());
                }
                Some(secs) => {
                    let expires_at = dep.observed_at
                        + time::Duration::seconds(i64::try_from(secs).unwrap_or(i64::MAX));
                    if now > expires_at {
                        return unknown(ReasonCode::TtlExpired, format!("ttl_seconds={secs}"));
                    }
                }
            }
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
                // The single case where "no probe" is not `Unknown`:
                // ARCHITECTURE §7 defines a `volatile` edge inside its
                // TTL as LikelyValid, and probe-contract §Trust-class
                // Semantics defines the volatile probe's entire job as
                // that same TTL comparison. Step 1 above already proved
                // the window is open. The status is capped at
                // LikelyValid and carries the volatile reason code, so
                // nothing is promoted to `valid`.
                if matches!(dep.trust_class, TrustClass::Volatile) {
                    return EdgeOutcome {
                        dependency: dep.clone(),
                        verdict: EdgeVerdict::Match {
                            recorded_trust_class: TrustClass::Volatile,
                            observed_trust_class: TrustClass::Volatile,
                            observed_fp: dep.fingerprint.clone(),
                        },
                        reason: Some((
                            ReasonCode::TrustClassVolatileCapsAtLikelyValid,
                            Some("within-declared-ttl".to_string()),
                        )),
                    };
                }
                return unknown(ReasonCode::NoProbeAvailable, no_probe.detail());
            }
        };

        // 4. Dispatch.
        let ttl_hint = dep.ttl_seconds.map(std::time::Duration::from_secs);
        let result = selected.probe.check(&dep.key, &dep.fingerprint, ttl_hint);

        match result {
            ProbeResult::Match {
                observed_fp,
                observed_trust_class,
            } => {
                let transition = ledger.observe(
                    &dep.key,
                    &selected.identity,
                    dep.trust_class,
                    observed_trust_class,
                    now,
                );
                // The class written to the log is the *adopted* one, so
                // a replay of this event cannot escalate a dependency's
                // trust class behind the anti-thrash protocol's back.
                let adopted = ledger
                    .adopted(&dep.key, &selected.identity)
                    .unwrap_or(dep.trust_class);
                emitted.push(self.probe_checked(
                    computation,
                    now,
                    sequence,
                    dep,
                    "match",
                    Some(&observed_fp.to_string()),
                    adopted,
                    observed_trust_class,
                    None,
                ));
                if let TrustTransition::Demoted { from, to } = transition {
                    emitted.push(self.trust_demoted(
                        computation,
                        now,
                        sequence,
                        dep,
                        &selected.identity,
                        from,
                        to,
                    ));
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
            ProbeResult::Drift {
                observed_fp,
                observed_trust_class,
            } => {
                ledger.note_probe(&dep.key, &selected.identity);
                emitted.push(self.probe_checked(
                    computation,
                    now,
                    sequence,
                    dep,
                    "drift",
                    Some(&observed_fp.to_string()),
                    dep.trust_class,
                    observed_trust_class,
                    None,
                ));
                EdgeOutcome {
                    dependency: dep.clone(),
                    verdict: EdgeVerdict::Drift,
                    reason: Some((ReasonCode::Drift, None)),
                }
            }
            ProbeResult::Unknown { reason, retryable } => {
                emitted.push(self.probe_checked(
                    computation,
                    now,
                    sequence,
                    dep,
                    "unknown",
                    None,
                    dep.trust_class,
                    dep.trust_class,
                    Some(retryable),
                ));
                if retryable {
                    return unknown(ReasonCode::ProbeUnknown, sanitize_detail(&reason));
                }
                // probe-contract §Anti-thrash Protocol: an unretryable
                // Unknown means the signal backing the recorded class is
                // gone. Demotion is explicit, never silent.
                let transition =
                    ledger.note_unretryable_failure(&dep.key, &selected.identity, dep.trust_class);
                if let TrustTransition::Demoted { from, to } = transition {
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
                unknown(ReasonCode::ProbeTrustDemoted, sanitize_detail(&reason))
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

/// The `depends_on[].trust_class` wire spelling.
fn trust_wire(class: TrustClass) -> &'static str {
    match class {
        TrustClass::Exact => "exact",
        TrustClass::Versioned => "versioned",
        TrustClass::Heuristic => "heuristic",
        TrustClass::Volatile => "volatile",
    }
}

/// The edge-scoped reason a *matching* edge contributes, if any.
///
/// This mirrors `Validity::aggregate`'s own table; the engine's
/// self-audit in `seal` fails loudly if the two ever disagree.
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
