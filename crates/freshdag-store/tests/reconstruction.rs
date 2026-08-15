//! The invariant #5 keystone.
//!
//! Authored event stream → append (in a deliberately non-canonical
//! physical order) → read → [`linearize`] → assert **byte-identical** to
//! an in-memory reconstruction sorted by the canonical rule from
//! `docs/contracts/execution-ir.md §Ordering`.
//!
//! Then: drop the derived state entirely, replay the canonical log, and
//! assert the rebuilt derived state is identical. That is what
//! "derived state is disposable" has to mean operationally.
//!
//! Everything here is deterministic. The physical write order is a
//! seeded Fisher-Yates shuffle (`.claude/rules/testing.md`: seed
//! randomness, never leave it to the ambient RNG).

use std::collections::BTreeMap;

use freshdag_core::ir::{CoverageManifest, EventKind, EventKindPattern, IrEvent};
use freshdag_store::{
    linearize, linearize_checked, read_log, CoverageRegistry, ProducerKey, Store,
};
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

// ---------------------------------------------------------------- rng

/// xorshift64*. Deterministic, seeded, dependency-free.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        assert!(seed != 0, "xorshift64* degenerates on a zero seed");
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Seeded Fisher-Yates.
fn shuffled<T: Clone>(items: &[T], seed: u64) -> Vec<T> {
    let mut out = items.to_vec();
    let mut rng = Rng::new(seed);
    for i in (1..out.len()).rev() {
        let j = usize::try_from(rng.next_u64() % (i as u64 + 1)).expect("index fits");
        out.swap(i, j);
    }
    out
}

// ----------------------------------------------------------- fixtures

const ADAPTER: &str = "freshdag-adapter-claude";
const OBSERVER: &str = "freshdag-observer-fsatrace";
const PROBE: &str = "freshdag-probe-https";

fn uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).expect("fixture uuid")
}

/// A fixed instant, `secs` seconds past the epoch, in UTC.
fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(secs).expect("fixture timestamp")
}

#[allow(clippy::too_many_arguments)]
fn ev(
    event_id: &str,
    producer: &str,
    version: &str,
    session_id: &str,
    computation_id: Option<&str>,
    ts: OffsetDateTime,
    kind: EventKind,
    payload: serde_json::Value,
) -> IrEvent {
    IrEvent {
        event_id: uuid(event_id),
        producer: producer.to_string(),
        producer_version: version.to_string(),
        session_id: session_id.to_string(),
        computation_id: computation_id.map(ToString::to_string),
        parent_id: None,
        causal_inputs: None,
        ts,
        kind,
        payload,
    }
}

