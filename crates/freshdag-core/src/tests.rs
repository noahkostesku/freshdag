//! S1 domain-model tests.
//!
//! Covers: serde round-trips, trust-class distinctions, unknown
//! propagation via `Validity::aggregate`, illegal-state prevention via
//! `Certificate::check_invariants`, schema examples matching the
//! certificate schema.

use std::collections::BTreeMap;

use time::OffsetDateTime;

use crate::artifact::{Artifact, ArtifactId};
use crate::certificate::{
    Certificate, Comparator, CoverageEntry, InvariantError, ProducedBy, ReasonMapping, Status,
    CERTIFICATE_SCHEMA_V0_1,
};
use crate::computation::ComputationId;
use crate::dependency::{
    exact_file, Dependency, DependencyId, EdgeVerdict, Fingerprint, FingerprintKind,
    FingerprintParseError, ReasonCode, TrustClass, Validity, ValidityAggregationError,
    ValidityReason, ValidityStatus,
};
use crate::ir::{
    EventKind, EventKindPattern, Hash, HashAlgo, PartialCoverage, PartialReason, ProducerRole,
    ALL_PARTIAL_REASONS,
};

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

#[test]
fn dependency_id_from_scheme_key_prefix_check_is_exact() {
    // Regression: previously used `key.starts_with(scheme)`, which
    // would drop the prefix for e.g. ("file", "filesystem/x.md")
    // because "filesystem" starts with "file". The check must include
    // the `:` or `://` delimiter.
    let dep = DependencyId::from_scheme_key("file", "filesystem/x.md");
    assert_eq!(dep.0, "file://filesystem/x.md");

    // `scheme:key` (single colon, no `//`) is also treated as already-formed.
    let dep = DependencyId::from_scheme_key("scheme", "scheme:opaque");
    assert_eq!(dep.0, "scheme:opaque");
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
fn fingerprint_unknown_rejected_case_insensitively() {
    // Regression: case-sensitive match would let `UNKNOWN:x`, `Unknown:x`,
    // etc. slip through as Custom fingerprints. Reject them all.
    for variant in ["UNKNOWN:x", "Unknown:x", "unKnown:x", "UNKNOWN:UNKNOWN"] {
        let err = variant.parse::<Fingerprint>().unwrap_err();
        assert!(
            matches!(err, FingerprintParseError::UnknownIsNotFingerprint),
            "{variant} must be rejected; got {err:?}"
        );
    }
}

#[test]
fn fingerprint_empty_prefix_rejected() {
    // Regression: `":something"` used to parse as Custom{payload:"something"}.
    // Empty prefix is malformed, not a Custom fingerprint.
    let err = ":something".parse::<Fingerprint>().unwrap_err();
    assert!(matches!(err, FingerprintParseError::Malformed(_)));
    // Same for double-colon at the start.
    let err = "::x".parse::<Fingerprint>().unwrap_err();
    assert!(matches!(err, FingerprintParseError::Malformed(_)));
}

#[test]
#[should_panic(expected = "empty Fingerprint payload violates invariant #7")]
fn fingerprint_new_with_empty_payload_panics_in_release() {
    // Regression: previously this was `debug_assert!` which vanished in
    // release builds. Now it is a plain `assert!` and panics
    // unconditionally.
    let _ = Fingerprint::new(FingerprintKind::Custom, "");
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
        EdgeVerdict::matched(
            TrustClass::Exact,
            Fingerprint::new(
                FingerprintKind::ContentHash,
                "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ),
        EdgeVerdict::matched(
            TrustClass::Versioned,
            Fingerprint::new(FingerprintKind::Version, "42"),
        ),
    ];
    let keys = vec!["a".to_string(), "b".to_string()];
    let v = Validity::aggregate(&verdicts, &keys).unwrap();
    assert_eq!(v.value, ValidityStatus::Valid);
    assert!(v.reasons.is_empty(), "Valid must not carry reasons");
}

#[test]
fn validity_any_heuristic_caps_at_likely_valid() {
    let verdicts = [
        EdgeVerdict::matched(
            TrustClass::Exact,
            Fingerprint::new(
                FingerprintKind::ContentHash,
                "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ),
        EdgeVerdict::matched(
            TrustClass::Heuristic,
            Fingerprint::new(FingerprintKind::Etag, "\"weak\""),
        ),
    ];
    let keys = vec!["a".to_string(), "b".to_string()];
    let v = Validity::aggregate(&verdicts, &keys).unwrap();
    assert_eq!(v.value, ValidityStatus::LikelyValid);
    assert!(
        v.reasons
            .iter()
            .any(|r| r.reason == ReasonCode::TrustClassHeuristicCapsAtLikelyValid),
        "LikelyValid must carry a reason naming the heuristic edge; got {:?}",
        v.reasons
    );
}

#[test]
fn validity_any_volatile_caps_at_likely_valid() {
    let verdicts = [
        EdgeVerdict::matched(
            TrustClass::Exact,
            Fingerprint::new(
                FingerprintKind::ContentHash,
                "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ),
        EdgeVerdict::matched(
            TrustClass::Volatile,
            Fingerprint::new(FingerprintKind::Custom, "vol"),
        ),
    ];
    let keys = vec!["a".to_string(), "b".to_string()];
    let v = Validity::aggregate(&verdicts, &keys).unwrap();
    assert_eq!(v.value, ValidityStatus::LikelyValid);
}

#[test]
fn validity_any_drift_is_stale_regardless_of_others() {
    let verdicts = [
        EdgeVerdict::matched(
            TrustClass::Exact,
            Fingerprint::new(
                FingerprintKind::ContentHash,
                "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ),
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
    let verdicts = [
        EdgeVerdict::matched(
            TrustClass::Exact,
            Fingerprint::new(
                FingerprintKind::ContentHash,
                "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ),
        EdgeVerdict::Unknown,
    ];
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
    let v = Validity::aggregate(
        &[EdgeVerdict::matched(
            TrustClass::Exact,
            Fingerprint::new(
                FingerprintKind::ContentHash,
                "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        )],
        &[],
    );
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
// ReasonCode — the closed reason set (invariants #6, #13)
// --------------------------------------------------------------------

/// Every `ReasonCode` variant, enumerated by hand. Adding a variant
/// without adding it here fails `reason_code_enumeration_is_complete`.
const ALL_REASON_CODES: &[ReasonCode] = &[
    ReasonCode::Drift,
    ReasonCode::ProbeUnknown,
    ReasonCode::NoProbeAvailable,
    ReasonCode::TrustClassHeuristicCapsAtLikelyValid,
    ReasonCode::TrustClassVolatileCapsAtLikelyValid,
    ReasonCode::TtlExpired,
    ReasonCode::ProbeTrustDemoted,
    ReasonCode::CoverageDeficit,
    ReasonCode::ProducerMissingFromCoverage,
    ReasonCode::NoDependenciesObserved,
    ReasonCode::VolatileWithinTtlUnprobed,
    ReasonCode::DependencyChangedDuringComputation,
    ReasonCode::RecipeIdentityUnavailable,
    ReasonCode::UnprovenDependency,
];

#[test]
fn reason_code_wire_form_is_exact() {
    for (variant, wire) in [
        (
            ReasonCode::RecipeIdentityUnavailable,
            "recipe-identity-unavailable",
        ),
        (ReasonCode::UnprovenDependency, "unproven-dependency"),
        (ReasonCode::Drift, "drift"),
        (ReasonCode::ProbeUnknown, "probe-unknown"),
        (ReasonCode::NoProbeAvailable, "no-probe-available"),
        (
            ReasonCode::TrustClassHeuristicCapsAtLikelyValid,
            "trust-class-heuristic-caps-at-likely-valid",
        ),
        (
            ReasonCode::TrustClassVolatileCapsAtLikelyValid,
            "trust-class-volatile-caps-at-likely-valid",
        ),
        (ReasonCode::TtlExpired, "ttl-expired"),
        (ReasonCode::ProbeTrustDemoted, "probe-trust-demoted"),
        (ReasonCode::CoverageDeficit, "coverage-deficit"),
        (
            ReasonCode::ProducerMissingFromCoverage,
            "producer-missing-from-coverage",
        ),
        (
            ReasonCode::NoDependenciesObserved,
            "no-dependencies-observed",
        ),
        (
            ReasonCode::VolatileWithinTtlUnprobed,
            "volatile-within-ttl-unprobed",
        ),
        (
            ReasonCode::DependencyChangedDuringComputation,
            "dependency-changed-during-computation",
        ),
    ] {
        assert_eq!(
            serde_json::to_string(&variant).unwrap(),
            format!("\"{wire}\""),
            "{variant:?} serde wire form mismatch"
        );
        assert_eq!(variant.as_wire_str(), wire, "{variant:?} as_wire_str drift");
        assert_eq!(variant.to_string(), wire, "{variant:?} Display drift");
        let back: ReasonCode = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
        assert_eq!(back, variant);
    }
}

#[test]
fn reason_code_serde_and_as_wire_str_agree() {
    // The load-bearing sync check: `as_wire_str` is what Rust callers
    // and error messages use; serde is what the certificate schema
    // sees. They must never diverge.
    for code in ALL_REASON_CODES {
        assert_eq!(
            serde_json::to_string(code).unwrap(),
            format!("\"{}\"", code.as_wire_str()),
            "{code:?}: as_wire_str disagrees with serde"
        );
    }
}

#[test]
fn reason_code_enumeration_is_complete() {
    // NOTE: this does NOT guard what its name suggests. A variant added
    // to the enum but omitted from ALL_REASON_CODES leaves this count
    // untouched, and `schema_reason_enums_match_rust` compares the
    // schemas against ALL_REASON_CODES rather than against the enum —
    // so an omission at the top of the chain hides the whole chain. A
    // verifier demonstrated this with a fourteenth variant that passed
    // the entire suite. The real guard is the compiler: the exhaustive
    // `match` in freshdag-cli's `prose()` will not build. What this
    // assertion does catch is a duplicate or accidental deletion inside
    // ALL_REASON_CODES itself.
    assert_eq!(ALL_REASON_CODES.len(), 14);
    let mut wires: Vec<&str> = ALL_REASON_CODES.iter().map(|c| c.as_wire_str()).collect();
    wires.sort_unstable();
    wires.dedup();
    assert_eq!(
        wires.len(),
        ALL_REASON_CODES.len(),
        "duplicate wire strings"
    );
}

#[test]
fn reason_code_scopes_are_as_documented() {
    for code in ALL_REASON_CODES {
        let expected_artifact_scoped = matches!(
            code,
            ReasonCode::CoverageDeficit
                | ReasonCode::ProducerMissingFromCoverage
                | ReasonCode::NoDependenciesObserved
                | ReasonCode::RecipeIdentityUnavailable
                | ReasonCode::UnprovenDependency
        );
        assert_eq!(
            code.is_artifact_scoped(),
            expected_artifact_scoped,
            "{code:?} scope disagrees with the certificate-contract table"
        );
    }
}

// --------------------------------------------------------------------
// Schema/type agreement — the vocabulary lives in three places
// (Rust serde, certificate schema, scenario schema) and they MUST NOT
// drift. `CoverageEntry.emits` previously existed in Rust but was
// absent from the certificate schema; this test class exists so that
// class of drift cannot recur silently.
// --------------------------------------------------------------------

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read_schema(rel: &str) -> serde_json::Value {
    let path = repo_root().join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel} is not valid JSON: {e}"))
}

/// Pull an `enum` array out of a schema at a `/`-delimited pointer path
/// and return it as a sorted, deduplicated string set.
fn schema_enum_at(schema: &serde_json::Value, pointer: &str) -> Vec<String> {
    let node = schema
        .pointer(pointer)
        .unwrap_or_else(|| panic!("schema pointer `{pointer}` not found"));
    let arr = node
        .as_array()
        .unwrap_or_else(|| panic!("schema pointer `{pointer}` is not an array"));
    let mut out: Vec<String> = arr
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("non-string enum member at `{pointer}`"))
                .to_string()
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn rust_reason_wire_set() -> Vec<String> {
    let mut out: Vec<String> = ALL_REASON_CODES
        .iter()
        .map(|c| c.as_wire_str().to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn schema_reason_enums_match_rust() {
    let rust = rust_reason_wire_set();
    assert_eq!(rust.len(), ALL_REASON_CODES.len());

    let cert = read_schema("schemas/certificate/v0.1.json");
    assert_eq!(
        schema_enum_at(
            &cert,
            "/properties/status/properties/reasons/items/properties/reason/enum"
        ),
        rust,
        "certificate schema `status.reasons[].reason` enum disagrees with ReasonCode"
    );

    let scenario = read_schema("schemas/scenario/v0.1.json");
    assert_eq!(
        schema_enum_at(
            &scenario,
            "/properties/expected/properties/certificate_status/properties/reason_codes/items/enum"
        ),
        rust,
        "scenario schema `certificate_status.reason_codes[]` enum disagrees with ReasonCode"
    );
    assert_eq!(
        schema_enum_at(
            &scenario,
            "/properties/expected/properties/invalidation/properties/after_mutation_reason_codes/items/enum"
        ),
        rust,
        "scenario schema `invalidation.after_mutation_reason_codes[]` enum disagrees with ReasonCode"
    );
}

#[test]
fn schema_partial_reason_enum_matches_rust() {
    // Same guard as `schema_reason_enums_match_rust`, one layer down:
    // a `PartialReason` variant that exists in Rust but not in the
    // schema (or vice versa) means a producer and a third-party
    // re-checker disagree about whether a certificate is readable.
    let mut rust: Vec<String> = ALL_PARTIAL_REASONS
        .iter()
        .map(|r| r.as_wire_str().to_string())
        .collect();
    rust.sort();
    rust.dedup();
    assert_eq!(rust.len(), ALL_PARTIAL_REASONS.len());

    let cert = read_schema("schemas/certificate/v0.1.json");
    assert_eq!(
        schema_enum_at(
            &cert,
            "/properties/observation_coverage/items/properties/partial/\
             additionalProperties/oneOf/1/properties/reason/enum"
        ),
        rust,
        "certificate schema `observation_coverage[].partial.*.reason` enum \
         disagrees with PartialReason"
    );
}

/// A `CoverageEntry` that omits `partial` must FAIL TO DECODE.
///
/// This is the fail-open hole an `architect` review found in the ADR
/// 0011 migration. `partial` carried `#[serde(default)]`, so an absent
/// map decoded as "this producer declared no blindness" — the
/// permissive answer. A certificate with `role: observer`, `fs.read` in
/// `emits`, and no `partial` therefore discharged the `bash`/`task`
/// obligation and re-checked `valid`: precisely the defect ADR 0011
/// exists to close, surviving on the third-party-recheck surface
/// `docs/NOVELTY.md §2` rests the wedge on.
///
/// Loud failure is the point. A tool whose job is to say "I cannot
/// prove this" must not answer `valid` about a document it could not
/// fully read.
#[test]
fn a_coverage_entry_without_partial_does_not_decode() {
    let no_partial = serde_json::json!({
        "producer": "freshdag-observer-example",
        "version": "0.1.0",
        "role": "observer",
        "emits": ["fs.read", "fs.write"]
    });
    let err = serde_json::from_value::<CoverageEntry>(no_partial.clone())
        .expect_err("an entry with no `partial` must not decode");
    assert!(
        err.to_string().contains("partial"),
        "the error must name the missing field: {err}"
    );

    // The same entry decodes once it says so out loud, and an empty map
    // remains the way to declare no partiality.
    let mut declared = no_partial;
    declared["partial"] = serde_json::json!({});
    let entry: CoverageEntry =
        serde_json::from_value(declared).expect("an explicit empty map decodes");
    assert!(
        entry.discharges_subprocess_obligation(),
        "an observer that explicitly declares no gaps still discharges"
    );
}

/// The permissive decode, demonstrated end to end on a certificate.
///
/// Guards the property rather than the field: whatever `partial`'s
/// representation becomes, a document that never mentions a producer's
/// blindness must not yield a reusable verdict.
#[test]
fn a_certificate_hiding_its_producers_blindness_is_unreadable() {
    let raw = std::fs::read_to_string(
        repo_root()
            .join("fixtures/certificate-conformance/positive/exact-dep-valid/certificate.json"),
    )
    .expect("fixture readable");
    let mut value: serde_json::Value = serde_json::from_str(&raw).expect("fixture parses");

    // Strip every `partial` — the shape a pre-ADR-0011 document has.
    for entry in value["observation_coverage"]
        .as_array_mut()
        .expect("coverage array")
    {
        entry
            .as_object_mut()
            .expect("entry object")
            .remove("partial");
    }

    assert!(
        serde_json::from_value::<Certificate>(value).is_err(),
        "a certificate whose producers never declare their blindness must \
         fail to decode rather than re-check as though they were faithful"
    );
}

#[test]
fn certificate_schema_accepts_the_legacy_bare_string_partial() {
    // The migration shape must stay expressible in the schema, or old
    // certificates become unreadable rather than conservatively read.
    let cert = read_schema("schemas/certificate/v0.1.json");
    let one_of = cert
        .pointer(
            "/properties/observation_coverage/items/properties/partial/additionalProperties/oneOf",
        )
        .and_then(serde_json::Value::as_array)
        .expect("partial.additionalProperties.oneOf");
    assert_eq!(one_of.len(), 2);
    assert_eq!(
        one_of[0].pointer("/type").and_then(|v| v.as_str()),
        Some("string")
    );
    assert_eq!(
        one_of[1].pointer("/type").and_then(|v| v.as_str()),
        Some("object")
    );

    // `reason` is required in the object form; `note` is not.
    let required = one_of[1]
        .pointer("/required")
        .and_then(serde_json::Value::as_array)
        .expect("object form required list");
    let required: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(required, vec!["reason"]);
}

#[test]
fn certificate_schema_declares_detail_and_emits() {
    // Regression guards for the two properties this wave added to the
    // schema to close pre-existing Rust/schema drift.
    let cert = read_schema("schemas/certificate/v0.1.json");
    assert!(
        cert.pointer("/properties/status/properties/reasons/items/properties/detail")
            .is_some(),
        "certificate schema is missing status.reasons[].detail"
    );
    assert!(
        cert.pointer("/properties/observation_coverage/items/properties/emits")
            .is_some(),
        "certificate schema is missing observation_coverage[].emits \
         (CoverageEntry.emits exists in Rust)"
    );
    // `dependency_key` and `reason` stay required; `detail` does not.
    let required = cert
        .pointer("/properties/status/properties/reasons/items/required")
        .and_then(serde_json::Value::as_array)
        .expect("reasons[].required");
    let required: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(required, vec!["dependency_key", "reason"]);
}

#[test]
fn validity_reason_omits_detail_when_none() {
    let r = ValidityReason {
        dependency_key: "file:///repo/notes.md".to_string(),
        reason: ReasonCode::Drift,
        detail: None,
    };
    let json = serde_json::to_string(&r).unwrap();
    assert_eq!(
        json, r#"{"dependency_key":"file:///repo/notes.md","reason":"drift"}"#,
        "detail: None must not emit a `detail` key"
    );
    let back: ValidityReason = serde_json::from_str(&json).unwrap();
    assert_eq!(back, r);
}

#[test]
fn validity_reason_round_trips_with_detail() {
    let r = ValidityReason {
        dependency_key: "https://acme.com/pricing".to_string(),
        reason: ReasonCode::ProbeUnknown,
        detail: Some("429 Too Many Requests".to_string()),
    };
    let json = serde_json::to_string(&r).unwrap();
    assert!(
        json.contains(r#""detail":"429 Too Many Requests""#),
        "{json}"
    );
    let back: ValidityReason = serde_json::from_str(&json).unwrap();
    assert_eq!(back, r);
    assert_eq!(back.detail.as_deref(), Some("429 Too Many Requests"));
}

#[test]
fn validity_reason_rejects_unknown_reason_code() {
    // The whole point of closing the set: an unrecognized code is a
    // hard parse failure, not a silently-accepted string.
    let err = serde_json::from_str::<ValidityReason>(
        r#"{"dependency_key":"x","reason":"not-a-real-code"}"#,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("unknown variant"),
        "expected unknown-variant error, got: {err}"
    );
}

#[test]
fn validity_reason_rejects_legacy_snake_case_codes() {
    // Wire-form migration guarantee: the pre-Wave-2 snake_case forms
    // are no longer accepted. A stale certificate fails loudly rather
    // than deserializing into something misleading.
    for legacy in [
        "probe_unknown",
        "trust_class_heuristic_caps_at_likely_valid",
        "trust_class_volatile_caps_at_likely_valid",
        "no_dependencies_observed",
        "coverage_deficit",
    ] {
        let json = format!(r#"{{"dependency_key":"x","reason":"{legacy}"}}"#);
        assert!(
            serde_json::from_str::<ValidityReason>(&json).is_err(),
            "legacy reason `{legacy}` must no longer deserialize"
        );
    }
}

#[test]
fn aggregate_emits_typed_reason_codes() {
    // Construction sites in Validity::aggregate must produce typed
    // codes with no detail (the aggregator has no extra context).
    let v = Validity::aggregate(&[], &[]).unwrap();
    assert_eq!(v.value, ValidityStatus::Unknown);
    assert_eq!(v.reasons.len(), 1);
    assert_eq!(v.reasons[0].reason, ReasonCode::NoDependenciesObserved);
    assert_eq!(v.reasons[0].detail, None);

    let verdicts = [
        EdgeVerdict::Drift,
        EdgeVerdict::Unknown,
        EdgeVerdict::matched(
            TrustClass::Heuristic,
            Fingerprint::new(FingerprintKind::Etag, "\"w\""),
        ),
        EdgeVerdict::matched(
            TrustClass::Volatile,
            Fingerprint::new(FingerprintKind::Custom, "v"),
        ),
    ];
    let keys = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ];
    let v = Validity::aggregate(&verdicts, &keys).unwrap();
    assert_eq!(v.value, ValidityStatus::Stale);
    let codes: Vec<ReasonCode> = v.reasons.iter().map(|r| r.reason).collect();
    assert_eq!(
        codes,
        vec![
            ReasonCode::Drift,
            ReasonCode::ProbeUnknown,
            ReasonCode::TrustClassHeuristicCapsAtLikelyValid,
            ReasonCode::TrustClassVolatileCapsAtLikelyValid,
        ]
    );
    assert!(v.reasons.iter().all(|r| r.detail.is_none()));
}

#[test]
fn invariant_error_reason_mapping_downgrade_arm() {
    // The two coverage variants describe a real evidence gap and map
    // to codes the engine can put on a certificate.
    assert_eq!(
        InvariantError::CoverageDeficit {
            tool_kind: "bash".to_string(),
        }
        .reason_mapping(),
        ReasonMapping::Downgrade(ReasonCode::CoverageDeficit)
    );
    assert_eq!(
        InvariantError::ProducerMissingFromCoverage {
            producer: "ghost".to_string(),
        }
        .reason_mapping(),
        ReasonMapping::Downgrade(ReasonCode::ProducerMissingFromCoverage)
    );
}

#[test]
fn invariant_error_reason_mapping_structural_defect_arm() {
    // Everything else is a malformed-certificate defect: the caller
    // must emit NOTHING, not downgrade to unknown. Downgrading would
    // hide a producer bug behind a legitimate-looking result.
    for err in [
        InvariantError::SchemaMismatch("nope".to_string()),
        InvariantError::ValidWithLowerTrust {
            dependency_key: "k".to_string(),
            trust_class: TrustClass::Heuristic,
        },
        InvariantError::MissingRecipeHash {
            status: ValidityStatus::Valid,
        },
        InvariantError::MissingReasons {
            status: ValidityStatus::Stale,
        },
        InvariantError::NakedVolatile("k".to_string()),
        InvariantError::EmptyObservationCoverage,
    ] {
        assert_eq!(
            err.reason_mapping(),
            ReasonMapping::StructuralDefect,
            "{err:?} is a structural defect and must not map to a ReasonCode"
        );
    }
}

#[test]
fn artifact_scoped_reasons_use_empty_key_sentinel_and_sort_last() {
    // Sentinel: artifact-scoped reasons carry "" as dependency_key —
    // not null, not omitted — so the schema `required` list holds.
    let v = Validity::aggregate(&[], &[]).unwrap();
    assert_eq!(v.reasons[0].dependency_key, "");
    assert!(v.reasons[0].reason.is_artifact_scoped());
    let json = serde_json::to_value(&v.reasons[0]).unwrap();
    assert_eq!(
        json.get("dependency_key").and_then(|k| k.as_str()),
        Some("")
    );

    // Ordering: edge-scoped reasons keep depends_on[] order; any
    // artifact-scoped reason sorts after all of them. `cert_id` hashes
    // this list, so the order is wire-visible.
    let mut reasons = [
        ValidityReason {
            dependency_key: String::new(),
            reason: ReasonCode::CoverageDeficit,
            detail: None,
        },
        ValidityReason {
            dependency_key: "a".to_string(),
            reason: ReasonCode::Drift,
            detail: None,
        },
        ValidityReason {
            dependency_key: "b".to_string(),
            reason: ReasonCode::ProbeUnknown,
            detail: None,
        },
    ];
    reasons.sort_by_key(|r| r.reason.is_artifact_scoped());
    let keys: Vec<&str> = reasons.iter().map(|r| r.dependency_key.as_str()).collect();
    assert_eq!(keys, vec!["a", "b", ""]);
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
            computation_id: ComputationId::derive("recipe", "inputs", "v1"),
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
            role: ProducerRole::Adapter,
            emits: vec![],
            partial: BTreeMap::new(),
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
        reason: ReasonCode::Drift,
        detail: None,
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
        reason: ReasonCode::TrustClassVolatileCapsAtLikelyValid,
        detail: None,
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

// --------------------------------------------------------------------
// Certificate::check_coverage_deficit — the invariant #7 keystone at
// the boundary between adapter+observer producers and the certificate.
// --------------------------------------------------------------------

fn bash_tool_invoked_event(producer: &str) -> crate::ir::IrEvent {
    crate::ir::IrEvent {
        event_id: uuid::Uuid::parse_str("018f5b52-4b8b-7a1a-9c2f-1a2b3c4d5e6f").unwrap(),
        producer: producer.to_string(),
        producer_version: "0.1.0".to_string(),
        session_id: "s".to_string(),
        computation_id: None,
        parent_id: None,
        causal_inputs: None,
        ts: ts(),
        kind: crate::ir::EventKind::ToolInvoked,
        payload: serde_json::json!({
            "tool_name": "bash",
            "tool_kind": "bash",
            "tool_input": { "command": "cat notes.md" },
        }),
    }
}

#[test]
fn coverage_deficit_flags_valid_cert_with_bash_and_no_fs_observer() {
    // Valid cert; bash tool.invoked; observation_coverage has ONLY the
    // adapter (no observer producer, no fs.* coverage). Contract
    // §Coverage-Deficit demands CoverageDeficit.
    let cert = valid_baseline_cert();
    let events = vec![bash_tool_invoked_event("freshdag-adapter-claude")];
    let err = cert.check_coverage_deficit(&events).unwrap_err();
    match err {
        InvariantError::CoverageDeficit { tool_kind } => assert_eq!(tool_kind, "bash"),
        other => panic!("expected CoverageDeficit(bash), got {other:?}"),
    }
}

fn fs_entry(producer: &str, role: ProducerRole) -> CoverageEntry {
    CoverageEntry {
        producer: producer.to_string(),
        version: "0.1.0".to_string(),
        role,
        emits: vec![
            crate::ir::EventKindPattern::from("fs.read"),
            crate::ir::EventKindPattern::from("fs.write"),
        ],
        partial: BTreeMap::new(),
        known_limitations: vec![],
    }
}

#[test]
fn coverage_deficit_passes_when_observer_declares_fs_coverage() {
    // Mirror case: an Observer-role producer with fs.* DOES discharge
    // the bash obligation. Proves the rule is specific, not merely
    // strict.
    let mut cert = valid_baseline_cert();
    cert.observation_coverage.push(fs_entry(
        "freshdag-observer-fsatrace",
        ProducerRole::Observer,
    ));
    let events = vec![bash_tool_invoked_event("freshdag-adapter-claude")];
    cert.check_coverage_deficit(&events).unwrap();
}

#[test]
fn coverage_deficit_adapter_fs_claim_does_not_discharge_bash_obligation() {
    // Inverted characterization test for the invariant-#7 hole found by
    // the observer workstream. An adapter declaring fs.read/fs.write —
    // exactly what adapter-contract §Responsibilities #4's canonical
    // manifest example does — plus a zero-coverage StubObserver used to
    // return Ok(()), certifying `valid` behind ZERO subprocess
    // observation. Adapters synthesize fs events from tool inputs they
    // can see and are blind inside subprocesses by construction, so
    // only an Observer discharges the obligation.
    let mut cert = valid_baseline_cert();
    cert.observation_coverage = vec![
        fs_entry("freshdag-adapter-claude", ProducerRole::Adapter),
        CoverageEntry {
            producer: "freshdag-observer-stub".to_string(),
            version: "0.1.0".to_string(),
            role: ProducerRole::Observer,
            emits: vec![], // StubObserver: declares nothing.
            partial: BTreeMap::new(),
            known_limitations: vec![],
        },
    ];
    let events = vec![bash_tool_invoked_event("freshdag-adapter-claude")];
    let err = cert.check_coverage_deficit(&events).unwrap_err();
    match err {
        InvariantError::CoverageDeficit { tool_kind } => assert_eq!(tool_kind, "bash"),
        other => panic!("expected CoverageDeficit(bash), got {other:?}"),
    }
}

#[test]
fn coverage_deficit_observer_discharges_even_alongside_adapter_fs_claim() {
    // Both present: the observer discharges the obligation and the
    // adapter's fs.* claim is simply irrelevant either way.
    let mut cert = valid_baseline_cert();
    cert.observation_coverage = vec![
        fs_entry("freshdag-adapter-claude", ProducerRole::Adapter),
        fs_entry("freshdag-observer-fsatrace", ProducerRole::Observer),
    ];
    let events = vec![bash_tool_invoked_event("freshdag-adapter-claude")];
    cert.check_coverage_deficit(&events).unwrap();
}

// --------------------------------------------------------------------
// ADR 0011 — the certificate carries `partial`, and the discharge rule
// reads it. Each of these is a certificate that used to say `valid`.
// --------------------------------------------------------------------

#[test]
fn coverage_entry_carries_partial_from_the_manifest() {
    // `From<&CoverageManifest>` used to drop `partial`, which made the
    // coverage-deficit rule uncheckable from the certificate alone: the
    // one fact that flips the verdict was not in the document.
    let mut partial = BTreeMap::new();
    partial.insert(
        "fs.read".to_string(),
        PartialCoverage::new(PartialReason::BlindInScope, "no subprocess visibility"),
    );
    let manifest = crate::ir::CoverageManifest {
        producer: "freshdag-observer-blind".to_string(),
        version: "0.1.0".to_string(),
        role: ProducerRole::Observer,
        platforms: vec![],
        emits: vec![EventKindPattern::from("fs.read")],
        partial,
        capabilities: BTreeMap::new(),
        known_limitations: vec![],
    };
    let entry = CoverageEntry::from(&manifest);
    assert_eq!(entry.partial, manifest.partial);

    // And it survives the wire, so a third party re-checking the
    // certificate on another machine reaches the same verdict.
    let json = serde_json::to_string(&entry).unwrap();
    let back: CoverageEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back, entry);
    assert!(!back.discharges_subprocess_obligation());
}

#[test]
fn coverage_deficit_blind_observer_does_not_discharge_bash_obligation() {
    // The verifier's reproduction (ADR 0011 §Context): two stores
    // identical except the observer's `partial` map both returned
    // "safe to reuse". An observer that declares itself blind to
    // subprocess reads must not discharge the obligation, or `role` is
    // a formality.
    for blind in [
        PartialReason::BlindInScope,
        PartialReason::UnderApproximates,
    ] {
        let mut cert = valid_baseline_cert();
        let mut entry = fs_entry("freshdag-observer-fsatrace", ProducerRole::Observer);
        entry.partial.insert(
            "fs.read".to_string(),
            PartialCoverage::new(blind, "cannot see reads inside subprocesses"),
        );
        cert.observation_coverage.push(entry);
        let events = vec![bash_tool_invoked_event("freshdag-adapter-claude")];
        let err = cert.check_coverage_deficit(&events).unwrap_err();
        assert!(
            matches!(err, InvariantError::CoverageDeficit { .. }),
            "{blind} must not discharge, got {err:?}"
        );
    }
}

#[test]
fn coverage_deficit_over_approximating_observer_still_discharges() {
    // The other half of the rule, and the reason "any partial note
    // disqualifies" was rejected: the only real observer we have
    // carries partial notes. Over-approximation costs spurious
    // staleness (invariant #15's explicit preference), never spurious
    // freshness.
    let mut cert = valid_baseline_cert();
    let mut entry = fs_entry("freshdag-observer-fsatrace", ProducerRole::Observer);
    entry.partial.insert(
        "fs.read".to_string(),
        PartialCoverage::new(
            PartialReason::OverApproximates,
            "mmap reads are pessimistic: hashed at mmap time",
        ),
    );
    cert.observation_coverage.push(entry);
    let events = vec![bash_tool_invoked_event("freshdag-adapter-claude")];
    cert.check_coverage_deficit(&events).unwrap();
}

#[test]
fn coverage_deficit_fs_write_only_observer_does_not_discharge() {
    // Validity is about *inputs*. The predicate was
    // `covers(FsRead) || covers(FsWrite)`, so an observer that sees
    // only writes — and therefore contributes zero dependency edges —
    // discharged a bash obligation.
    let mut cert = valid_baseline_cert();
    cert.observation_coverage.push(CoverageEntry {
        producer: "freshdag-observer-writes-only".to_string(),
        version: "0.1.0".to_string(),
        role: ProducerRole::Observer,
        emits: vec![EventKindPattern::from("fs.write")],
        partial: BTreeMap::new(),
        known_limitations: vec![],
    });
    let events = vec![bash_tool_invoked_event("freshdag-adapter-claude")];
    let err = cert.check_coverage_deficit(&events).unwrap_err();
    assert!(matches!(err, InvariantError::CoverageDeficit { .. }));
}

#[test]
fn coverage_entry_covers_stays_syntactic_while_discharges_reads_partial() {
    // The two predicates answer different questions on purpose. ADR
    // 0011 exists because `covers`'s doc said `partial` was "a separate
    // consumer-side signal" and no consumer consulted it; quietly
    // widening `covers` would have changed every existing call site's
    // meaning with no compiler help.
    let mut entry = fs_entry("freshdag-observer-fsatrace", ProducerRole::Observer);
    entry.partial.insert(
        "fs.read".to_string(),
        PartialCoverage::new(PartialReason::BlindInScope, "blind"),
    );
    assert!(entry.covers(EventKind::FsRead));
    assert!(!entry.discharges(EventKind::FsRead));
    // fs.write is untouched by an fs.read declaration.
    assert!(entry.discharges(EventKind::FsWrite));
    assert!(!entry.discharges_subprocess_obligation());
}

#[test]
fn coverage_deficit_probe_role_does_not_discharge_bash_obligation() {
    // A probe reports external-state freshness; it has no vantage point
    // on subprocess filesystem effects either.
    let mut cert = valid_baseline_cert();
    cert.observation_coverage = vec![
        CoverageEntry {
            producer: "freshdag-adapter-claude".to_string(),
            version: "0.1.0".to_string(),
            role: ProducerRole::Adapter,
            emits: vec![],
            partial: BTreeMap::new(),
            known_limitations: vec![],
        },
        fs_entry("freshdag-probe-file", ProducerRole::Probe),
    ];
    let events = vec![bash_tool_invoked_event("freshdag-adapter-claude")];
    let err = cert.check_coverage_deficit(&events).unwrap_err();
    assert!(matches!(err, InvariantError::CoverageDeficit { .. }));
}

#[test]
fn producer_role_wire_form() {
    for (variant, wire) in [
        (ProducerRole::Adapter, "adapter"),
        (ProducerRole::Observer, "observer"),
        (ProducerRole::Probe, "probe"),
    ] {
        assert_eq!(
            serde_json::to_string(&variant).unwrap(),
            format!("\"{wire}\"")
        );
        let back: ProducerRole = serde_json::from_str(&format!("\"{wire}\"")).unwrap();
        assert_eq!(back, variant);
    }
}

#[test]
fn coverage_entry_role_is_required_on_the_wire() {
    // No `#[serde(default)]`: a certificate that omits `role` must fail
    // to parse rather than silently defaulting into a role that might
    // discharge an obligation it cannot.
    let json = r#"{"producer":"p","version":"0.1.0","emits":[],"known_limitations":[]}"#;
    assert!(serde_json::from_str::<CoverageEntry>(json).is_err());
}

#[test]
fn coverage_deficit_ignores_bash_when_status_not_valid() {
    // If the cert already claims stale/unknown/likely-valid, the
    // coverage-deficit rule is moot — the status is already conservative.
    let mut cert = valid_baseline_cert();
    cert.status.value = ValidityStatus::Stale;
    cert.status.reasons = vec![ValidityReason {
        dependency_key: cert.depends_on[0].key.clone(),
        reason: ReasonCode::Drift,
        detail: None,
    }];
    let events = vec![bash_tool_invoked_event("freshdag-adapter-claude")];
    cert.check_coverage_deficit(&events).unwrap();
}

#[test]
fn coverage_deficit_flags_missing_producer_even_on_non_valid_cert() {
    // Producer-membership rule runs regardless of status.
    let mut cert = valid_baseline_cert();
    cert.status.value = ValidityStatus::Stale;
    cert.status.reasons = vec![ValidityReason {
        dependency_key: cert.depends_on[0].key.clone(),
        reason: ReasonCode::Drift,
        detail: None,
    }];
    let events = vec![bash_tool_invoked_event("some-unregistered-producer")];
    let err = cert.check_coverage_deficit(&events).unwrap_err();
    match err {
        InvariantError::ProducerMissingFromCoverage { producer } => {
            assert_eq!(producer, "some-unregistered-producer");
        }
        other => panic!("expected ProducerMissingFromCoverage, got {other:?}"),
    }
}

#[test]
fn coverage_deficit_flags_task_kind_too() {
    let cert = valid_baseline_cert();
    let mut ev = bash_tool_invoked_event("freshdag-adapter-claude");
    // Rewrite the payload to be a task, not a bash invocation.
    ev.payload = serde_json::json!({
        "tool_name": "delegate-research",
        "tool_kind": "task",
        "tool_input": { "prompt": "..." },
    });
    let err = cert.check_coverage_deficit(&[ev]).unwrap_err();
    match err {
        InvariantError::CoverageDeficit { tool_kind } => assert_eq!(tool_kind, "task"),
        other => panic!("expected CoverageDeficit(task), got {other:?}"),
    }
}

#[test]
fn coverage_deficit_ignores_builtin_and_mcp_tool_kinds() {
    let cert = valid_baseline_cert();
    let mut ev = bash_tool_invoked_event("freshdag-adapter-claude");
    ev.payload = serde_json::json!({
        "tool_name": "mcp/attio/get-record",
        "tool_kind": "mcp",
        "tool_input": { "record_id": "acme" },
    });
    cert.check_coverage_deficit(&[ev]).unwrap();
}
