//! Serialization round-trip and contract-conformance tests for the S0
//! IR surface. See `docs/contracts/execution-ir.md` and
//! `schemas/execution-ir/v0.1.json`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    envelope::DecodeError, CoverageManifest, EventKind, EventKindPattern, FsRead, FsReadKind,
    FsWrite, FsWriteMode, Hash, HashAlgo, HashParseError, IrEvent, PartialCoverage, PartialReason,
    ProducerRole, ToolCompleted, ToolInvoked, ToolKind, TypedPayload, ALL_PARTIAL_REASONS,
};

fn sample_uuid() -> Uuid {
    Uuid::parse_str("018f5b52-4b8b-7a1a-9c2f-1a2b3c4d5e6f").unwrap()
}

fn sample_ts() -> OffsetDateTime {
    OffsetDateTime::parse(
        "2026-08-15T13:45:14.220000000Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap()
}

fn envelope_with_payload(kind: EventKind, payload: serde_json::Value) -> IrEvent {
    IrEvent {
        event_id: sample_uuid(),
        producer: "freshdag-observer-fsatrace".to_string(),
        producer_version: "0.1.0".to_string(),
        session_id: "sess-abc".to_string(),
        computation_id: Some("comp-xyz".to_string()),
        parent_id: None,
        causal_inputs: None,
        ts: sample_ts(),
        kind,
        payload,
    }
}

// --------------------------------------------------------------------
// Envelope round-trips (via decode_payload for the S0 typed variants)
// --------------------------------------------------------------------

#[test]
fn round_trip_fs_read() {
    let payload_struct = FsRead {
        path: PathBuf::from("/abs/path/ICP.md"),
        size: 4213,
        hash: Some(Hash::new(HashAlgo::Blake3, "a".repeat(64)).unwrap()),
        follow_symlink_target: None,
        raw_path: Some(PathBuf::from("ICP.md")),
        read_kind: FsReadKind::Direct,
        impure: false,
    };
    let payload_value = serde_json::to_value(&payload_struct).unwrap();
    let event = envelope_with_payload(EventKind::FsRead, payload_value);

    let encoded = serde_json::to_string(&event).unwrap();
    let decoded: IrEvent = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, event);

    let typed = decoded.decode_payload().unwrap();
    assert_eq!(typed, TypedPayload::FsRead(payload_struct));
}

#[test]
fn round_trip_fs_write() {
    let payload_struct = FsWrite {
        path: PathBuf::from("/abs/path/brief.md"),
        size: 1024,
        hash: Some(Hash::new(HashAlgo::Blake3, "b".repeat(64)).unwrap()),
        mode: FsWriteMode::Truncate,
        raw_path: None,
    };
    let payload_value = serde_json::to_value(&payload_struct).unwrap();
    let event = envelope_with_payload(EventKind::FsWrite, payload_value);

    let encoded = serde_json::to_string(&event).unwrap();
    let decoded: IrEvent = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, event);

    let typed = decoded.decode_payload().unwrap();
    assert_eq!(typed, TypedPayload::FsWrite(payload_struct));
}

#[test]
fn round_trip_tool_invoked_and_completed() {
    let invoked = ToolInvoked {
        tool_name: "mcp/attio/get-record".to_string(),
        tool_kind: ToolKind::Mcp,
        tool_input: json!({ "record_id": "acme" }),
        cwd: Some(PathBuf::from("/repo")),
    };
    let completed = ToolCompleted {
        tool_output: json!({ "name": "Acme", "version": 42 }),
        is_error: false,
        duration_ms: 187,
    };

    let inv_event = envelope_with_payload(
        EventKind::ToolInvoked,
        serde_json::to_value(&invoked).unwrap(),
    );
    let comp_event = envelope_with_payload(
        EventKind::ToolCompleted,
        serde_json::to_value(&completed).unwrap(),
    );

    let inv_round: IrEvent =
        serde_json::from_str(&serde_json::to_string(&inv_event).unwrap()).unwrap();
    let comp_round: IrEvent =
        serde_json::from_str(&serde_json::to_string(&comp_event).unwrap()).unwrap();

    assert_eq!(inv_round, inv_event);
    assert_eq!(comp_round, comp_event);
    assert_eq!(
        inv_round.decode_payload().unwrap(),
        TypedPayload::ToolInvoked(invoked)
    );
    assert_eq!(
        comp_round.decode_payload().unwrap(),
        TypedPayload::ToolCompleted(completed)
    );
}