/// The authored stream.
///
/// Deliberately adversarial:
///
/// - three producers interleaved;
/// - three events at the same instant, two of them from the same
///   producer (exercises producer tiebreak *and* `event_id` tiebreak,
///   and proves producer beats `event_id`: the observer's `...01` sorts
///   after the adapter's `...0b`);
/// - two events at the same instant written with **different UTC
///   offsets** (`+05:30` vs `Z`), which must tie on `ts` rather than
///   ordering by their textual spelling;
/// - `proc.spawn` and `net.connect` — kinds with no typed payload
///   variant — carrying fields no struct in `freshdag-core` knows about.
#[allow(clippy::too_many_lines)]
fn authored_events() -> Vec<IrEvent> {
    let t500_utc = at(500);
    let t500_offset = t500_utc.to_offset(UtcOffset::from_hms(5, 30, 0).expect("offset"));
    assert_eq!(t500_utc, t500_offset, "same instant, different spelling");

    vec![
        ev(
            "00000000-0000-7000-8000-00000000000a",
            ADAPTER,
            "0.1.0",
            "sess-1",
            Some("comp-1"),
            at(100),
            EventKind::ComputationStarted,
            serde_json::json!({ "recipe_id": "recipe-a" }),
        ),
        ev(
            "00000000-0000-7000-8000-00000000000b",
            ADAPTER,
            "0.1.0",
            "sess-1",
            Some("comp-1"),
            at(300),
            EventKind::ToolInvoked,
            serde_json::json!({
                "tool_name": "Bash",
                "tool_kind": "bash",
                "tool_input": { "command": "make build" },
                "cwd": "/repo"
            }),
        ),
        // Same ts AND same producer as the event above: event_id breaks it.
        ev(
            "00000000-0000-7000-8000-000000000005",
            ADAPTER,
            "0.1.0",
            "sess-1",
            Some("comp-1"),
            at(300),
            EventKind::ToolCompleted,
            serde_json::json!({ "tool_output": "ok", "is_error": false, "duration_ms": 12 }),
        ),
        // Same ts as both of the above, different producer. Its event_id
        // is the smallest in the whole fixture, so if producer did not
        // dominate event_id this would sort first.
        ev(
            "00000000-0000-7000-8000-000000000001",
            OBSERVER,
            "0.2.0",
            "sess-1",
            Some("comp-1"),
            at(300),
            EventKind::FsRead,
            serde_json::json!({ "path": "/repo/src/main.rs", "size": 2048 }),
        ),
        // No typed payload variant; arbitrary producer-defined fields.
        ev(
            "00000000-0000-7000-8000-000000000002",
            OBSERVER,
            "0.2.0",
            "sess-1",
            Some("comp-1"),
            at(200),
            EventKind::ProcSpawn,
            serde_json::json!({
                "parent_pid": 4001,
                "child_pid": 4002,
                "argv": ["make", "build"],
                "envp_hash": "blake3:deadbeef",
                "cwd": "/repo",
                "exe": "/usr/bin/make",
                "producer_private": { "seccomp_notify_fd": 7, "nested": [1, 2, 3] }
            }),
        ),
        // Same instant as the probe event below, but spelled +05:30.
        ev(
            "00000000-0000-7000-8000-000000000003",
            OBSERVER,
            "0.2.0",
            "sess-1",
            Some("comp-1"),
            t500_offset,
            EventKind::NetConnect,
            serde_json::json!({
                "family": "inet6",
                "addr": "[2606:4700::1]:443",
                "hostname": "example.test",
                "unknown_to_every_struct": { "tls_alpn": "h2" }
            }),
        ),
        ev(
            "00000000-0000-7000-8000-000000000004",
            PROBE,
            "0.3.0",
            "sess-1",
            Some("comp-2"),
            t500_utc,
            EventKind::ProbeChecked,
            serde_json::json!({
                "scheme": "https",
                "key": "https://example.test/data.json",
                "observed_fingerprint": "etag:\"abc\"",
                "trust_class": "versioned",
                "result": "match"
            }),
        ),
        // Infrastructural: no computation_id.
        ev(
            "00000000-0000-7000-8000-000000000006",
            PROBE,
            "0.3.0",
            "sess-1",
            None,
            at(100),
            EventKind::Diagnostic,
            serde_json::json!({ "message": "probe warm-up" }),
        ),
        ev(
            "00000000-0000-7000-8000-000000000007",
            PROBE,
            "0.3.0",
            "sess-1",
            Some("comp-2"),
            at(400),
            EventKind::ArtifactProduced,
            serde_json::json!({
                "artifact_id": "art-1",
                "content_hash": "blake3:cafe",
                "kind": "file",
                "produced_by": "comp-2",
                "comparator": "exact"
            }),
        ),
        ev(
            "00000000-0000-7000-8000-000000000008",
            PROBE,
            "0.3.0",
            "sess-1",
            None,
            at(600),
            EventKind::SessionEnded,
            serde_json::json!({ "reason": "complete" }),
        ),
    ]
}

fn manifest(producer: &str, version: &str, emits: &[&str]) -> CoverageManifest {
    CoverageManifest {
        producer: producer.to_string(),
        version: version.to_string(),
        platforms: vec!["linux-x86_64".to_string()],
        emits: emits.iter().map(|p| EventKindPattern::new(*p)).collect(),
        partial: BTreeMap::new(),
        capabilities: BTreeMap::new(),
        known_limitations: Vec::new(),
    }
}

fn manifests() -> Vec<CoverageManifest> {
    vec![
        manifest(ADAPTER, "0.1.0", &["computation.*", "tool.*"]),
        manifest(OBSERVER, "0.2.0", &["fs.*", "proc.*", "net.*"]),
        manifest(
            PROBE,
            "0.3.0",
            &["probe.*", "artifact.*", "session.*", "diagnostic"],
        ),
    ]
}

