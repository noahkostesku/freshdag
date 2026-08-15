//! Direct tests of the coverage gate in `crate::seal`.
//!
//! These exercise paths `Engine::check` cannot reach *by construction* —
//! which is the point. The engine assembles `observation_coverage` from
//! the same events it passes to the gate, so a producer missing from it
//! is impossible unless the engine is buggy. This module fakes that bug
//! and asserts the gate refuses to emit rather than downgrading, because
//! downgrading would let the engine hide its own invariant-#7 violation
//! behind a plausible-looking `unknown`.

use std::collections::BTreeSet;

use freshdag_core::artifact::{Artifact, ArtifactId};
use freshdag_core::certificate::{CoverageEntry, ProducedBy};
use freshdag_core::computation::ComputationId;
use freshdag_core::dependency::{
    Dependency, EdgeVerdict, Fingerprint, FingerprintKind, ReasonCode, TrustClass, ValidityStatus,
};
use freshdag_core::ir::{EventKind, EventKindPattern, Hash, HashAlgo, IrEvent, ProducerRole};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::seal::{seal, CoverageAuthority, EdgeOutcome, SealInput};
use crate::EngineError;

use super::support::blake3_of;

fn hash(label: &str) -> Hash {
    Hash::new(HashAlgo::Blake3, blake3_of(label)).expect("valid digest")
}

fn artifact() -> Artifact {
    Artifact {
        id: ArtifactId(format!("blake3:{}", blake3_of("gate/artifact"))),
        path: Some("/repo/out.md".to_string()),
        kind: "text/markdown".to_string(),
        content_hash: hash("gate/artifact"),
        size: 64,
    }
}

fn produced_by(recipe_hash: Option<Hash>) -> ProducedBy {
    ProducedBy {
        computation_id: ComputationId(format!("comp:{}", blake3_of("gate"))),
        recipe: Some("gate".to_string()),
        recipe_hash,
        adapter: "freshdag-adapter-claude/0.1.0".to_string(),
        started: OffsetDateTime::UNIX_EPOCH,
        ended: OffsetDateTime::UNIX_EPOCH,
    }
}

fn coverage(producer: &str) -> Vec<CoverageEntry> {
    vec![CoverageEntry {
        producer: producer.to_string(),
        version: "0.1.0".to_string(),
        role: ProducerRole::Adapter,
        emits: vec![EventKindPattern::new("fs.*")],
        known_limitations: Vec::new(),
    }]
}

fn event(producer: &str) -> IrEvent {
    IrEvent {
        event_id: Uuid::nil(),
        producer: producer.to_string(),
        producer_version: "0.1.0".to_string(),
        session_id: "s".to_string(),
        computation_id: Some(format!("comp:{}", blake3_of("gate"))),
        parent_id: None,
        causal_inputs: None,
        ts: OffsetDateTime::UNIX_EPOCH,
        kind: EventKind::FsRead,
        payload: serde_json::json!({ "path": "/repo/notes.md", "size": 1 }),
    }
}

fn matching_edge() -> EdgeOutcome {
    let fingerprint = Fingerprint::new(
        FingerprintKind::ContentHash,
        format!("blake3:{}", blake3_of("dep")),
    );
    EdgeOutcome {
        dependency: Dependency {
            key: "file:///repo/notes.md".to_string(),
            scheme: "file".to_string(),
            trust_class: TrustClass::Exact,
            fingerprint: fingerprint.clone(),
            observed_at: OffsetDateTime::UNIX_EPOCH,
            produced_by: None,
            ttl_seconds: None,
        },
        verdict: EdgeVerdict::Match {
            recorded_trust_class: TrustClass::Exact,
            observed_trust_class: TrustClass::Exact,
            observed_fp: fingerprint,
        },
        reason: None,
    }
}

fn input(
    events: &[IrEvent],
    coverage: Vec<CoverageEntry>,
    authority: CoverageAuthority,
    accounted: BTreeSet<String>,
    edges: Vec<EdgeOutcome>,
    recipe_hash: Option<Hash>,
) -> SealInput<'_> {
    SealInput {
        artifact: artifact(),
        produced_by: produced_by(recipe_hash),
        edges,
        artifact_reasons: Vec::new(),
        coverage,
        events,
        authority,
        accounted_missing_producers: accounted,
        checked_at: OffsetDateTime::UNIX_EPOCH,
        comparator: None,
    }
}

