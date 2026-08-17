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
        partial: std::collections::BTreeMap::new(),
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

/// The same edge, drifted. A `Drift` verdict makes the artifact
/// `Stale`, which the certificate contract does not gate on
/// `recipe_hash`.
fn drifted_edge() -> EdgeOutcome {
    let mut edge = matching_edge();
    edge.verdict = EdgeVerdict::Drift;
    edge.reason = Some((ReasonCode::Drift, None));
    edge
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

/// D8: one pre-accounted missing producer must not mask a genuinely
/// unaccounted one.
///
/// `Certificate::check_coverage_deficit` returns on the *first* producer
/// missing from `observation_coverage`. If that first one is a gap the
/// engine already knows about, the gate took the downgrade branch and
/// `CoverageAssemblyBug` — which the design says is fatal — was never
/// raised, however many real assembly bugs followed it in the stream.
///
/// The interleaving is the whole test: `accounted-ghost` sorts and
/// appears before `rogue-producer`, so a first-miss check sees only the
/// benign one.
#[test]
fn an_accounted_missing_producer_does_not_mask_an_unaccounted_one() {
    let events = vec![event("accounted-ghost"), event("rogue-producer")];
    let mut accounted = BTreeSet::new();
    accounted.insert("accounted-ghost".to_string());
    let err = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::EngineAssembled,
        accounted,
        Vec::new(),
        Some(hash("recipe")),
    ))
    .expect_err("an unaccounted producer is an engine bug whichever order it arrives in");
    assert_eq!(
        err,
        EngineError::CoverageAssemblyBug {
            producer: "rogue-producer".to_string()
        }
    );
}

/// The same two producers in the other order, so the test above cannot
/// pass merely because the engine looks at the last event instead of the
/// first. Which producer is named must not depend on event order either.
#[test]
fn the_coverage_assembly_bug_is_independent_of_event_order() {
    let mut accounted = BTreeSet::new();
    accounted.insert("accounted-ghost".to_string());
    for events in [
        vec![event("accounted-ghost"), event("rogue-producer")],
        vec![event("rogue-producer"), event("accounted-ghost")],
    ] {
        let err = seal(input(
            &events,
            coverage("freshdag-adapter-claude"),
            CoverageAuthority::EngineAssembled,
            accounted.clone(),
            Vec::new(),
            Some(hash("recipe")),
        ))
        .expect_err("unaccounted producer");
        assert_eq!(
            err,
            EngineError::CoverageAssemblyBug {
                producer: "rogue-producer".to_string()
            }
        );
    }
}

/// The re-check path is untouched: a certificate read off a document is
/// not the engine's assembly, so missing producers stay a downgrade
/// however many there are.
#[test]
fn multiple_missing_producers_still_downgrade_on_the_document_path() {
    let events = vec![event("ghost-one"), event("ghost-two")];
    let certificate = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::FromDocument,
        BTreeSet::new(),
        Vec::new(),
        Some(hash("recipe")),
    ))
    .expect("a portable certificate with uninterpretable silences is still emittable");
    assert_eq!(certificate.status.value, ValidityStatus::Unknown);
    assert!(certificate
        .status
        .reasons
        .iter()
        .any(|r| r.reason == ReasonCode::ProducerMissingFromCoverage));
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
/// `valid` and `likely-valid`. Without one the engine **caps at
/// `unknown`** and says why, rather than refusing to seal.
///
/// It refused until ADR 0014. That was wrong for the case that
/// actually occurs: some runtimes cannot supply a recipe at all —
/// Claude Code exposes none — so refusing reported a *tool failure*
/// for an artifact whose evidence was merely incomplete, and the caller
/// got exit 3 ("ignore this result") where the truth was exit 2 ("do
/// not reuse"). Capping states the same fact as a verdict the user can
/// act on.
#[test]
fn a_valid_status_without_a_recipe_hash_is_capped_at_unknown() {
    let events: Vec<IrEvent> = Vec::new();
    let certificate = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::EngineAssembled,
        BTreeSet::new(),
        vec![matching_edge()],
        None,
    ))
    .expect("a missing recipe hash caps rather than refusing");

    assert_eq!(certificate.status.value, ValidityStatus::Unknown);
    assert_eq!(
        certificate
            .status
            .reasons
            .iter()
            .map(|r| r.reason)
            .collect::<Vec<_>>(),
        vec![ReasonCode::RecipeIdentityUnavailable],
        "the cap must explain itself (invariant #6)"
    );
    assert!(
        certificate.produced_by.recipe_hash.is_none(),
        "the test is vacuous unless the recipe hash really is absent"
    );
}

/// The edge that was verified is still reported as verified. Capping
/// the *artifact* must not rewrite what was observed about its inputs,
/// or `freshdag why` would tell the user their dependency was
/// unchecked when a probe had in fact matched it.
#[test]
fn capping_for_recipe_identity_does_not_discard_edge_evidence() {
    let events: Vec<IrEvent> = Vec::new();
    let certificate = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::EngineAssembled,
        BTreeSet::new(),
        vec![matching_edge()],
        None,
    ))
    .expect("caps rather than refusing");

    assert_eq!(
        certificate.depends_on.len(),
        1,
        "the observed dependency survives the cap"
    );
    assert!(
        !certificate
            .status
            .reasons
            .iter()
            .any(|r| !r.reason.is_artifact_scoped()),
        "no edge-scoped reason was invented for an edge that matched"
    );
}

/// `Stale` does not require a recipe hash, and drift is positive
/// evidence the artifact is out of date. A missing recipe identity must
/// not launder that into `unknown`.
#[test]
fn a_stale_status_without_a_recipe_hash_stays_stale() {
    let events: Vec<IrEvent> = Vec::new();
    let certificate = seal(input(
        &events,
        coverage("freshdag-adapter-claude"),
        CoverageAuthority::EngineAssembled,
        BTreeSet::new(),
        vec![drifted_edge()],
        None,
    ))
    .expect("stale seals without a recipe hash");

    assert_eq!(certificate.status.value, ValidityStatus::Stale);
    assert!(
        !certificate
            .status
            .reasons
            .iter()
            .any(|r| r.reason == ReasonCode::RecipeIdentityUnavailable),
        "a stale artifact is not capped, so it gets no cap reason"
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