/// The in-memory reference implementation of the canonical order, written
/// independently of `linearize` so the test is not just asserting that a
/// function equals itself.
fn reference_sorted(events: &[IrEvent]) -> Vec<IrEvent> {
    let mut out = events.to_vec();
    out.sort_by(|a, b| {
        let ka = (
            a.ts.unix_timestamp(),
            a.ts.nanosecond(),
            a.producer.clone(),
            a.event_id,
        );
        let kb = (
            b.ts.unix_timestamp(),
            b.ts.nanosecond(),
            b.producer.clone(),
            b.event_id,
        );
        ka.cmp(&kb)
    });
    out
}

/// Serialize a stream to the exact bytes the log would hold.
fn to_bytes(events: &[IrEvent]) -> Vec<u8> {
    let mut out = Vec::new();
    for e in events {
        out.extend_from_slice(&serde_json::to_vec(e).expect("serialize"));
        out.push(b'\n');
    }
    out
}

// ------------------------------------------------------ derived state

/// A stand-in for real derived state (W3 builds the real thing). The
/// point of the test is not this shape, it is that *whatever* the shape
/// is, it must fall out of the canonical log alone.
#[derive(Debug, PartialEq, Eq)]
struct Derived {
    /// Canonically ordered event ids per computation.
    per_computation: BTreeMap<String, Vec<Uuid>>,
    /// Event-kind histogram.
    kinds: BTreeMap<String, usize>,
    /// Coverage entries per computation, as `(producer, version)` pairs.
    coverage: BTreeMap<String, Vec<(String, String)>>,
    /// Producers that emitted but declared no coverage.
    unregistered: Vec<ProducerKey>,
}

impl Derived {
    fn materialize(events: &[IrEvent], registry: &CoverageRegistry) -> Self {
        let mut per_computation: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for e in events {
            if let Some(c) = &e.computation_id {
                per_computation
                    .entry(c.clone())
                    .or_default()
                    .push(e.event_id);
            }
            *kinds.entry(e.kind.as_wire_str().to_string()).or_default() += 1;
        }

        let mut coverage = BTreeMap::new();
        let mut unregistered = Vec::new();
        for computation in per_computation.keys() {
            let lookup = registry.coverage_for_computation(events, computation);
            coverage.insert(
                computation.clone(),
                lookup
                    .entries
                    .iter()
                    .map(|c| (c.producer.clone(), c.version.clone()))
                    .collect(),
            );
            unregistered.extend(lookup.unregistered);
        }
        unregistered.sort();
        unregistered.dedup();

        Self {
            per_computation,
            kinds,
            coverage,
            unregistered,
        }
    }
}

// ----------------------------------------------------------- the tests

#[test]
fn replay_is_byte_identical_to_the_canonical_reconstruction() {
    // Physical write order: seeded shuffle, deliberately not canonical.
    const SEED: u64 = 0x5EED_1234_ABCD_0001;

    let dir = tempfile::tempdir().expect("tempdir");
    let authored = authored_events();
    let expected = reference_sorted(&authored);
    let physical = shuffled(&authored, SEED);
    assert_ne!(
        physical.iter().map(|e| e.event_id).collect::<Vec<_>>(),
        expected.iter().map(|e| e.event_id).collect::<Vec<_>>(),
        "the shuffle must actually disturb the canonical order"
    );

    let mut store = Store::open(dir.path()).expect("open");
    store.append_all(&physical).expect("append");
    store.sync().expect("sync");

    let read = store.read_log().expect("read");
    assert_eq!(read, physical, "the log preserves physical append order");

    let replayed = linearize(read.into_iter());

    assert_eq!(
        to_bytes(&replayed),
        to_bytes(&expected),
        "replay must be byte-identical to the in-memory reconstruction"
    );
}