// --------------------------------------------------------------------
// Event kind wire names — must match schemas/execution-ir/v0.1.json
// --------------------------------------------------------------------

#[test]
fn event_kind_wire_names_match_schema() {
    let cases: &[(EventKind, &str)] = &[
        (EventKind::SessionStarted, "session.started"),
        (EventKind::SessionEnded, "session.ended"),
        (EventKind::ComputationStarted, "computation.started"),
        (EventKind::ComputationEnded, "computation.ended"),
        (EventKind::ToolInvoked, "tool.invoked"),
        (EventKind::ToolCompleted, "tool.completed"),
        (EventKind::FsRead, "fs.read"),
        (EventKind::FsWrite, "fs.write"),
        (EventKind::FsStat, "fs.stat"),
        (EventKind::FsRename, "fs.rename"),
        (EventKind::FsUnlink, "fs.unlink"),
        (EventKind::FsDirlist, "fs.dirlist"),
        (EventKind::ProcSpawn, "proc.spawn"),
        (EventKind::ProcExit, "proc.exit"),
        (EventKind::NetConnect, "net.connect"),
        (EventKind::NetFetch, "net.fetch"),
        (EventKind::ProbeChecked, "probe.checked"),
        (EventKind::ArtifactProduced, "artifact.produced"),
        (EventKind::Diagnostic, "diagnostic"),
    ];
    for (variant, wire) in cases {
        let expected_json = format!("\"{wire}\"");
        assert_eq!(
            serde_json::to_string(variant).unwrap(),
            expected_json,
            "{variant:?} did not serialize to {expected_json}"
        );
        assert_eq!(variant.as_wire_str(), *wire);
        let parsed: EventKind = serde_json::from_str(&expected_json).unwrap();
        assert_eq!(parsed, *variant);
    }
}

// --------------------------------------------------------------------
// Hash — wire form and Option<Hash> distinction
// --------------------------------------------------------------------

#[test]
fn hash_string_form_round_trip() {
    let err = Hash::new(HashAlgo::Blake3, "abc123").expect_err("wrong length must reject");
    assert!(matches!(err, HashParseError::BadDigest(_)));

    let digest = "0".repeat(64);
    let hash = Hash::new(HashAlgo::Blake3, &digest).unwrap();
    let s = hash.to_string();
    assert_eq!(s, format!("blake3:{digest}"));
    let round: Hash = s.parse().unwrap();
    assert_eq!(round, hash);

    // JSON round-trip (as a Hash inside a container).
    let as_json = serde_json::to_string(&hash).unwrap();
    assert_eq!(as_json, format!("\"blake3:{digest}\""));
    let back: Hash = serde_json::from_str(&as_json).unwrap();
    assert_eq!(back, hash);
}

#[test]
fn hash_rejects_uppercase_hex() {
    let err = Hash::new(HashAlgo::Blake3, "A".repeat(64)).unwrap_err();
    assert!(matches!(err, HashParseError::BadDigest(_)));
}

#[test]
fn hash_from_str_rejects_missing_prefix() {
    let err = Hash::from_str("0123456789abcdef".repeat(4).as_str()).unwrap_err();
    assert!(matches!(err, HashParseError::MalformedTag(_)));
}

#[test]
fn hash_from_str_rejects_unknown_algo() {
    let err = Hash::from_str(&format!("md5:{}", "0".repeat(64))).unwrap_err();
    assert!(matches!(err, HashParseError::UnknownAlgo(_)));
}

#[test]
fn option_hash_none_serializes_absent() {
    // Invariant #7 keystone: absence is Option::None, never a sentinel hash.
    let read = FsRead {
        path: PathBuf::from("/x"),
        size: 0,
        hash: None,
        follow_symlink_target: None,
        raw_path: None,
        read_kind: FsReadKind::Direct,
        impure: false,
    };
    let v = serde_json::to_value(&read).unwrap();
    assert!(
        v.get("hash").is_none(),
        "Option<Hash>::None must be absent, not null; found: {v}"
    );
}

// --------------------------------------------------------------------
// CoverageManifest — pattern matching and covers() semantics
// --------------------------------------------------------------------

