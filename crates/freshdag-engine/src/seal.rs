//! The single choke point through which every certificate passes.
//!
//! # Why this module exists
//!
//! Invariant #7 is only as strong as the weakest path from evidence to
//! `status.value`. An adversarial reader's job is to find a route from
//! `Engine::check` to a `valid` certificate that skips
//! [`Certificate::check_coverage_deficit`]. This module makes that
//! route non-existent rather than merely discouraged, by three
//! mechanisms that hold together:
//!
//! 1. **One construction site.** The `Certificate { .. }` struct
//!    literal appears exactly once in this crate, inside `seal` below.
//!    `certificate_is_constructed_in_exactly_one_place` in
//!    `crate::tests` reads the crate's own sources and fails if a
//!    second one appears — as does
//!    `coverage_deficit_is_checked_in_exactly_one_place`.
//! 2. **A token only the gate can mint.** `GateOutcome` is
//!    module-private with private fields and exactly one constructor —
//!    `run_gate`, whose body calls `check_coverage_deficit`
//!    unconditionally. `Gated::admit` consumes a `GateOutcome` by
//!    value, and `Gated::into_certificate` is the only way to turn a
//!    `Gated` back into a `Certificate`; `seal`'s sole `Ok` return goes
//!    through it. So "produce a certificate" type-checks only after
//!    "run the gate".
//! 3. **No branch between them.** Between the struct literal and the
//!    gate call there is no conditional and no early `Ok(..)` — only
//!    `Err(..)` returns, which emit nothing.
//!
//! Mechanism 2 is compile-time and covers everything inside this
//! module; mechanism 1 is a source-level test and covers the rest of
//! the crate; mechanism 3 covers this function's own control flow.
//!
//! [`Certificate::check_coverage_deficit`]: freshdag_core::certificate::Certificate::check_coverage_deficit

use std::collections::BTreeSet;

use freshdag_core::artifact::Artifact;
use freshdag_core::certificate::{
    Certificate, Comparator, CoverageEntry, InvariantError, ProducedBy, ReasonMapping, Status,
    CERTIFICATE_SCHEMA_V0_1,
};
use freshdag_core::dependency::{
    Dependency, EdgeVerdict, ReasonCode, Validity, ValidityReason, ValidityStatus,
};
use freshdag_core::ir::{Hash, HashAlgo, IrEvent};
use time::OffsetDateTime;

use crate::error::EngineError;

/// Where `observation_coverage` came from, which decides what a missing
/// producer means.
///
/// Certificate contract, `ProducerMissingFromCoverage` §"dual-use": on
/// emission the engine assembles the list itself, so a gap is an engine
/// bug; on re-check the list came from the document, so a gap is real
/// evidence that the certificate's silences cannot be interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageAuthority {
    /// The engine built `observation_coverage` from the producers it
    /// saw. A producer missing from it is fatal.
    EngineAssembled,
    /// `observation_coverage` was read off an existing certificate
    /// (machine A → machine B). A producer missing from it downgrades
    /// the status to `Unknown`.
    FromDocument,
}

/// One dependency edge's evaluated outcome.
#[derive(Debug, Clone)]
pub struct EdgeOutcome {
    /// The dependency as recorded by the store.
    pub dependency: Dependency,
    /// The verdict the probe (or the TTL rule) produced.
    pub verdict: EdgeVerdict,
    /// The edge-scoped reason, if this edge is not silently fine.
    /// `detail` is non-normative and never decides anything.
    pub reason: Option<(ReasonCode, Option<String>)>,
}

/// Everything [`seal`] needs. Assembled by [`crate::Engine`].
#[derive(Debug)]
pub struct SealInput<'a> {
    /// The artifact being certified.
    pub artifact: Artifact,
    /// Provenance.
    pub produced_by: ProducedBy,
    /// Per-edge outcomes, in `depends_on[]` order.
    pub edges: Vec<EdgeOutcome>,
    /// Artifact-scoped reasons the engine detected independently of
    /// [`Certificate::check_coverage_deficit`] — notably the general
    /// effect-kind deficit and the always-on `bash`/`task` obligation
    /// check, which must be reported at every status, not only `valid`.
    pub artifact_reasons: Vec<ValidityReason>,
    /// Producers that contributed.
    pub coverage: Vec<CoverageEntry>,
    /// The event stream this certificate is about.
    pub events: &'a [IrEvent],
    /// Where `coverage` came from.
    pub authority: CoverageAuthority,
    /// Producers the engine already knows have no registered manifest,
    /// and has already accounted for as evidence. A
    /// `ProducerMissingFromCoverage` naming one of these is expected;
    /// naming any other producer on the emission path is an engine bug.
    pub accounted_missing_producers: BTreeSet<String>,
    /// `status.checked`, from the injected clock.
    pub checked_at: OffsetDateTime,
    /// Comparator, if the computation declared one.
    pub comparator: Option<Comparator>,
}

