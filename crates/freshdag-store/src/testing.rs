//! Deterministic fixtures for the crate's unit tests.
//!
//! Every value here is fixed: no clocks, no random UUIDs. Per
//! `.claude/rules/testing.md`, store tests must be deterministic.

use freshdag_core::ir::{CoverageManifest, EventKind, EventKindPattern, IrEvent, ProducerRole};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;

/// Parse a fixed UUID.
pub(crate) fn uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).expect("fixture uuid")
}

/// A fixed instant `secs` seconds after the Unix epoch, in UTC.
pub(crate) fn ts(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(secs).expect("fixture timestamp")
}

/// A simple `fs.read` event.
pub(crate) fn event(
    producer: &str,
    session_id: &str,
    at: OffsetDateTime,
    event_id: &str,
) -> IrEvent {
    event_at(producer, session_id, at, uuid(event_id))
}

/// A simple `fs.read` event with an already-parsed identifier.
pub(crate) fn event_at(
    producer: &str,
    session_id: &str,
    at: OffsetDateTime,
    event_id: Uuid,
) -> IrEvent {
    IrEvent {
        event_id,
        producer: producer.to_string(),
        producer_version: "0.1.0".to_string(),
        session_id: session_id.to_string(),
        computation_id: None,
        parent_id: None,
        causal_inputs: None,
        ts: at,
        kind: EventKind::FsRead,
        payload: serde_json::json!({ "path": "/tmp/fixture", "size": 1 }),
    }
}

/// Derive a [`ProducerRole`] from a fixture producer name.
///
/// Store fixtures name producers `adapter-*`, `observer-*`, or
/// `probe-*`. Rather than thread a role through ~20 call sites, we read
/// it off that convention — but an unrecognized name **panics** instead
/// of defaulting, because a silently-wrong role is exactly the failure
/// `ProducerRole` was introduced to prevent (see
/// `certificate-contract.md` §Coverage-Deficit Rule).
pub(crate) fn role_for(producer: &str) -> ProducerRole {
    if producer.contains("observer") {
        ProducerRole::Observer
    } else if producer.contains("adapter") {
        ProducerRole::Adapter
    } else if producer.contains("probe") {
        ProducerRole::Probe
    } else {
        panic!(
            "fixture producer `{producer}` must contain `adapter`, `observer`, or `probe` \
             so its ProducerRole is unambiguous"
        )
    }
}

/// A minimal coverage manifest. Role is derived via [`role_for`].
pub(crate) fn manifest(producer: &str, version: &str, emits: &[&str]) -> CoverageManifest {
    CoverageManifest {
        role: role_for(producer),
        producer: producer.to_string(),
        version: version.to_string(),
        platforms: Vec::new(),
        emits: emits.iter().map(|p| EventKindPattern::new(*p)).collect(),
        partial: BTreeMap::new(),
        capabilities: BTreeMap::new(),
        known_limitations: Vec::new(),
    }
}