#[test]
fn coverage_manifest_pattern_matching() {
    let fs_star = EventKindPattern::from("fs.*");
    assert!(fs_star.matches(EventKind::FsRead));
    assert!(fs_star.matches(EventKind::FsWrite));
    assert!(fs_star.matches(EventKind::FsStat));
    assert!(!fs_star.matches(EventKind::ToolInvoked));

    let exact = EventKindPattern::from("fs.read");
    assert!(exact.matches(EventKind::FsRead));
    assert!(!exact.matches(EventKind::FsWrite));

    // Patterns must match the whole segment, not just the prefix: `fs.` is
    // not a match for `fs.read` unless expressed as `fs.*`.
    let leading_dot = EventKindPattern::from("fs.");
    assert!(!leading_dot.matches(EventKind::FsRead));
}

#[test]
fn coverage_manifest_covers_and_partial() {
    let manifest = CoverageManifest {
        producer: "freshdag-observer-fsatrace".to_string(),
        version: "0.1.0".to_string(),
        role: ProducerRole::Observer,
        platforms: vec!["linux-x86_64".to_string()],
        emits: vec![
            EventKindPattern::from("fs.read"),
            EventKindPattern::from("fs.write"),
            EventKindPattern::from("proc.*"),
        ],
        partial: {
            let mut m = BTreeMap::new();
            m.insert(
                "fs.read".to_string(),
                PartialCoverage::new(
                    PartialReason::OverApproximates,
                    "mmap-pessimistic; hashes at mmap time",
                ),
            );
            m
        },
        capabilities: BTreeMap::new(),
        known_limitations: vec!["glibc only".to_string()],
    };

    assert!(manifest.covers(EventKind::FsRead));
    assert!(manifest.covers(EventKind::ProcSpawn));
    assert!(!manifest.covers(EventKind::NetConnect));
    assert!(!manifest.covers(EventKind::ToolInvoked));

    assert_eq!(
        manifest.partial_note(EventKind::FsRead),
        Some("mmap-pessimistic; hashes at mmap time")
    );
    assert_eq!(manifest.partial_note(EventKind::FsWrite), None);
}

// --------------------------------------------------------------------
// ADR 0011: `partial` is a closed vocabulary, and the direction of the
// error decides whether an obligation is discharged.
// --------------------------------------------------------------------

fn observer_with_partial(entries: &[(&str, PartialCoverage)]) -> CoverageManifest {
    CoverageManifest {
        producer: "freshdag-observer-test".to_string(),
        version: "0.1.0".to_string(),
        role: ProducerRole::Observer,
        platforms: vec![],
        emits: vec![
            EventKindPattern::from("fs.read"),
            EventKindPattern::from("fs.write"),
        ],
        partial: entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect(),
        capabilities: BTreeMap::new(),
        known_limitations: vec![],
    }
}

#[test]
fn partial_reason_serde_and_as_wire_str_agree() {
    for reason in ALL_PARTIAL_REASONS {
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, format!("\"{}\"", reason.as_wire_str()));
        let back: PartialReason = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reason);
        assert_eq!(reason.to_string(), reason.as_wire_str());
    }
}

#[test]
fn partial_reason_rejects_codes_outside_the_vocabulary() {
    // The vocabulary is closed: an unrecognized reason is unreadable,
    // never guessed at.
    assert!(serde_json::from_str::<PartialReason>("\"mostly-fine\"").is_err());
    assert!(serde_json::from_str::<PartialReason>("\"OverApproximates\"").is_err());
}

#[test]
fn only_over_approximation_discharges() {
    assert!(PartialReason::OverApproximates.discharges());
    assert!(!PartialReason::UnderApproximates.discharges());
    assert!(!PartialReason::BlindInScope.discharges());
}

#[test]
fn legacy_bare_string_partial_decodes_as_under_approximates() {
    // The migration's whole safety argument: a pre-ADR-0011 manifest
    // cannot state its direction of error, so it gets the conservative
    // answer and stops discharging until its owner reclassifies it.
    // Defaulting the other way would be a silent-wrong-answer generator
    // on the invariant-#7 path.
    let raw = json!({
        "producer": "freshdag-observer-legacy",
        "version": "0.1.0",
        "role": "observer",
        "emits": ["fs.read", "fs.write"],
        "partial": { "fs.read": "cannot see reads inside subprocesses" }
    });
    let manifest: CoverageManifest = serde_json::from_value(raw).unwrap();

    let entry = manifest.partial_coverage(EventKind::FsRead).unwrap();
    assert_eq!(entry.reason, PartialReason::UnderApproximates);
    assert_eq!(entry.note, "cannot see reads inside subprocesses");

    // The note survives verbatim...
    assert_eq!(
        manifest.partial_note(EventKind::FsRead),
        Some("cannot see reads inside subprocesses")
    );
    // ...and the producer still *covers* fs.read syntactically, but no
    // longer discharges an obligation on it.
    assert!(manifest.covers(EventKind::FsRead));
    assert!(!manifest.discharges(EventKind::FsRead));
}