#[test]
fn canonical_order_is_exactly_what_the_contract_specifies() {
    let replayed = linearize(authored_events().into_iter());
    let order: Vec<(String, i64, String)> = replayed
        .iter()
        .map(|e| {
            (
                e.producer.clone(),
                e.ts.unix_timestamp(),
                e.event_id.to_string()[24..].to_string(),
            )
        })
        .collect();

    let expected: Vec<(String, i64, String)> = [
        // t=100: producer tiebreak.
        (ADAPTER, 100, "00000000000a"),
        (PROBE, 100, "000000000006"),
        (OBSERVER, 200, "000000000002"),
        // t=300: producer dominates, then event_id inside the adapter.
        (ADAPTER, 300, "000000000005"),
        (ADAPTER, 300, "00000000000b"),
        (OBSERVER, 300, "000000000001"),
        (PROBE, 400, "000000000007"),
        // t=500: one spelled +05:30, one spelled Z; same instant.
        (OBSERVER, 500, "000000000003"),
        (PROBE, 500, "000000000004"),
        (PROBE, 600, "000000000008"),
    ]
    .into_iter()
    .map(|(p, t, s)| (p.to_string(), t, s.to_string()))
    .collect();

    assert_eq!(order, expected);
}

#[test]
fn replay_is_stable_across_many_physical_orders() {
    let authored = authored_events();
    let canonical = to_bytes(&linearize(authored.clone().into_iter()));

    for seed in [1u64, 7, 42, 1337, 0xDEAD_BEEF, 0x0123_4567_89AB_CDEF] {
        let dir = tempfile::tempdir().expect("tempdir");
        let physical = shuffled(&authored, seed);
        let mut store = Store::open(dir.path()).expect("open");
        store.append_all(&physical).expect("append");
        store.sync().expect("sync");
        let replayed = linearize(store.read_log().expect("read").into_iter());
        assert_eq!(
            to_bytes(&replayed),
            canonical,
            "physical order (seed {seed}) leaked into the replay"
        );
    }
}

#[test]
fn derived_state_can_be_dropped_and_rebuilt_from_the_log_alone() {
    const SEED: u64 = 0xC0FF_EE00_1234_5678;

    let dir = tempfile::tempdir().expect("tempdir");
    let authored = authored_events();
    let derived_before = {
        let mut store = Store::open(dir.path()).expect("open");
        for m in manifests() {
            store.register_producer(m).expect("register");
        }
        store
            .append_all(&shuffled(&authored, SEED))
            .expect("append");
        store.sync().expect("sync");

        let events = linearize(store.read_log().expect("read").into_iter());
        Derived::materialize(&events, store.coverage())
    };

    // The `Store` handle above (and with it the in-memory coverage
    // projection) is already out of scope. All that survives is the two
    // append-only files on disk. Rebuild from those alone.
    let store = Store::open(dir.path()).expect("reopen");
    let events = linearize(store.read_log().expect("read").into_iter());
    let derived_after = Derived::materialize(&events, store.coverage());

    assert_eq!(derived_before, derived_after);
    assert!(
        derived_after.unregistered.is_empty(),
        "every producer in the log declared coverage"
    );
    assert_eq!(derived_after.per_computation["comp-1"].len(), 6);
    assert_eq!(derived_after.per_computation["comp-2"].len(), 2);
    assert_eq!(
        derived_after.coverage["comp-1"],
        vec![
            (ADAPTER.to_string(), "0.1.0".to_string()),
            (OBSERVER.to_string(), "0.2.0".to_string()),
        ]
    );
}

#[test]
fn rebuild_ignores_a_wiped_derived_directory() {
    // Derived state living inside the store root must never be load
    // bearing. Simulate a rebuild by deleting everything that is not the
    // canonical log or the coverage registry.
    let dir = tempfile::tempdir().expect("tempdir");
    let authored = authored_events();
    {
        let mut store = Store::open(dir.path()).expect("open");
        for m in manifests() {
            store.register_producer(m).expect("register");
        }
        store.append_all(&authored).expect("append");
        store.sync().expect("sync");
    }
    std::fs::create_dir_all(dir.path().join("derived")).expect("mkdir");
    std::fs::write(dir.path().join("derived/graph.bin"), b"stale garbage").expect("write");

    let before = {
        let store = Store::open(dir.path()).expect("open");
        let events = linearize(store.read_log().expect("read").into_iter());
        Derived::materialize(&events, store.coverage())
    };
    std::fs::remove_dir_all(dir.path().join("derived")).expect("rm");
    let after = {
        let store = Store::open(dir.path()).expect("open");
        let events = linearize(store.read_log().expect("read").into_iter());
        Derived::materialize(&events, store.coverage())
    };
    assert_eq!(before, after);
}