/// Sort key for artifact-scoped reasons.
///
/// The contract fixes edge-scoped reasons in `depends_on[]` order with
/// artifact-scoped reasons after them, but leaves the order *within*
/// the artifact-scoped block open. The engine fixes it here as
/// cause-before-symptom: a coverage failure explains why the dependency
/// set looks the way it does, so it must precede
/// `no-dependencies-observed`. `cert_id` hashes `status.reasons[]` and
/// fixtures pin `reasons[0]`, so this must be deterministic.
fn artifact_reason_rank(code: ReasonCode) -> u8 {
    match code {
        ReasonCode::CoverageDeficit => 0,
        ReasonCode::ProducerMissingFromCoverage => 1,
        ReasonCode::NoDependenciesObserved => 2,
        // Edge-scoped codes never reach here; rank them last so a
        // future miscategorisation is visible rather than silent.
        _ => u8::MAX,
    }
}

/// Cap a status at `Unknown`: `Valid`/`LikelyValid` become `Unknown`;
/// `Stale` and `Unknown` are already at or below it.
///
/// `Stale` is deliberately *not* raised to `Unknown`. Drift is positive
/// evidence that the artifact is out of date; a coverage gap does not
/// make it less out of date.
fn cap_at_unknown(status: ValidityStatus) -> ValidityStatus {
    match status {
        ValidityStatus::Valid | ValidityStatus::LikelyValid => ValidityStatus::Unknown,
        ValidityStatus::Stale | ValidityStatus::Unknown => status,
    }
}

/// Proof that [`run_gate`] ran. Module-private, private fields, one
/// constructor. See this module's header, mechanism 2.
#[must_use]
struct GateOutcome {
    /// Extra artifact-scoped reasons the gate found.
    reasons: Vec<ValidityReason>,
    /// Whether the gate requires the status be capped at `Unknown`.
    cap: bool,
}

/// A certificate that has been through the gate.
#[must_use]
struct Gated(Certificate);

impl Gated {
    /// The only constructor. Takes a [`GateOutcome`] by value, so it
    /// cannot be called without having run the gate.
    fn admit(certificate: Certificate, _proof: GateOutcome) -> Self {
        Self(certificate)
    }

    fn into_certificate(self) -> Certificate {
        self.0
    }
}

/// Run `Certificate::check_coverage_deficit` and translate the result.
///
/// This function is the only producer of a [`GateOutcome`], and its
/// body calls `check_coverage_deficit` unconditionally on the path to
/// every `Ok`.
fn run_gate(
    certificate: &Certificate,
    events: &[IrEvent],
    authority: CoverageAuthority,
    accounted_missing: &BTreeSet<String>,
) -> Result<GateOutcome, EngineError> {
    // `check_coverage_deficit` returns on the *first* producer missing
    // from `observation_coverage`. That is fine for core — one violation
    // is one violation — but the engine asks a different question of the
    // answer: "is this a gap I already know about, or a bug in my own
    // assembly?" Consulting only the first offender lets a pre-accounted
    // producer that happens to appear earlier in the stream mask a
    // genuinely unaccounted one, and `CoverageAssemblyBug` — which must
    // be fatal — silently becomes a downgrade. So the engine audits
    // every missing producer against its own accounting, before
    // interpreting whichever one core reported.
    if matches!(authority, CoverageAuthority::EngineAssembled) {
        if let Some(producer) = unaccounted_producers(certificate, events, accounted_missing).next()
        {
            return Err(EngineError::CoverageAssemblyBug { producer });
        }
    }

    let Err(violation) = certificate.check_coverage_deficit(events) else {
        return Ok(GateOutcome {
            reasons: Vec::new(),
            cap: false,
        });
    };

    match violation.reason_mapping() {
        ReasonMapping::Downgrade(ReasonCode::ProducerMissingFromCoverage) => {
            let InvariantError::ProducerMissingFromCoverage { producer } = &violation else {
                return Err(EngineError::InternalInconsistency {
                    message: format!(
                        "reason_mapping said ProducerMissingFromCoverage for {violation:?}"
                    ),
                });
            };
            // The dual-use split. On the emission path the audit above
            // already proved every missing producer is accounted for, so
            // reaching here with an unaccounted one would mean the two
            // disagree — which is a defect in this function, not a fact
            // about the world.
            if matches!(authority, CoverageAuthority::EngineAssembled)
                && !accounted_missing.contains(producer)
            {
                return Err(EngineError::CoverageAssemblyBug {
                    producer: producer.clone(),
                });
            }
            Ok(GateOutcome {
                reasons: vec![ValidityReason {
                    dependency_key: String::new(),
                    reason: ReasonCode::ProducerMissingFromCoverage,
                    detail: Some(format!("producer={producer}")),
                }],
                cap: true,
            })
        }
        ReasonMapping::Downgrade(code) => {
            let detail = match &violation {
                InvariantError::CoverageDeficit { tool_kind } => {
                    Some(format!("tool_kind={tool_kind}"))
                }
                _ => None,
            };
            Ok(GateOutcome {
                reasons: vec![ValidityReason {
                    dependency_key: String::new(),
                    reason: code,
                    detail,
                }],
                cap: true,
            })
        }
        ReasonMapping::StructuralDefect => {
            Err(EngineError::MalformedCertificate { source: violation })
        }
    }
}