#[test]
fn structured_partial_round_trips_and_normalizes_legacy_forward() {
    let manifest = observer_with_partial(&[(
        "fs.read",
        PartialCoverage::new(
            PartialReason::OverApproximates,
            "mmap reads are pessimistic",
        ),
    )]);
    let s = serde_json::to_string(&manifest).unwrap();
    assert!(s.contains("\"reason\":\"over-approximates\""));
    let back: CoverageManifest = serde_json::from_str(&s).unwrap();
    assert_eq!(back, manifest);

    // A legacy manifest re-serializes in the structured form, so the
    // wire shape converges forward rather than staying ambiguous.
    let legacy: CoverageManifest = serde_json::from_value(json!({
        "producer": "p", "version": "0.1.0", "role": "observer",
        "emits": ["fs.read"], "partial": { "fs.read": "prose" }
    }))
    .unwrap();
    let s = serde_json::to_string(&legacy).unwrap();
    assert!(s.contains("\"reason\":\"under-approximates\""));
    let back: CoverageManifest = serde_json::from_str(&s).unwrap();
    assert_eq!(back, legacy);
}

#[test]
fn partial_object_without_reason_fails_to_deserialize() {
    // There is no default in the object form. A producer that writes
    // `{note: …}` has a bug, and a bug on this path must be loud rather
    // than guessed at in either direction.
    let err = serde_json::from_value::<CoverageManifest>(json!({
        "producer": "p", "version": "0.1.0", "role": "observer",
        "emits": ["fs.read"], "partial": { "fs.read": { "note": "prose" } }
    }))
    .unwrap_err();
    assert!(
        err.to_string().contains("reason"),
        "error should name the missing field: {err}"
    );

    // `note` alone is optional when `reason` is present.
    let ok: CoverageManifest = serde_json::from_value(json!({
        "producer": "p", "version": "0.1.0", "role": "observer",
        "emits": ["fs.read"], "partial": { "fs.read": { "reason": "blind-in-scope" } }
    }))
    .unwrap();
    assert_eq!(
        ok.partial_coverage(EventKind::FsRead).unwrap().reason,
        PartialReason::BlindInScope
    );
    assert_eq!(ok.partial_note(EventKind::FsRead), Some(""));
}

#[test]
fn discharges_requires_coverage_and_a_safe_direction() {
    // Over-approximation is the only direction that discharges.
    let over = observer_with_partial(&[(
        "fs.read",
        PartialCoverage::new(PartialReason::OverApproximates, "pessimistic"),
    )]);
    assert!(over.discharges(EventKind::FsRead));

    for bad in [
        PartialReason::UnderApproximates,
        PartialReason::BlindInScope,
    ] {
        let m = observer_with_partial(&[("fs.read", PartialCoverage::new(bad, "n"))]);
        assert!(m.covers(EventKind::FsRead), "{bad} should still cover");
        assert!(!m.discharges(EventKind::FsRead), "{bad} must not discharge");
    }

    // No partial declaration at all is a full-fidelity claim.
    let clean = observer_with_partial(&[]);
    assert!(clean.discharges(EventKind::FsRead));

    // Discharge requires coverage in the first place.
    assert!(!clean.discharges(EventKind::NetConnect));
}

#[test]
fn a_specific_partial_entry_cannot_override_a_broader_blindness() {
    // The Claude adapter's real shape: `fs.*` admits total blindness
    // inside subprocesses while `fs.read` describes what it does see.
    // Letting the more specific key win would let any producer annotate
    // its way out of its own broadest admission.
    let m = observer_with_partial(&[
        (
            "fs.*",
            PartialCoverage::new(PartialReason::BlindInScope, "nothing inside subprocesses"),
        ),
        (
            "fs.read",
            PartialCoverage::new(PartialReason::OverApproximates, "pessimistic where visible"),
        ),
    ]);
    assert!(m.covers(EventKind::FsRead));
    assert!(!m.discharges(EventKind::FsRead));

    // Presentation still prefers the most specific note.
    assert_eq!(
        m.partial_note(EventKind::FsRead),
        Some("pessimistic where visible")
    );
}

