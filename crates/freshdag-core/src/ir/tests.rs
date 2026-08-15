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
    FsWrite, FsWriteMode, Hash, HashAlgo, HashParseError, IrEvent, ToolCompleted, ToolInvoked,
    ProducerRole, ToolKind, TypedPayload,
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
                "mmap-pessimistic; hashes at mmap time".to_string(),
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
