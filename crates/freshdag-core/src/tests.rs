//! S1 domain-model tests.
//!
//! Covers: serde round-trips, trust-class distinctions, unknown
//! propagation via `Validity::aggregate`, illegal-state prevention via
//! `Certificate::check_invariants`, schema examples matching the
//! certificate schema.

use time::OffsetDateTime;

use crate::artifact::{Artifact, ArtifactId};
use crate::certificate::{
    Certificate, Comparator, CoverageEntry, InvariantError, ProducedBy, Status,
    CERTIFICATE_SCHEMA_V0_1,
};
use crate::computation::ComputationId;
use crate::dependency::{
    exact_file, Dependency, DependencyId, EdgeVerdict, Fingerprint, FingerprintKind,
    FingerprintParseError, TrustClass, Validity, ValidityAggregationError, ValidityReason,
    ValidityStatus,
};
use crate::ir::{Hash, HashAlgo};

// --------------------------------------------------------------------
// helpers
// --------------------------------------------------------------------

fn ts() -> OffsetDateTime {
    OffsetDateTime::parse(
        "2026-08-15T13:45:14.220000000Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
}

fn sample_blake3() -> Hash {
    Hash::new(HashAlgo::Blake3, "a".repeat(64)).unwrap()
}

fn other_blake3() -> Hash {
    Hash::new(HashAlgo::Blake3, "b".repeat(64)).unwrap()
}

// --------------------------------------------------------------------
// DependencyId
// --------------------------------------------------------------------

#[test]
fn dependency_id_from_scheme_key_is_stable() {
    let a = DependencyId::from_scheme_key("file", "file:///abs/foo.md");
    let b = DependencyId::from_scheme_key("file", "file:///abs/foo.md");
    assert_eq!(a, b);
    assert_eq!(a.0, "file:///abs/foo.md");

    // Bare key without the scheme prefix — prepends it.
    let c = DependencyId::from_scheme_key("attio", "company/acme");
    assert_eq!(c.0, "attio://company/acme");
}

// --------------------------------------------------------------------
// TrustClass — rank, escalation, demotion
// --------------------------------------------------------------------

#[test]
fn trust_class_rank_ordering() {
    assert!(TrustClass::Exact > TrustClass::Versioned);
    assert!(TrustClass::Versioned > TrustClass::Heuristic);
    assert!(TrustClass::Heuristic > TrustClass::Volatile);
}

#[test]
fn trust_class_escalation_and_demotion() {
    assert!(TrustClass::Versioned.is_escalation_over(TrustClass::Heuristic));
    assert!(TrustClass::Exact.is_escalation_over(TrustClass::Versioned));
    assert!(!TrustClass::Heuristic.is_escalation_over(TrustClass::Versioned));

    assert!(TrustClass::Heuristic.is_demotion_from(TrustClass::Versioned));
    assert!(TrustClass::Volatile.is_demotion_from(TrustClass::Heuristic));
    assert!(!TrustClass::Exact.is_demotion_from(TrustClass::Versioned));
}

#[test]
fn trust_class_wire_form() {
    for (variant, wire) in [
        (TrustClass::Exact, "exact"),
        (TrustClass::Versioned, "versioned"),
        (TrustClass::Heuristic, "heuristic"),
        (TrustClass::Volatile, "volatile"),
    ] {
        assert_eq!(
            serde_json::to_string(&variant).unwrap(),
            format!("\"{wire}\"")
        );
        let back: TrustClass = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
        assert_eq!(back, variant);
    }
}

// --------------------------------------------------------------------
// Fingerprint — parse, round-trip, unknown-not-a-fingerprint
// --------------------------------------------------------------------

#[test]
fn fingerprint_wire_forms() {
    let cases: &[(&str, FingerprintKind)] = &[
        (
            "blake3:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            FingerprintKind::ContentHash,
        ),
        (
            "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            FingerprintKind::ContentHash,
        ),
        ("version:42", FingerprintKind::Version),
        ("etag:\"abc123\"", FingerprintKind::Etag),
        ("mtime:1690000000", FingerprintKind::Mtime),
        ("custom:some-scheme-specific-thing", FingerprintKind::Custom),
    ];
    for (wire, expected_kind) in cases {
        let fp: Fingerprint = wire.parse().unwrap_or_else(|e| panic!("{wire}: {e}"));
        assert_eq!(fp.kind, *expected_kind);
        assert_eq!(fp.to_string(), *wire);
        // JSON round-trip. Compare via decode, not string form — the
        // ETag case contains embedded double quotes which JSON escapes.
        let json = serde_json::to_string(&fp).unwrap();
        let back: Fingerprint = serde_json::from_str(&json).unwrap();
        assert_eq!(back, fp);
    }
}

#[test]
fn fingerprint_empty_is_invariant_7_violation() {
    let err = "".parse::<Fingerprint>().unwrap_err();
    assert!(matches!(err, FingerprintParseError::Empty));
}

#[test]
fn fingerprint_missing_prefix_rejected() {
    let err = "abcdef".parse::<Fingerprint>().unwrap_err();
    assert!(matches!(err, FingerprintParseError::Malformed(_)));
}

#[test]
fn fingerprint_unknown_token_rejected() {
    // "unknown" as a fingerprint payload is exactly the failure mode
    // invariant #7 forbids: it looks like a value, but it means absence.
    let err = "unknown:something".parse::<Fingerprint>().unwrap_err();
    assert!(matches!(
        err,
        FingerprintParseError::UnknownIsNotFingerprint
    ));
}

#[test]
fn fingerprint_kind_survives_serialization() {
    // Two fingerprints with different kinds but the same string payload
    // are distinct (compare via full wire form).
    let v = Fingerprint::new(FingerprintKind::Version, "42");
    let e = Fingerprint::new(FingerprintKind::Etag, "42");
    assert_ne!(v.to_string(), e.to_string());
    assert_eq!(v.to_string(), "version:42");
    assert_eq!(e.to_string(), "etag:42");
}

// --------------------------------------------------------------------
// Dependency — construction, consistency, produced_by
// --------------------------------------------------------------------

#[test]
fn exact_file_dependency_construction() {
    let dep = exact_file("/abs/path/ICP.md", &sample_blake3(), ts());
    assert_eq!(dep.scheme, "file");
    assert_eq!(dep.key, "file:///abs/path/ICP.md");
    assert_eq!(dep.trust_class, TrustClass::Exact);
    assert!(dep.ttl_seconds.is_none());
    assert!(dep.produced_by.is_none());
    assert!(dep.is_consistent());
}

#[test]
fn volatile_dependency_without_ttl_is_inconsistent() {
    let dep = Dependency {
        key: "web.search://q=acme".to_string(),
        scheme: "web.search".to_string(),
        trust_class: TrustClass::Volatile,
        fingerprint: Fingerprint::new(FingerprintKind::Custom, "abc"),
        observed_at: ts(),
        produced_by: None,
        ttl_seconds: None,
    };
    assert!(!dep.is_consistent());
}

#[test]
fn dependency_round_trip_with_produced_by() {
    let upstream = ArtifactId("blake3:cafe".to_string());
    let dep = Dependency {
        key: "artifact:acme-brief.md".to_string(),
        scheme: "artifact".to_string(),
        trust_class: TrustClass::Exact,
        fingerprint: Fingerprint::new(FingerprintKind::ContentHash, "blake3:xxx".to_string()),
        observed_at: ts(),
        produced_by: Some(upstream.clone()),
        ttl_seconds: None,
    };
    let json = serde_json::to_string(&dep).unwrap();
    let back: Dependency = serde_json::from_str(&json).unwrap();
    assert_eq!(back, dep);
    assert_eq!(back.produced_by, Some(upstream));
}

// --------------------------------------------------------------------
// Validity — the aggregation table (invariant #7 keystone)
// --------------------------------------------------------------------

#[test]
fn validity_all_exact_match_is_valid() {
    let verdicts = [
        EdgeVerdict::Match(TrustClass::Exact),
        EdgeVerdict::Match(TrustClass::Versioned),
    ];
    let keys = vec!["a".to_string(), "b".to_string()];
    let v = Validity::aggregate(&verdicts, &keys).unwrap();
    assert_eq!(v.value, ValidityStatus::Valid);
    assert!(v.reasons.is_empty(), "Valid must not carry reasons");
}

#[test]
fn validity_any_heuristic_caps_at_likely_valid() {
    let verdicts = [
        EdgeVerdict::Match(TrustClass::Exact),
        EdgeVerdict::Match(TrustClass::Heuristic),
    ];
    let keys = vec!["a".to_string(), "b".to_string()];
    let v = Validity::aggregate(&verdicts, &keys).unwrap();
    assert_eq!(v.value, ValidityStatus::LikelyValid);
    assert!(
        v.reasons.iter().any(|r| r.reason.contains("heuristic")),
        "LikelyValid must carry a reason naming the heuristic edge; got {:?}",
        v.reasons
    );
}

#[test]
fn validity_any_volatile_caps_at_likely_valid() {
    let verdicts = [
        EdgeVerdict::Match(TrustClass::Exact),
        EdgeVerdict::Match(TrustClass::Volatile),
    ];
    let keys = vec!["a".to_string(), "b".to_string()];
    let v = Validity::aggregate(&verdicts, &keys).unwrap();
    assert_eq!(v.value, ValidityStatus::LikelyValid);
}

#[test]
fn validity_any_drift_is_stale_regardless_of_others() {
    let verdicts = [
        EdgeVerdict::Match(TrustClass::Exact),
        EdgeVerdict::Drift,
        EdgeVerdict::Unknown,
    ];
    let keys = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let v = Validity::aggregate(&verdicts, &keys).unwrap();
    assert_eq!(v.value, ValidityStatus::Stale);
    // Non-Valid must carry non-empty reasons.
    assert!(!v.reasons.is_empty());
}

#[test]
fn validity_any_unknown_without_drift_is_unknown() {
    let verdicts = [EdgeVerdict::Match(TrustClass::Exact), EdgeVerdict::Unknown];
    let keys = vec!["a".to_string(), "b".to_string()];
    let v = Validity::aggregate(&verdicts, &keys).unwrap();
    assert_eq!(v.value, ValidityStatus::Unknown);
    // Invariant #7: Unknown MUST NOT be Valid.
    assert_ne!(v.value, ValidityStatus::Valid);
}

#[test]
fn validity_empty_verdicts_is_unknown_not_valid() {
    // No evidence at all: cannot be Valid. This is the "empty
    // dependency set" adversarial-review fixture (docs/EVALUATION.md).
    let v = Validity::aggregate(&[], &[]).unwrap();
    assert_eq!(v.value, ValidityStatus::Unknown);
    assert!(!v.reasons.is_empty());
}

#[test]
fn validity_mismatched_lengths_errors() {
    let v = Validity::aggregate(&[EdgeVerdict::Match(TrustClass::Exact)], &[]);
    assert!(matches!(
        v,
        Err(ValidityAggregationError::MismatchedLengths {
            verdicts: 1,
            keys: 0
        })
    ));
}

#[test]
fn validity_status_wire_form() {
    for (variant, wire) in [
        (ValidityStatus::Valid, "valid"),
        (ValidityStatus::LikelyValid, "likely-valid"),
        (ValidityStatus::Stale, "stale"),
        (ValidityStatus::Unknown, "unknown"),
    ] {
        assert_eq!(
            serde_json::to_string(&variant).unwrap(),
            format!("\"{wire}\""),
            "{variant:?} wire form mismatch"
        );
        let back: ValidityStatus = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
        assert_eq!(back, variant);
    }
}

// --------------------------------------------------------------------
// Artifact
// --------------------------------------------------------------------

#[test]
fn artifact_round_trip() {
    let art = Artifact {
        id: ArtifactId::from_hash(&sample_blake3()),
        path: Some("briefs/acme.md".to_string()),
        kind: "text/markdown".to_string(),
        content_hash: sample_blake3(),
        size: 4213,
    };
    let json = serde_json::to_string(&art).unwrap();
    let back: Artifact = serde_json::from_str(&json).unwrap();
    assert_eq!(back, art);
    assert_eq!(art.id.to_string(), sample_blake3().to_string());
}

// --------------------------------------------------------------------
// ComputationId — derivation is deterministic and adapter-independent
// --------------------------------------------------------------------

#[test]
fn computation_id_is_deterministic() {
    let a = ComputationId::derive("research-account", "icp.md,notes.md", "v1");
    let b = ComputationId::derive("research-account", "icp.md,notes.md", "v1");
    assert_eq!(a, b);
    assert!(a.0.starts_with("comp:"));
    assert_eq!(a.0.len(), "comp:".len() + 64);
}

#[test]
fn computation_id_differs_on_any_input_change() {
    let base = ComputationId::derive("r", "i", "v");
    assert_ne!(base, ComputationId::derive("r2", "i", "v"));
    assert_ne!(base, ComputationId::derive("r", "i2", "v"));
    assert_ne!(base, ComputationId::derive("r", "i", "v2"));
}

#[test]
fn computation_id_extracts_hash() {
    let id = ComputationId::derive("r", "i", "v");
    let h = id.as_hash().expect("well-formed id should yield a hash");
    assert_eq!(h.algo, HashAlgo::Blake3);
    assert_eq!(h.digest_hex.len(), 64);

    let bad = ComputationId("not-a-comp-id".to_string());
    assert!(bad.as_hash().is_none());
}

// --------------------------------------------------------------------
// Certificate — schema-shape round-trip + machine-checked invariants
// --------------------------------------------------------------------

fn valid_baseline_cert() -> Certificate {
    let art = Artifact {
        id: ArtifactId::from_hash(&sample_blake3()),
        path: Some("briefs/acme.md".to_string()),
        kind: "text/markdown".to_string(),
        content_hash: sample_blake3(),
        size: 4213,
    };
    let dep = exact_file("/abs/path/ICP.md", &other_blake3(), ts());
    Certificate {
        cert_id: sample_blake3(),
        schema: CERTIFICATE_SCHEMA_V0_1.to_string(),
        artifact: art,
        produced_by: ProducedBy {
            computation_id: ComputationId::derive("recipe", "inputs", "v1").0,
            recipe: Some("research-account".to_string()),
            recipe_hash: Some(sample_blake3()),
            adapter: "freshdag-adapter-claude/0.1.0".to_string(),
            started: ts(),
            ended: ts(),
        },
        depends_on: vec![dep],
        comparator: Some(Comparator {
            name: "exact".to_string(),
            config: None,
        }),
        status: Status {
            value: ValidityStatus::Valid,
            checked: ts(),
            reasons: vec![],
        },
        observation_coverage: vec![CoverageEntry {
            producer: "freshdag-adapter-claude".to_string(),
            version: "0.1.0".to_string(),
            known_limitations: vec![],
        }],
    }
}

#[test]
fn certificate_valid_baseline_passes_invariants() {
    let cert = valid_baseline_cert();
    cert.check_invariants().unwrap();
}

#[test]
fn certificate_round_trip() {
    let cert = valid_baseline_cert();
    let json = serde_json::to_string(&cert).unwrap();
    let back: Certificate = serde_json::from_str(&json).unwrap();
    assert_eq!(back, cert);
}

#[test]
fn certificate_wrong_schema_rejected() {
    let mut cert = valid_baseline_cert();
    cert.schema = "freshdag.certificate/v0.2".to_string();
    let err = cert.check_invariants().unwrap_err();
    assert!(matches!(err, InvariantError::SchemaMismatch(_)));
}

#[test]
fn certificate_valid_with_heuristic_dep_rejected() {
    // The invariant #7 keystone: no code path constructs a Valid cert
    // whose evidence is heuristic. Contract §Field Rules requires this
    // to be machine-checked.
    let mut cert = valid_baseline_cert();
    cert.depends_on[0].trust_class = TrustClass::Heuristic;
    cert.depends_on[0].fingerprint = Fingerprint::new(FingerprintKind::Etag, "\"weak\"");
    let err = cert.check_invariants().unwrap_err();
    match err {
        InvariantError::ValidWithLowerTrust {
            trust_class: TrustClass::Heuristic,
            ..
        } => {}
        other => panic!("expected ValidWithLowerTrust(Heuristic), got {other:?}"),
    }
}

#[test]
fn certificate_valid_with_volatile_dep_rejected() {
    let mut cert = valid_baseline_cert();
    cert.depends_on[0].trust_class = TrustClass::Volatile;
    cert.depends_on[0].ttl_seconds = Some(3600);
    let err = cert.check_invariants().unwrap_err();
    assert!(matches!(err, InvariantError::ValidWithLowerTrust { .. }));
}

#[test]
fn certificate_valid_without_recipe_hash_rejected() {
    let mut cert = valid_baseline_cert();
    cert.produced_by.recipe_hash = None;
    let err = cert.check_invariants().unwrap_err();
    assert!(matches!(err, InvariantError::MissingRecipeHash { .. }));
}

#[test]
fn certificate_stale_without_reasons_rejected() {
    let mut cert = valid_baseline_cert();
    cert.status.value = ValidityStatus::Stale;
    cert.status.reasons = vec![]; // MUST be non-empty for non-Valid
    let err = cert.check_invariants().unwrap_err();
    assert!(matches!(err, InvariantError::MissingReasons { .. }));
}

#[test]
fn certificate_stale_with_reasons_ok() {
    let mut cert = valid_baseline_cert();
    cert.status.value = ValidityStatus::Stale;
    cert.status.reasons = vec![ValidityReason {
        dependency_key: cert.depends_on[0].key.clone(),
        reason: "drift".to_string(),
    }];
    cert.check_invariants().unwrap();
}

#[test]
fn certificate_empty_coverage_rejected() {
    let mut cert = valid_baseline_cert();
    cert.observation_coverage = vec![];
    let err = cert.check_invariants().unwrap_err();
    assert!(matches!(err, InvariantError::EmptyObservationCoverage));
}

#[test]
fn certificate_naked_volatile_rejected() {
    let mut cert = valid_baseline_cert();
    cert.status.value = ValidityStatus::LikelyValid;
    cert.status.reasons = vec![ValidityReason {
        dependency_key: "web.search://q=x".to_string(),
        reason: "trust_class_volatile_caps_at_likely_valid".to_string(),
    }];
    cert.depends_on.push(Dependency {
        key: "web.search://q=x".to_string(),
        scheme: "web.search".to_string(),
        trust_class: TrustClass::Volatile,
        fingerprint: Fingerprint::new(FingerprintKind::Custom, "x"),
        observed_at: ts(),
        produced_by: None,
        ttl_seconds: None, // <-- naked
    });
    let err = cert.check_invariants().unwrap_err();
    assert!(matches!(err, InvariantError::NakedVolatile(_)));
}

#[test]
fn certificate_derive_cert_id_is_idempotent() {
    let mut cert = valid_baseline_cert();
    let id1 = cert.derive_cert_id().unwrap();
    cert.cert_id = id1.clone();
    let id2 = cert.derive_cert_id().unwrap();
    assert_eq!(id1, id2, "derive_cert_id must be idempotent");
}

#[test]
fn certificate_derive_cert_id_differs_on_content_change() {
    let cert = valid_baseline_cert();
    let mut variant = cert.clone();
    variant.artifact.size += 1;
    assert_ne!(
        cert.derive_cert_id().unwrap(),
        variant.derive_cert_id().unwrap()
    );
}

// --------------------------------------------------------------------
// Probe trait — result type distinctions
// --------------------------------------------------------------------

#[test]
fn probe_result_unknown_is_distinct_from_match() {
    use crate::probe::ProbeResult;

    let m = ProbeResult::Match {
        observed_fp: Fingerprint::new(FingerprintKind::Etag, "\"abc\""),
        observed_trust_class: TrustClass::Versioned,
    };
    let u = ProbeResult::Unknown {
        reason: "network down".to_string(),
        retryable: true,
    };
    // Serialize and confirm they are different JSON tags.
    let m_json = serde_json::to_string(&m).unwrap();
    let u_json = serde_json::to_string(&u).unwrap();
    assert!(m_json.contains("Match"));
    assert!(u_json.contains("Unknown"));
    assert_ne!(m_json, u_json);
}