#[test]
fn adding_a_partial_entry_can_only_ever_make_a_producer_discharge_less() {
    // ADR 0011, Amendment, Correction 4 names monotonicity as "the
    // property to preserve, and the one to test": a `partial` map is a
    // conjunction of admissions, so a new entry may withdraw a
    // discharge but must never grant one. The test above pins one
    // instance of this; here it is pinned as the general property, over
    // every (base manifest, added entry, kind) triple the closed
    // vocabulary allows.
    //
    // A rule that resolved most-specific-wins fails this: adding a
    // narrow `over-approximates` entry beneath a broad `blind-in-scope`
    // one would turn a non-discharging manifest into a discharging one.
    const KINDS: [EventKind; 6] = [
        EventKind::FsRead,
        EventKind::FsWrite,
        EventKind::FsStat,
        EventKind::ProcSpawn,
        EventKind::NetConnect,
        EventKind::ToolInvoked,
    ];
    const PATTERNS: [&str; 6] = ["fs.read", "fs.write", "fs.*", "proc.*", "net.connect", "*"];
    const REASONS: [PartialReason; 3] = [
        PartialReason::OverApproximates,
        PartialReason::UnderApproximates,
        PartialReason::BlindInScope,
    ];

    let bases: Vec<Vec<(&str, PartialCoverage)>> = vec![
        vec![],
        vec![(
            "fs.read",
            PartialCoverage::new(PartialReason::OverApproximates, "coarse"),
        )],
        vec![(
            "fs.*",
            PartialCoverage::new(PartialReason::BlindInScope, "blind in subprocesses"),
        )],
        vec![
            (
                "fs.*",
                PartialCoverage::new(PartialReason::OverApproximates, "coarse"),
            ),
            (
                "fs.write",
                PartialCoverage::new(PartialReason::UnderApproximates, "lossy"),
            ),
        ],
    ];

    for base in &bases {
        let before = observer_with_partial(base);
        for pattern in PATTERNS {
            // `partial` is keyed by pattern, so reusing an existing key
            // is a replacement rather than an addition. Monotonicity is
            // a claim about additions only.
            if base.iter().any(|(k, _)| *k == pattern) {
                continue;
            }
            for reason in REASONS {
                let mut extended = base.clone();
                extended.push((pattern, PartialCoverage::new(reason, "added")));
                let after = observer_with_partial(&extended);

                for kind in KINDS {
                    assert!(
                        !after.discharges(kind) || before.discharges(kind),
                        "adding `{pattern}` => `{reason}` granted a discharge for `{}` \
                         that the base manifest did not have; a `partial` entry must \
                         only ever subtract (ADR 0011, Amendment, Correction 4)",
                        kind.as_wire_str()
                    );
                }
            }
        }
    }
}

#[test]
fn wildcard_partial_applies_to_every_matching_kind() {
    let m = observer_with_partial(&[(
        "fs.*",
        PartialCoverage::new(PartialReason::UnderApproximates, "lossy"),
    )]);
    assert!(!m.discharges(EventKind::FsRead));
    assert!(!m.discharges(EventKind::FsWrite));
}

#[test]
fn coverage_manifest_round_trip() {
    let manifest = CoverageManifest {
        producer: "freshdag-adapter-claude".to_string(),
        version: "0.1.0".to_string(),
        role: ProducerRole::Adapter,
        platforms: vec![],
        emits: vec![
            EventKindPattern::from("tool.*"),
            EventKindPattern::from("session.*"),
        ],
        partial: BTreeMap::new(),
        capabilities: BTreeMap::new(),
        known_limitations: vec!["subprocess I/O invisible without observer".to_string()],
    };
    let s = serde_json::to_string(&manifest).unwrap();
    let back: CoverageManifest = serde_json::from_str(&s).unwrap();
    assert_eq!(back, manifest);
}

// --------------------------------------------------------------------
// Schema conformance — hand-authored valid JSON parses; required
// envelope fields survive round-trip.
// --------------------------------------------------------------------