/// Every producer that emitted an event, is absent from
/// `observation_coverage`, and is not one the engine pre-accounted for.
///
/// Sorted and deduplicated, so which producer a `CoverageAssemblyBug`
/// names is a function of the inputs rather than of event order — the
/// error is part of the engine's observable behaviour and fixtures may
/// pin it.
fn unaccounted_producers<'a>(
    certificate: &'a Certificate,
    events: &'a [IrEvent],
    accounted_missing: &'a BTreeSet<String>,
) -> impl Iterator<Item = String> + 'a {
    let declared: BTreeSet<&str> = certificate
        .observation_coverage
        .iter()
        .map(|c| c.producer.as_str())
        .collect();
    events
        .iter()
        .map(|e| e.producer.clone())
        .filter(move |p| !declared.contains(p.as_str()) && !accounted_missing.contains(p))
        .collect::<BTreeSet<String>>()
        .into_iter()
}

/// Aggregate, gate, and assemble. The only path to a `Certificate` in
/// this crate.
///
/// # Errors
///
/// Any [`EngineError`]; every one of them means no certificate is
/// emitted at all.
#[allow(clippy::too_many_lines)] // the gate must stay one straight-line procedure
pub fn seal(input: SealInput<'_>) -> Result<Certificate, EngineError> {
    let SealInput {
        artifact,
        produced_by,
        edges,
        artifact_reasons,
        coverage,
        events,
        authority,
        accounted_missing_producers,
        checked_at,
        comparator,
    } = input;

    let depends_on: Vec<Dependency> = edges.iter().map(|e| e.dependency.clone()).collect();
    let keys: Vec<String> = depends_on.iter().map(|d| d.key.clone()).collect();
    let verdicts: Vec<EdgeVerdict> = edges.iter().map(|e| e.verdict.clone()).collect();

    // `Validity::aggregate` is the ONLY source of the status value. The
    // engine never computes a status by hand.
    let aggregated = Validity::aggregate(&verdicts, &keys)?;

    // Edge reasons, in depends_on[] order, from the engine's richer
    // vocabulary (NoProbeAvailable / TtlExpired / ProbeTrustDemoted,
    // which `aggregate` cannot distinguish because it never sees why an
    // edge was Unknown).
    let mut edge_reasons: Vec<ValidityReason> = Vec::new();
    for edge in &edges {
        if let Some((code, detail)) = &edge.reason {
            if code.is_artifact_scoped() {
                return Err(EngineError::InternalInconsistency {
                    message: format!(
                        "edge `{}` carries artifact-scoped reason `{code}`",
                        edge.dependency.key
                    ),
                });
            }
            edge_reasons.push(ValidityReason {
                dependency_key: edge.dependency.key.clone(),
                reason: *code,
                detail: detail.clone(),
            });
        }
    }

    // Self-audit: our per-edge reasons must name exactly the edges
    // `aggregate` flagged. A disagreement means the engine's edge
    // classification and core's aggregation have drifted apart, which
    // is precisely the situation in which an Unknown could be lost.
    if !matches!(aggregated.value, ValidityStatus::Valid) {
        let ours: BTreeSet<&str> = edge_reasons
            .iter()
            .map(|r| r.dependency_key.as_str())
            .collect();
        let theirs: BTreeSet<&str> = aggregated
            .reasons
            .iter()
            .filter(|r| !r.reason.is_artifact_scoped())
            .map(|r| r.dependency_key.as_str())
            .collect();
        if ours != theirs {
            return Err(EngineError::InternalInconsistency {
                message: format!(
                    "edge reasons {ours:?} disagree with Validity::aggregate {theirs:?}"
                ),
            });
        }
    }

    let mut artifact_scoped: Vec<ValidityReason> = artifact_reasons;
    // `aggregate` owns the empty-dependency-set case; adopt whatever
    // artifact-scoped reasons it produced rather than re-deriving them.
    for reason in aggregated
        .reasons
        .iter()
        .filter(|r| r.reason.is_artifact_scoped())
    {
        if !artifact_scoped.iter().any(|r| r.reason == reason.reason) {
            artifact_scoped.push(reason.clone());
        }
    }

    let status_value = aggregated.value;

    // --- the one construction site -------------------------------------
    // Everything from here to `run_gate` below is straight-line code.
    let mut certificate = Certificate {
        cert_id: zero_hash(),
        schema: CERTIFICATE_SCHEMA_V0_1.to_string(),
        artifact,
        produced_by,
        depends_on,
        comparator,
        status: Status {
            value: status_value,
            checked: checked_at,
            reasons: Vec::new(),
        },
        observation_coverage: coverage,
    };
    let gate = run_gate(
        &certificate,
        events,
        authority,
        &accounted_missing_producers,
    )?;
    // -------------------------------------------------------------------

    for reason in &gate.reasons {
        if !artifact_scoped.iter().any(|r| r.reason == reason.reason) {
            artifact_scoped.push(reason.clone());
        }
    }
    let capped = gate.cap
        || artifact_scoped.iter().any(|r| {
            matches!(
                r.reason,
                ReasonCode::CoverageDeficit | ReasonCode::ProducerMissingFromCoverage
            )
        });
    let final_status = if capped {
        cap_at_unknown(status_value)
    } else {
        status_value
    };

    artifact_scoped.sort_by_key(|r| artifact_reason_rank(r.reason));
    let mut reasons = edge_reasons;
    reasons.extend(artifact_scoped);

    if matches!(final_status, ValidityStatus::Valid) {
        // A `Valid` certificate carries no reasons by contract. If the
        // engine got here with reasons in hand, its own classification
        // disagrees with the status and the honest response is to emit
        // nothing.
        if !reasons.is_empty() {
            return Err(EngineError::InternalInconsistency {
                message: format!(
                    "status is Valid but {} reason(s) were produced",
                    reasons.len()
                ),
            });
        }
    } else if reasons.is_empty() {
        return Err(EngineError::InternalInconsistency {
            message: format!("status is {final_status:?} with no reasons (invariant #6)"),
        });
    }

    if matches!(
        final_status,
        ValidityStatus::Valid | ValidityStatus::LikelyValid
    ) && certificate.produced_by.recipe_hash.is_none()
    {
        return Err(EngineError::MissingRecipeHash {
            status: final_status,
        });
    }

    certificate.status.value = final_status;
    certificate.status.reasons = reasons;

    // `check_invariants` is redundant with the schema and with the
    // logic above. That redundancy is the point: it is the last place
    // an invariant-#7 violation can be caught before bytes exist.
    // Note the asymmetry with `run_gate`: reaching here means the
    // engine already applied every downgrade it knows how to apply, so
    // *any* surviving violation — including one whose `reason_mapping`
    // is `Downgrade` — is a defect in the engine's own assembly, not a
    // fact about the world. Emitting nothing is the only honest answer.
    if let Err(violation) = certificate.check_invariants() {
        return Err(EngineError::MalformedCertificate { source: violation });
    }

    certificate.cert_id =
        certificate
            .derive_cert_id()
            .map_err(|e| EngineError::InternalInconsistency {
                message: format!("cert_id derivation failed: {e}"),
            })?;

    Ok(Gated::admit(certificate, gate).into_certificate())
}

fn zero_hash() -> Hash {
    Hash {
        algo: HashAlgo::Blake3,
        digest_hex: "0".repeat(64),
    }
}
