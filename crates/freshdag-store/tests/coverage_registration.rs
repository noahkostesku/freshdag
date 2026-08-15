//! The store half of W6.2: producers register coverage manifests; the
//! store answers "which manifests contributed to this computation?".
//!
//! The answer is a `Vec<CoverageEntry>` — the exact type
//! `Certificate.observation_coverage` holds — so the engine (W4) can drop
//! it straight onto a certificate and run
//! `Certificate::check_coverage_deficit`. This test exercises the seam up
//! to that hand-off; it deliberately does not construct a `Certificate`,
//! because how the engine consumes coverage is W4's design, not the
//! store's.

use std::collections::BTreeMap;

use freshdag_core::ir::{CoverageManifest, EventKind, EventKindPattern, IrEvent, ProducerRole};
use freshdag_store::{ProducerKey, Store};
use time::OffsetDateTime;
use uuid::Uuid;

const ADAPTER: &str = "freshdag-adapter-claude";
const OBSERVER: &str = "freshdag-observer-scripted";

/// Derive the role from the fixture producer name. An unrecognized name
/// panics rather than defaulting — a silently-wrong role is the exact
/// failure `ProducerRole` exists to prevent.
fn role_for(producer: &str) -> ProducerRole {
    if producer.contains("observer") {
        ProducerRole::Observer
    } else if producer.contains("adapter") {
        ProducerRole::Adapter
    } else if producer.contains("probe") {
        ProducerRole::Probe
    } else {
        panic!("fixture producer `{producer}` must name its role")
    }
}

fn manifest(producer: &str, version: &str, emits: &[&str]) -> CoverageManifest {
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

fn bash_tool_invoked(id: &str, producer: &str, version: &str) -> IrEvent {
    IrEvent {
        event_id: Uuid::parse_str(id).expect("uuid"),
        producer: producer.to_string(),
        producer_version: version.to_string(),
        session_id: "sess-1".to_string(),
        computation_id: Some("comp-1".to_string()),
        parent_id: None,
        causal_inputs: None,
        ts: OffsetDateTime::from_unix_timestamp(100).expect("ts"),
        kind: EventKind::ToolInvoked,
        payload: serde_json::json!({
            "tool_name": "Bash",
            "tool_kind": "bash",
            "tool_input": { "command": "make build" },
            "cwd": "/repo"
        }),
    }
}

#[test]
fn an_observer_declaring_no_coverage_yields_no_fs_covered_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    store
        .register_producer(manifest(ADAPTER, "0.1.0", &["tool.*"]))
        .expect("register adapter");
    // A scripted observer that declares it emits nothing at all.
    store
        .register_producer(manifest(OBSERVER, "0.1.0", &[]))
        .expect("register observer");

    let events = vec![
        bash_tool_invoked("00000000-0000-7000-8000-000000000001", ADAPTER, "0.1.0"),
        IrEvent {
            kind: EventKind::Diagnostic,
            payload: serde_json::json!({ "message": "observing nothing" }),
            ..bash_tool_invoked("00000000-0000-7000-8000-000000000002", OBSERVER, "0.1.0")
        },
    ];
    store.append_all(&events).expect("append");
    store.sync().expect("sync");

    let log = store.read_log().expect("read");
    let lookup = store.coverage().coverage_for_computation(&log, "comp-1");

    assert!(lookup.is_complete(), "both producers are registered");
    assert_eq!(lookup.entries.len(), 2);

    // This is exactly the condition `Certificate::check_coverage_deficit`
    // trips on: a bash `tool.invoked` in the stream and no entry in
    // `observation_coverage` declaring fs.* coverage. The engine must
    // refuse `valid` here (invariant #7).
    let has_fs_observer = lookup
        .entries
        .iter()
        .any(|c| c.covers(EventKind::FsRead) || c.covers(EventKind::FsWrite));
    assert!(
        !has_fs_observer,
        "an emits:[] observer must not satisfy the fs.* coverage requirement"
    );
    let has_bash_invocation = log.iter().any(|e| {
        e.kind == EventKind::ToolInvoked && e.payload.get("tool_kind") == Some(&"bash".into())
    });
    assert!(has_bash_invocation);
}

#[test]
fn a_real_observer_satisfies_the_fs_coverage_requirement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    store
        .register_producer(manifest(ADAPTER, "0.1.0", &["tool.*"]))
        .expect("register");
    store
        .register_producer(manifest(OBSERVER, "0.1.0", &["fs.*"]))
        .expect("register");

    let events = vec![bash_tool_invoked(
        "00000000-0000-7000-8000-000000000001",
        ADAPTER,
        "0.1.0",
    )];
    store.append_all(&events).expect("append");
    store.sync().expect("sync");

    // Coverage is scoped by *who emitted in scope*, so an observer that
    // registered but emitted nothing into this computation is not counted
    // — silence from a registered producer is still silence.
    let log = store.read_log().expect("read");
    let lookup = store.coverage().coverage_for_computation(&log, "comp-1");
    assert_eq!(lookup.entries.len(), 1);
    assert_eq!(lookup.entries[0].producer, ADAPTER);

    // Only once the observer actually emits does it appear.
    let mut store = Store::open(dir.path()).expect("reopen");
    let observer_event = IrEvent {
        kind: EventKind::FsRead,
        payload: serde_json::json!({ "path": "/repo/src/main.rs", "size": 10 }),
        ..bash_tool_invoked("00000000-0000-7000-8000-000000000002", OBSERVER, "0.1.0")
    };
    assert_eq!(
        store.append(&observer_event).expect("append"),
        freshdag_store::AppendOutcome::Appended
    );
    store.sync().expect("sync");

    let log = store.read_log().expect("read");
    let lookup = store.coverage().coverage_for_computation(&log, "comp-1");
    assert_eq!(lookup.entries.len(), 2);
    assert!(lookup
        .entries
        .iter()
        .any(|c| c.covers(EventKind::FsRead) || c.covers(EventKind::FsWrite)));
}

#[test]
fn an_unregistered_producer_is_reported_rather_than_omitted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    store
        .register_producer(manifest(ADAPTER, "0.1.0", &["tool.*"]))
        .expect("register");
    // Version skew: the adapter registered 0.1.0 but emits as 0.2.0.
    let events = vec![bash_tool_invoked(
        "00000000-0000-7000-8000-000000000001",
        ADAPTER,
        "0.2.0",
    )];
    store.append_all(&events).expect("append");
    store.sync().expect("sync");

    let log = store.read_log().expect("read");
    let lookup = store.coverage().coverage_for_computation(&log, "comp-1");
    assert!(lookup.entries.is_empty());
    assert!(!lookup.is_complete());
    assert_eq!(
        lookup.unregistered,
        vec![ProducerKey::new(ADAPTER, "0.2.0")],
        "a producer whose coverage we cannot read must never be silently omitted"
    );
}

#[test]
fn registrations_survive_a_process_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    {
        let mut store = Store::open(dir.path()).expect("open");
        store
            .register_producer(manifest(OBSERVER, "0.1.0", &["fs.*", "proc.*"]))
            .expect("register");
    }
    let store = Store::open(dir.path()).expect("reopen");
    let m = store
        .coverage()
        .manifest(&ProducerKey::new(OBSERVER, "0.1.0"))
        .expect("manifest survives");
    assert!(m.covers(EventKind::FsWrite));
    assert!(m.covers(EventKind::ProcSpawn));
    assert!(!m.covers(EventKind::NetConnect));
}