#[test]
fn schema_conformance_envelope_required_fields() {
    let raw = json!({
        "event_id": "018f5b52-4b8b-7a1a-9c2f-1a2b3c4d5e6f",
        "producer": "freshdag-observer-fsatrace",
        "producer_version": "0.1.0",
        "session_id": "sess-abc",
        "computation_id": "comp-xyz",
        "parent_id": null,
        "causal_inputs": null,
        "ts": "2026-08-15T13:45:14.220000000Z",
        "kind": "fs.read",
        "payload": {
            "path": "/abs/path/ICP.md",
            "size": 4213,
            "hash": "blake3:0000000000000000000000000000000000000000000000000000000000000000"
        }
    });

    let parsed: IrEvent = serde_json::from_value(raw.clone()).unwrap();
    let re_encoded = serde_json::to_value(&parsed).unwrap();

    // Every field from `schemas/execution-ir/v0.1.json §required` MUST
    // appear on re-encoding.
    for required in [
        "event_id",
        "producer",
        "producer_version",
        "session_id",
        "ts",
        "kind",
        "payload",
    ] {
        assert!(
            re_encoded.get(required).is_some(),
            "re-encoded event missing required field `{required}`"
        );
    }
}

#[test]
fn unknown_envelope_fields_are_currently_tolerated() {
    // This test documents current behavior. v0.1 uses serde's default
    // (unknown envelope fields are silently ignored), which is
    // permissive enough for producers speaking a future minor version.
    //
    // The stricter policy (`#[serde(deny_unknown_fields)]` on
    // `IrEvent`) is a candidate ADR — if adopted, this test flips to
    // `is_err()` and gets renamed.
    let raw = json!({
        "event_id": "018f5b52-4b8b-7a1a-9c2f-1a2b3c4d5e6f",
        "producer": "x",
        "producer_version": "0.1.0",
        "session_id": "s",
        "ts": "2026-08-15T13:45:14.220000000Z",
        "kind": "fs.read",
        "payload": {},
        "unknown_envelope_field": "tolerated today"
    });
    let parsed = serde_json::from_value::<IrEvent>(raw);
    assert!(
        parsed.is_ok(),
        "v0.1 tolerates unknown envelope fields; see test docstring"
    );
}

#[test]
fn payload_leniency_unknown_fields_tolerated() {
    // The IR contract intentionally leaves payload extensible: adapters
    // add fields, not kinds. A payload with future fields must still
    // parse — the S0 typed decoder ignores them.
    let raw = json!({
        "event_id": "018f5b52-4b8b-7a1a-9c2f-1a2b3c4d5e6f",
        "producer": "x",
        "producer_version": "0.1.0",
        "session_id": "s",
        "ts": "2026-08-15T13:45:14.220000000Z",
        "kind": "fs.read",
        "payload": {
            "path": "/x",
            "size": 0,
            "future_field": "should be ignored by typed decoder"
        }
    });
    let event: IrEvent = serde_json::from_value(raw).unwrap();
    // Untyped access preserves the field.
    assert_eq!(
        event.payload.get("future_field").and_then(|v| v.as_str()),
        Some("should be ignored by typed decoder")
    );
    // Typed access strips it.
    let typed = event.decode_payload().unwrap();
    assert!(matches!(typed, TypedPayload::FsRead(_)));
}

#[test]
fn decode_payload_errors_for_kinds_without_typed_variant() {
    let event = envelope_with_payload(
        EventKind::SessionStarted,
        json!({ "agent_kind": "claude-code" }),
    );
    match event.decode_payload() {
        Err(DecodeError::Unsupported(EventKind::SessionStarted)) => {}
        other => panic!("expected Unsupported(SessionStarted), got {other:?}"),
    }
}

#[test]
fn decode_payload_errors_on_malformed_payload_shape() {
    let event = envelope_with_payload(EventKind::FsRead, json!({ "wrong": "shape" }));
    match event.decode_payload() {
        Err(DecodeError::Malformed(_)) => {}
        other => panic!("expected Malformed, got {other:?}"),
    }
}

// --------------------------------------------------------------------
// Unknown-state invariants (invariant #7)
// --------------------------------------------------------------------

#[test]
fn hash_variants_distinguish_unknown_from_present() {
    // A hash string that would collide with an "unknown" sentinel must
    // not parse. The domain-level "unknown" is represented by Option::None
    // at the field level, never by a Hash value.
    assert!(Hash::from_str("blake3:unknown").is_err());
    assert!(Hash::from_str("unknown").is_err());
    assert!(Hash::from_str("").is_err());
}