/// Emission path: the engine built the coverage list, so a producer
/// missing from it is an engine bug. Fatal, not a downgrade.
#[test]
fn a_missing_producer_is_fatal_when_the_engine_assembled_the_coverage() {
    let events = vec![event("ghost-producer")];
    let err = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::EngineAssembled,
        BTreeSet::new(),
        Vec::new(),
        Some(hash("recipe")),
    ))
    .expect_err("engine-assembled coverage with a gap must refuse to emit");
    assert_eq!(
        err,
        EngineError::CoverageAssemblyBug {
            producer: "ghost-producer".to_string()
        }
    );
}

/// Re-check path: the coverage list came off the document, so a missing
/// producer is real evidence that the certificate's silences cannot be
/// interpreted. Downgrade to `unknown`, with the reason attached.
#[test]
fn a_missing_producer_downgrades_when_the_coverage_came_from_the_document() {
    let events = vec![event("ghost-producer")];
    let certificate = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::FromDocument,
        BTreeSet::new(),
        Vec::new(),
        Some(hash("recipe")),
    ))
    .expect("a portable certificate with an uninterpretable silence is still emittable");
    assert_eq!(certificate.status.value, ValidityStatus::Unknown);
    assert!(certificate
        .status
        .reasons
        .iter()
        .any(|r| r.reason == ReasonCode::ProducerMissingFromCoverage));
}

/// The engine pre-accounts producers it knows have no manifest, so the
/// same violation is expected rather than a bug.
#[test]
fn an_accounted_missing_producer_downgrades_on_the_emission_path_too() {
    let events = vec![event("ghost-producer")];
    let mut accounted = BTreeSet::new();
    accounted.insert("ghost-producer".to_string());
    let certificate = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::EngineAssembled,
        accounted,
        Vec::new(),
        Some(hash("recipe")),
    ))
    .expect("an accounted-for gap is evidence, not a bug");
    assert_eq!(certificate.status.value, ValidityStatus::Unknown);
}

/// A structural defect emits nothing. `ReasonMapping::StructuralDefect`
/// exists precisely so this cannot be quietly downgraded.
#[test]
fn an_empty_coverage_list_is_a_structural_defect() {
    let events: Vec<IrEvent> = Vec::new();
    let err = seal(input(
        &events,
        Vec::new(),
        CoverageAuthority::EngineAssembled,
        BTreeSet::new(),
        Vec::new(),
        Some(hash("recipe")),
    ))
    .expect_err("a certificate naming no producers is malformed");
    assert!(matches!(err, EngineError::MalformedCertificate { .. }));
}

/// certificate-contract §Field Rules: `recipe_hash` is required for
/// `valid` and `likely-valid`. Without one the engine refuses rather
/// than emitting a certificate whose provenance cannot be checked.
#[test]
fn a_valid_status_without_a_recipe_hash_is_refused() {
    let events: Vec<IrEvent> = Vec::new();
    let err = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::EngineAssembled,
        BTreeSet::new(),
        vec![matching_edge()],
        None,
    ))
    .expect_err("no recipe hash");
    assert_eq!(
        err,
        EngineError::MissingRecipeHash {
            status: ValidityStatus::Valid
        }
    );
}

/// The happy path, so the tests above cannot pass by the gate simply
/// rejecting everything.
#[test]
fn a_clean_input_seals_into_a_valid_certificate() {
    let events: Vec<IrEvent> = Vec::new();
    let certificate = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::EngineAssembled,
        BTreeSet::new(),
        vec![matching_edge()],
        Some(hash("recipe")),
    ))
    .expect("clean input");
    assert_eq!(certificate.status.value, ValidityStatus::Valid);
    assert!(certificate.status.reasons.is_empty());
    assert_eq!(
        certificate.cert_id,
        certificate.derive_cert_id().expect("re-derive"),
        "cert_id is idempotent over the emitted bytes"
    );
}

/// `Validity::aggregate` owns the empty-dependency case, and the engine
/// adopts its reason rather than re-deriving one.
#[test]
fn zero_dependencies_is_unknown_with_no_dependencies_observed() {
    let events: Vec<IrEvent> = Vec::new();
    let certificate = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::EngineAssembled,
        BTreeSet::new(),
        Vec::new(),
        Some(hash("recipe")),
    ))
    .expect("no dependencies is a legitimate, reportable state");
    assert_eq!(certificate.status.value, ValidityStatus::Unknown);
    assert_eq!(
        certificate.status.reasons[0].reason,
        ReasonCode::NoDependenciesObserved
    );
    assert_eq!(certificate.status.reasons[0].dependency_key, "");
}