#[test]
fn unknown_payload_fields_survive_the_round_trip_byte_for_byte() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");

    // `proc.spawn` and `net.connect` have no typed payload variant in
    // freshdag-core; their payloads are pure `serde_json::Value`.
    let authored: Vec<IrEvent> = authored_events()
        .into_iter()
        .filter(|e| matches!(e.kind, EventKind::ProcSpawn | EventKind::NetConnect))
        .collect();
    assert_eq!(authored.len(), 2);
    assert!(authored.iter().all(|e| e.decode_payload().is_err()));

    let mut sink = freshdag_store::JsonlSink::open(&path).expect("open");
    let outcomes = sink.append_all(&authored).expect("append");
    assert!(outcomes
        .iter()
        .all(|o| *o == freshdag_store::AppendOutcome::Appended));
    sink.sync().expect("sync");

    let on_disk = std::fs::read(&path).expect("read");
    let read = read_log(&path).expect("read");

    assert_eq!(read, authored, "structural equality");
    assert_eq!(to_bytes(&read), on_disk, "byte-for-byte equality");

    // And the specific unknown fields are still there, not silently
    // dropped by a typed struct that never knew about them.
    let spawn = read
        .iter()
        .find(|e| e.kind == EventKind::ProcSpawn)
        .expect("proc.spawn");
    assert_eq!(spawn.payload["producer_private"]["seccomp_notify_fd"], 7);
    assert_eq!(
        spawn.payload["producer_private"]["nested"],
        serde_json::json!([1, 2, 3])
    );
    let connect = read
        .iter()
        .find(|e| e.kind == EventKind::NetConnect)
        .expect("net.connect");
    assert_eq!(connect.payload["unknown_to_every_struct"]["tls_alpn"], "h2");
}

#[test]
fn an_unrecognized_kind_string_is_reported_not_swallowed() {
    // `EventKind` is a closed enum, so an event whose `kind` string is
    // outside it does NOT round-trip. Documenting the real behavior:
    // the reader surfaces it as a defect rather than skipping the line,
    // which is the invariant #7 requirement. Widening `EventKind` is a
    // contract change and is not the store's call.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.jsonl");
    let raw = br#"{"event_id":"00000000-0000-7000-8000-000000000001","producer":"p","producer_version":"0.1.0","session_id":"s","ts":"1970-01-01T00:00:10Z","kind":"gpu.alloc","payload":{"bytes":1}}
"#;
    std::fs::write(&path, raw).expect("write");

    let scan = freshdag_store::scan_log(&path).expect("scan");
    assert!(scan.events.is_empty());
    assert_eq!(scan.defects.len(), 1);
    assert!(!scan.is_clean());
    assert!(read_log(&path).is_err());
}

#[test]
fn a_clean_replay_reports_no_duplicate_event_ids() {
    let report = linearize_checked(authored_events().into_iter());
    assert!(report.duplicate_event_ids().is_empty());
    assert!(report.is_deterministic());
    assert_eq!(report.events().len(), 10);
}

#[test]
fn a_duplicated_event_id_makes_replay_non_deterministic_and_says_so() {
    let mut authored = authored_events();
    // A producer bug: the same event_id emitted twice with different
    // content. The store must not dedupe it (invariant #4) and must not
    // claim the replay is reproducible (invariant #7).
    let mut clone = authored[0].clone();
    clone.session_id = "sess-2".to_string();
    authored.push(clone);

    let a = linearize_checked(authored.clone().into_iter());
    assert_eq!(a.events().len(), 11, "nothing was dropped");
    assert_eq!(a.duplicate_event_ids().len(), 1);
    assert!(a.duplicate_event_ids()[0].divergent);
    assert!(!a.is_deterministic());
}
