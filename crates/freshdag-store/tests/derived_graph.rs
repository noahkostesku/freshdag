//! The W3 invariant-#5 keystone: derived state is disposable.
//!
//! Author an adversarial event stream → append it in a seeded-shuffled
//! physical order → materialize `derived/` → **delete `derived/`** →
//! replay the canonical log → assert the rebuilt directory is
//! **byte-identical**, file by file.
//!
//! Byte-identical, not structurally equal. A structural `PartialEq` would
//! pass even if the on-disk ordering of a map or vector wobbled between
//! runs, and an unstable derived layout is a determinism bug that would
//! surface later as a certificate that changes for no reason. This
//! follows the precedent set in `tests/reconstruction.rs`.
//!
//! Everything here is deterministic: fixed UUIDs, fixed timestamps,
//! fixed hashes, and a seeded xorshift64* Fisher-Yates shuffle for the
//! physical write order (`.claude/rules/testing.md` — seed randomness,
//! never leave it to the ambient RNG).

use std::collections::{BTreeMap, BTreeSet};

use freshdag_core::artifact::ArtifactId;
use freshdag_core::computation::ComputationId;
use freshdag_core::dependency::{DependencyId, TrustClass};
use freshdag_core::ir::{CoverageManifest, EventKind, EventKindPattern, IrEvent, ProducerRole};
use freshdag_store::{
    DerivedGraph, ExclusionReason, GraphDefect, SilenceMeaning, Store, DERIVED_FILES,
};
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

// ---------------------------------------------------------------- rng

/// xorshift64*. Deterministic, seeded, dependency-free. Same generator as
/// `tests/reconstruction.rs` so the two suites shuffle identically.
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

const COMP_A: &str = "comp-a";
const COMP_B: &str = "comp-b";
const COMP_C: &str = "comp-c";
const COMP_D: &str = "comp-d";
const COMP_E: &str = "comp-e";

const MAIN_RS: &str = "/repo/src/main.rs";
const REPORT_MD: &str = "/repo/out/report.md";
const BUILD_LOG: &str = "/repo/out/build.log";
const BIG_BIN: &str = "/repo/data/big.bin";
const URANDOM: &str = "/dev/urandom";
const CFG_TOML: &str = "/repo/cfg.toml";

/// A valid 64-hex-char BLAKE3 wire hash, distinguished by `tag`.
fn h(tag: &str) -> String {
    assert!(tag.len() <= 64);
    format!("blake3:{tag:0>64}")
}

fn uuid(n: u16) -> Uuid {
    Uuid::parse_str(&format!("00000000-0000-7000-8000-{n:012x}")).expect("fixture uuid")
}

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(secs).expect("fixture timestamp")
}

struct Ev {
    id: u16,
    producer: &'static str,
    version: &'static str,
    comp: Option<&'static str>,
    ts: OffsetDateTime,
    kind: EventKind,
    payload: serde_json::Value,
}

impl From<Ev> for IrEvent {
    fn from(e: Ev) -> Self {
        Self {
            event_id: uuid(e.id),
            producer: e.producer.to_string(),
            producer_version: e.version.to_string(),
            session_id: "sess-1".to_string(),
            computation_id: e.comp.map(ToString::to_string),
            parent_id: None,
            causal_inputs: None,
            ts: e.ts,
            kind: e.kind,
            payload: e.payload,
        }
    }
}

fn read(id: u16, comp: &'static str, secs: i64, path: &str, hash: Option<&str>) -> IrEvent {
    let mut payload = serde_json::json!({ "path": path, "size": 1024u64 });
    if let Some(hash) = hash {
        payload["hash"] = serde_json::Value::String(hash.to_string());
    }
    Ev {
        id,
        producer: OBSERVER,
        version: "0.2.0",
        comp: Some(comp),
        ts: at(secs),
        kind: EventKind::FsRead,
        payload,
    }
    .into()
}

fn write(id: u16, comp: &'static str, secs: i64, path: &str) -> IrEvent {
    Ev {
        id,
        producer: OBSERVER,
        version: "0.2.0",
        comp: Some(comp),
        ts: at(secs),
        kind: EventKind::FsWrite,
        payload: serde_json::json!({
            "path": path,
            "size": 64u64,
            "mode": "create",
            "hash": h("de"),
        }),
    }
    .into()
}

/// The authored event stream.
///
/// Deliberately adversarial. Every derivation rule in
/// `freshdag_store::graph` has at least one witness here, and the
/// ordering-sensitive rules (read-after-own-write, edge conflict,
/// artifact linkage) have witnesses whose classification *changes* if
/// the derivation runs over physical rather than canonical order.
#[allow(clippy::too_many_lines)]
fn authored_events() -> Vec<IrEvent> {
    let t500_utc = at(500);
    let t500_plus = t500_utc.to_offset(UtcOffset::from_hms(5, 30, 0).expect("offset"));
    assert_eq!(t500_utc, t500_plus, "same instant, different spelling");

    vec![
        // ---------------------------------------------------- comp-a
        Ev {
            id: 0x0100,
            producer: ADAPTER,
            version: "0.1.0",
            comp: Some(COMP_A),
            ts: at(100),
            kind: EventKind::ComputationStarted,
            payload: serde_json::json!({ "recipe_id": "build-report" }),
        }
        .into(),
        // A hashed read: the canonical dependency edge.
        read(0x0110, COMP_A, 110, MAIN_RS, Some(&h("a1"))),
        // Bash: no edge, but a coverage obligation the adapter cannot
        // discharge on its own.
        Ev {
            id: 0x0120,
            producer: ADAPTER,
            version: "0.1.0",
            comp: Some(COMP_A),
            ts: at(120),
            kind: EventKind::ToolInvoked,
            payload: serde_json::json!({
                "tool_name": "Bash",
                "tool_kind": "bash",
                "tool_input": { "command": "make report" },
                "cwd": "/repo"
            }),
        }
        .into(),
        Ev {
            id: 0x0130,
            producer: ADAPTER,
            version: "0.1.0",
            comp: Some(COMP_A),
            ts: at(130),
            kind: EventKind::ToolCompleted,
            payload: serde_json::json!({
                "tool_output": "ok", "is_error": false, "duration_ms": 9
            }),
        }
        .into(),
        // Write, then read the same path: internal state, not a dep.
        write(0x0140, COMP_A, 140, BUILD_LOG),
        read(0x0150, COMP_A, 150, BUILD_LOG, Some(&h("bb"))),
        // Read with no hash: unknown state. Not an edge, not forgotten.
        read(0x0160, COMP_A, 160, BIG_BIN, None),
        // Impure read: no meaningful fingerprint or blast radius.
        {
            let mut e = read(0x0170, COMP_A, 170, URANDOM, Some(&h("cc")));
            e.payload["impure"] = serde_json::Value::Bool(true);
            e
        },
        // comp-a's artifact. Later read by comp-b.
        Ev {
            id: 0x0180,
            producer: ADAPTER,
            version: "0.1.0",
            comp: Some(COMP_A),
            ts: at(180),
            kind: EventKind::ArtifactProduced,
            payload: serde_json::json!({
                "artifact_id": "art-a",
                "path": REPORT_MD,
                "content_hash": h("aa"),
                "kind": "text/markdown",
                "produced_by": COMP_A,
                "comparator": "exact"
            }),
        }
        .into(),
        // ---------------------------------------------------- comp-b
        // Reads comp-a's artifact: the edge must carry produced_by.
        read(0x0200, COMP_B, 200, REPORT_MD, Some(&h("aa"))),
        // A versioned external dependency.
        Ev {
            id: 0x0210,
            producer: PROBE,
            version: "0.3.0",
            comp: Some(COMP_B),
            ts: at(210),
            kind: EventKind::ProbeChecked,
            payload: serde_json::json!({
                "scheme": "https",
                "key": "https://example.test/data.json",
                "observed_fingerprint": "etag:\"abc123\"",
                "trust_class": "versioned",
                "result": "match"
            }),
        }
        .into(),
        // Volatile with no TTL: the certificate contract forbids a naked
        // volatile dependency, so this must not become an edge.
        Ev {
            id: 0x0220,
            producer: PROBE,
            version: "0.3.0",
            comp: Some(COMP_B),
            ts: at(220),
            kind: EventKind::ProbeChecked,
            payload: serde_json::json!({
                "scheme": "attio",
                "key": "attio://people/42",
                "observed_fingerprint": "version:7",
                "trust_class": "volatile",
                "result": "match"
            }),
        }
        .into(),
        // Volatile *with* a TTL: legal, becomes an edge.
        Ev {
            id: 0x0230,
            producer: PROBE,
            version: "0.3.0",
            comp: Some(COMP_B),
            ts: at(230),
            kind: EventKind::ProbeChecked,
            payload: serde_json::json!({
                "scheme": "attio",
                "key": "attio://companies/9",
                "observed_fingerprint": "version:3",
                "trust_class": "volatile",
                "ttl_seconds": 300u64,
                "result": "match"
            }),
        }
        .into(),
        Ev {
            id: 0x0240,
            producer: ADAPTER,
            version: "0.1.0",
            comp: Some(COMP_B),
            ts: at(240),
            kind: EventKind::ArtifactProduced,
            payload: serde_json::json!({
                "artifact_id": "art-b",
                "path": "/repo/out/summary.md",
                "content_hash": h("bc"),
                "kind": "text/markdown",
                "produced_by": COMP_B,
                "comparator": "exact"
            }),
        }
        .into(),
        // ---------------------------------------------------- comp-c
        // Shares MAIN_RS with comp-a. Produces no artifact: "consumed,
        // nothing produced yet" must not read as "nothing depends on it."
        read(0x0300, COMP_C, 300, MAIN_RS, Some(&h("a1"))),
        // The same key re-observed with a different hash mid-computation:
        // the world moved. First canonical observation wins; the
        // divergence is recorded as a conflict.
        read(0x0310, COMP_C, 310, MAIN_RS, Some(&h("a2"))),
        // Network I/O the store deliberately does not classify.
        Ev {
            id: 0x0320,
            producer: OBSERVER,
            version: "0.2.0",
            comp: Some(COMP_C),
            ts: at(320),
            kind: EventKind::NetFetch,
            payload: serde_json::json!({
                "url": "https://example.test/blob",
                "method": "GET",
                "status": 200,
                "etag": "\"zz\""
            }),
        }
        .into(),
        // ---------------------------------------------------- comp-d
        // read → write → read on ONE path. The first read is a genuine
        // external dependency; the second is internal state. This is the
        // witness that ordering is load-bearing.
        read(0x0400, COMP_D, 400, CFG_TOML, Some(&h("d1"))),
        write(0x0410, COMP_D, 410, CFG_TOML),
        read(0x0420, COMP_D, 420, CFG_TOML, Some(&h("d2"))),
        // ---------------------------------------------------- comp-e
        // Adapter-only. Ran Bash; no producer covering fs.* contributed.
        // Zero fs.read events here means UNOBSERVED, not "read nothing."
        Ev {
            id: 0x0500,
            producer: ADAPTER,
            version: "0.1.0",
            comp: Some(COMP_E),
            ts: at(500),
            kind: EventKind::ToolInvoked,
            payload: serde_json::json!({
                "tool_name": "Task",
                "tool_kind": "task",
                "tool_input": { "prompt": "summarize" }
            }),
        }
        .into(),
        // Same instant, different offset spelling: must tie on `ts` and
        // fall through to the producer tiebreak, deterministically.
        Ev {
            id: 0x0510,
            producer: PROBE,
            version: "0.3.0",
            comp: None,
            ts: t500_plus,
            kind: EventKind::Diagnostic,
            payload: serde_json::json!({ "message": "probe warm-up" }),
        }
        .into(),
        // ------------------------------------------------- orphan
        // An `fs.read` with NO computation_id. The contract requires one
        // on any event contributing to a dependency edge, so this is a
        // producer bug that must be reported, not swallowed.
        {
            let mut e = read(0x0600, COMP_A, 600, "/repo/orphan.txt", Some(&h("ee")));
            e.computation_id = None;
            e
        },
        // ------------------------------------------------ session end
        Ev {
            id: 0x0700,
            producer: PROBE,
            version: "0.3.0",
            comp: None,
            ts: at(700),
            kind: EventKind::SessionEnded,
            payload: serde_json::json!({ "reason": "complete" }),
        }
        .into(),
    ]
}

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
        platforms: vec!["linux-x86_64".to_string()],
        emits: emits.iter().map(|p| EventKindPattern::new(*p)).collect(),
        partial: BTreeMap::new(),
        capabilities: BTreeMap::new(),
        known_limitations: Vec::new(),
    }
}

fn manifests() -> Vec<CoverageManifest> {
    let mut observer = manifest(OBSERVER, "0.2.0", &["fs.*", "proc.*", "net.*"]);
    observer.partial.insert(
        "fs.dirlist".to_string(),
        "directory listings are sampled, not exhaustive".to_string(),
    );
    observer.known_limitations = vec!["mmap reads are pessimistic".to_string()];
    vec![
        manifest(ADAPTER, "0.1.0", &["computation.*", "tool.*", "artifact.*"]),
        observer,
        manifest(PROBE, "0.3.0", &["probe.*", "session.*", "diagnostic"]),
    ]
}

/// Build a store in `dir`, register every manifest, and append the
/// authored events in a seeded-shuffled physical order.
fn seeded_store(dir: &std::path::Path, seed: u64) -> Store {
    let mut store = Store::open(dir).expect("open");
    for m in manifests() {
        store.register_producer(m).expect("register");
    }
    let physical = shuffled(&authored_events(), seed);
    store.append_all(&physical).expect("append");
    store.sync().expect("sync");
    store
}

/// Read every file in a derived directory as raw bytes.
fn derived_bytes(dir: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        out.insert(name, std::fs::read(entry.path()).expect("read file"));
    }
    out
}

fn comp(id: &str) -> ComputationId {
    ComputationId(id.to_string())
}

fn dep(key: &str) -> DependencyId {
    DependencyId(key.to_string())
}

// ------------------------------------------------- the keystone tests

#[test]
fn derived_state_drops_and_rebuilds_byte_identically() {
    const SEED: u64 = 0xD3E1_7ED6_0A21_1001;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = seeded_store(dir.path(), SEED);

    // Sanity: the physical order really is not the canonical order.
    let physical = store.read_log().expect("read");
    let canonical = freshdag_store::linearize(physical.clone().into_iter());
    assert_ne!(
        physical.iter().map(|e| e.event_id).collect::<Vec<_>>(),
        canonical.iter().map(|e| e.event_id).collect::<Vec<_>>(),
        "the shuffle must actually disturb the canonical order"
    );

    store.rebuild_derived().expect("build derived");
    let before = derived_bytes(&store.derived_dir());
    assert_eq!(
        before.keys().cloned().collect::<BTreeSet<_>>(),
        DERIVED_FILES
            .iter()
            .map(|s| (*s).to_string())
            .collect::<BTreeSet<_>>(),
        "every declared derived file was written"
    );

    // Drop derived state entirely. All that survives is the two
    // append-only files.
    store.drop_derived().expect("drop");
    assert!(!store.derived_dir().exists());
    assert!(store.load_derived().expect("load").is_none());

    // Reopen from disk alone — the in-memory coverage projection from the
    // handle above is irrelevant, it is rebuilt from coverage.jsonl.
    let reopened = Store::open(dir.path()).expect("reopen");
    reopened.rebuild_derived().expect("rebuild derived");
    let after = derived_bytes(&reopened.derived_dir());

    assert_eq!(
        before, after,
        "rebuilt derived state must be byte-identical to the dropped one"
    );
}

#[test]
fn physical_order_never_leaks_into_derived_state() {
    let mut reference: Option<BTreeMap<String, Vec<u8>>> = None;

    for seed in [1u64, 7, 42, 1337, 0xDEAD_BEEF, 0x0123_4567_89AB_CDEF] {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = seeded_store(dir.path(), seed);
        store.rebuild_derived().expect("build derived");
        let bytes = derived_bytes(&store.derived_dir());
        match &reference {
            None => reference = Some(bytes),
            Some(expected) => assert_eq!(
                &bytes, expected,
                "physical order (seed {seed}) leaked into the derived graph"
            ),
        }
    }
}

#[test]
fn a_wiped_derived_directory_is_rebuilt_from_the_log_alone() {
    const SEED: u64 = 0x0BAD_CAFE_0000_0007;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = seeded_store(dir.path(), SEED);
    let graph = store.rebuild_derived().expect("build");

    // Vandalize the derived directory: stale garbage and a deleted file.
    std::fs::write(store.derived_dir().join("computations.jsonl"), b"garbage").expect("vandalize");
    std::fs::write(store.derived_dir().join("stray.bin"), b"not ours").expect("stray");

    // A rebuild replaces the directory wholesale; the stray file is gone.
    let rebuilt = store.rebuild_derived().expect("rebuild");
    assert!(!store.derived_dir().join("stray.bin").exists());
    assert_eq!(
        graph.computations().collect::<Vec<_>>(),
        rebuilt.computations().collect::<Vec<_>>()
    );
}

#[test]
fn a_derived_directory_is_a_cache_not_an_authority() {
    const SEED: u64 = 0x1111_2222_3333_4444;

    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = seeded_store(dir.path(), SEED);
    store.rebuild_derived().expect("build");

    let (_, digest_before) = store.derive_graph().expect("derive");
    let loaded = store.load_derived().expect("load").expect("present");
    assert!(loaded.matches(&digest_before));

    // Append one more observation. The derived directory is now stale,
    // and it must say so rather than quietly answering from the cache.
    let extra = read(0x0900, COMP_A, 900, "/repo/late.txt", Some(&h("f1")));
    assert_eq!(
        store.append(&extra).expect("append"),
        freshdag_store::AppendOutcome::Appended
    );
    store.sync().expect("sync");

    let (_, digest_after) = store.derive_graph().expect("derive");
    assert_ne!(digest_before, digest_after);
    let stale = store.load_derived().expect("load").expect("present");
    assert!(
        !stale.matches(&digest_after),
        "a stale derived directory must not claim to match the log"
    );
}

// -------------------------------------------- edge-derivation rules

fn built_graph() -> DerivedGraph {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = seeded_store(dir.path(), 0x5EED_0000_0000_0001);
    store.derive_graph().expect("derive").0
}

#[test]
fn a_hashed_read_is_an_exact_file_dependency() {
    let graph = built_graph();
    let deps = graph.dependencies_of(&comp(COMP_A));
    let main = deps
        .iter()
        .find(|d| d.key == format!("file://{MAIN_RS}"))
        .expect("main.rs edge");
    assert_eq!(main.scheme, "file");
    assert_eq!(main.trust_class, TrustClass::Exact);
    assert_eq!(main.fingerprint.to_string(), h("a1"));
    assert!(main.is_consistent());
    assert!(
        deps.iter()
            .all(freshdag_core::dependency::Dependency::is_consistent),
        "no self-inconsistent dependency may enter the graph"
    );
}

#[test]
fn read_after_own_write_is_internal_state_not_a_dependency() {
    let graph = built_graph();
    let node = graph.computation(&comp(COMP_A)).expect("comp-a");

    assert!(
        !node
            .dependencies
            .iter()
            .any(|d| d.key == format!("file://{BUILD_LOG}")),
        "a read of a path this computation wrote is not an external dependency"
    );
    let excluded = node
        .excluded
        .iter()
        .find(|e| e.key.as_ref() == Some(&dep(&format!("file://{BUILD_LOG}"))))
        .expect("build.log exclusion recorded");
    assert_eq!(excluded.reason, ExclusionReason::ReadAfterOwnWrite);

    // The write itself is an output.
    assert!(node
        .outputs
        .iter()
        .any(|o| o.path.to_string_lossy() == BUILD_LOG));

    // And it does NOT appear in the reverse index at all: this is a
    // positive finding of "no external dependency here," so widening the
    // blast radius with it would be wrong.
    assert!(graph
        .blast_radius(&dep(&format!("file://{BUILD_LOG}")))
        .is_none());
}

#[test]
fn a_read_before_the_computations_own_write_is_still_a_dependency() {
    let graph = built_graph();
    let node = graph.computation(&comp(COMP_D)).expect("comp-d");

    // read(t=400) → write(t=410) → read(t=420) on the same path.
    let edge = node
        .dependencies
        .iter()
        .find(|d| d.key == format!("file://{CFG_TOML}"))
        .expect("the pre-write read is a dependency");
    assert_eq!(
        edge.fingerprint.to_string(),
        h("d1"),
        "the retained edge is the read that happened BEFORE the write"
    );
    assert_eq!(
        node.excluded
            .iter()
            .filter(|e| e.reason == ExclusionReason::ReadAfterOwnWrite)
            .count(),
        1,
        "only the post-write read is excluded"
    );
    assert_eq!(node.outputs.len(), 1);
}

#[test]
fn an_unfingerprinted_read_is_unproven_not_absent() {
    let graph = built_graph();
    let node = graph.computation(&comp(COMP_A)).expect("comp-a");
    let key = format!("file://{BIG_BIN}");

    assert!(!node.dependencies.iter().any(|d| d.key == key));
    let excluded = node
        .excluded
        .iter()
        .find(|e| e.key.as_ref() == Some(&dep(&key)))
        .expect("recorded");
    assert_eq!(excluded.reason, ExclusionReason::NoFingerprint);
    assert!(excluded.reason.is_unproven_dependency());

    // It has no proven consumers, but it is NOT absent from the blast
    // radius — under-reporting is the dangerous direction.
    let entry = graph.blast_radius(&dep(&key)).expect("reverse index entry");
    assert!(entry.consuming_computations.is_empty());
    assert_eq!(entry.unproven_computations, vec![comp(COMP_A)]);
    assert_eq!(
        entry.unproven_consumers,
        vec![ArtifactId("art-a".to_string())]
    );
    assert!(!entry.is_fully_proven());
}

#[test]
fn an_impure_read_is_excluded_and_carries_no_blast_radius() {
    let graph = built_graph();
    let node = graph.computation(&comp(COMP_A)).expect("comp-a");
    let key = format!("file://{URANDOM}");

    let excluded = node
        .excluded
        .iter()
        .find(|e| e.key.as_ref() == Some(&dep(&key)))
        .expect("recorded");
    assert_eq!(excluded.reason, ExclusionReason::Impure);
    assert!(!excluded.reason.is_unproven_dependency());
    assert!(
        graph.blast_radius(&dep(&key)).is_none(),
        "/dev/urandom has no meaningful blast radius"
    );
}

#[test]
fn bash_and_task_create_obligations_not_edges() {
    let graph = built_graph();

    let a = graph.computation(&comp(COMP_A)).expect("comp-a");
    assert!(a.has_coverage_obligation());
    assert_eq!(a.obligations.len(), 1);
    assert_eq!(a.obligations[0].tool_kind, "bash");
    assert_eq!(a.obligations[0].tool_name, "Bash");

    let e = graph.computation(&comp(COMP_E)).expect("comp-e");
    assert_eq!(e.obligations.len(), 1);
    assert_eq!(e.obligations[0].tool_kind, "task");
    assert!(
        e.dependencies.is_empty(),
        "a task invocation is not a dependency edge"
    );

    // `tool.completed` and other non-bash tool traffic is counted, not
    // silently dropped.
    assert_eq!(a.unmodeled.get("tool.completed").copied(), Some(1));
}

#[test]
fn a_naked_volatile_probe_result_is_excluded_but_a_ttl_one_is_not() {
    let graph = built_graph();
    let node = graph.computation(&comp(COMP_B)).expect("comp-b");

    let excluded = node
        .excluded
        .iter()
        .find(|e| e.key.as_ref() == Some(&dep("attio://people/42")))
        .expect("naked volatile recorded");
    assert_eq!(excluded.reason, ExclusionReason::NakedVolatile);

    let ttl_edge = node
        .dependencies
        .iter()
        .find(|d| d.key == "attio://companies/9")
        .expect("volatile-with-ttl edge");
    assert_eq!(ttl_edge.trust_class, TrustClass::Volatile);
    assert_eq!(ttl_edge.ttl_seconds, Some(300));
    assert!(ttl_edge.is_consistent());

    let versioned = node
        .dependencies
        .iter()
        .find(|d| d.key == "https://example.test/data.json")
        .expect("https edge");
    assert_eq!(versioned.trust_class, TrustClass::Versioned);
    assert_eq!(versioned.fingerprint.to_string(), "etag:\"abc123\"");
}

#[test]
fn net_fetch_is_visible_but_deliberately_unclassified() {
    let graph = built_graph();
    let node = graph.computation(&comp(COMP_C)).expect("comp-c");
    assert_eq!(node.unmodeled.get("net.fetch").copied(), Some(1));
    assert!(
        !node.dependencies.iter().any(|d| d.scheme == "https"),
        "trust classing an HTTP response belongs to the probe contract"
    );
}

#[test]
fn a_downstream_read_links_back_to_the_producing_artifact() {
    let graph = built_graph();
    let node = graph.computation(&comp(COMP_B)).expect("comp-b");
    let edge = node
        .dependencies
        .iter()
        .find(|d| d.key == format!("file://{REPORT_MD}"))
        .expect("report.md edge");
    assert_eq!(
        edge.produced_by,
        Some(ArtifactId("art-a".to_string())),
        "invariant #9: an artifact is traceable to the computation that produced it"
    );
}

#[test]
fn a_re_observation_that_disagrees_is_recorded_as_a_conflict() {
    let graph = built_graph();
    let node = graph.computation(&comp(COMP_C)).expect("comp-c");

    assert_eq!(
        node.dependencies
            .iter()
            .filter(|d| d.key == format!("file://{MAIN_RS}"))
            .count(),
        1,
        "the edge is deduped by DependencyId"
    );
    assert_eq!(node.conflicts.len(), 1);
    let conflict = &node.conflicts[0];
    assert_eq!(conflict.dependency, dep(&format!("file://{MAIN_RS}")));
    assert_eq!(conflict.first_fingerprint.to_string(), h("a1"));
    assert_eq!(conflict.conflicting_fingerprint.to_string(), h("a2"));
    assert_eq!(conflict.first_event_id, uuid(0x0300));
    assert_eq!(conflict.conflicting_event_id, uuid(0x0310));
}

// ------------------------------------------------------ blast radius

#[test]
fn blast_radius_names_every_consuming_artifact() {
    let graph = built_graph();
    let entry = graph
        .blast_radius(&dep(&format!("file://{MAIN_RS}")))
        .expect("main.rs entry");

    assert_eq!(entry.scheme, "file");
    assert_eq!(
        entry.consuming_computations,
        vec![comp(COMP_A), comp(COMP_C)]
    );
    // comp-c produced nothing yet, so only comp-a's artifact appears.
    assert_eq!(entry.consumers, vec![ArtifactId("art-a".to_string())]);
    assert!(entry.is_fully_proven());
    assert!(
        !entry.consuming_computations.is_empty() && entry.consumers.len() == 1,
        "an empty consumer list with a non-empty computation list means \
         'nothing produced yet', not 'nothing depends on it'"
    );
}

#[test]
fn the_reverse_index_covers_probe_dependencies_too() {
    let graph = built_graph();
    let entry = graph
        .blast_radius(&dep("https://example.test/data.json"))
        .expect("https entry");
    assert_eq!(entry.scheme, "https");
    assert_eq!(entry.consuming_computations, vec![comp(COMP_B)]);
    assert_eq!(entry.consumers, vec![ArtifactId("art-b".to_string())]);
}

// ------------------------------------------------------- attribution

#[test]
fn attribution_names_only_producers_that_actually_emitted() {
    let graph = built_graph();

    let a = graph.attribution(&comp(COMP_A)).expect("comp-a coverage");
    let producers: Vec<&str> = a.entries.iter().map(|e| e.producer.as_str()).collect();
    assert_eq!(producers, vec![ADAPTER, OBSERVER]);
    assert!(a.unregistered.is_empty());

    // comp-e is adapter-only. The probe and observer registered manifests
    // but emitted nothing for it, so they are NOT attributed to it.
    let e = graph.attribution(&comp(COMP_E)).expect("comp-e coverage");
    assert_eq!(
        e.entries
            .iter()
            .map(|c| c.producer.as_str())
            .collect::<Vec<_>>(),
        vec![ADAPTER]
    );

    // comp-b's probe events attribute the probe.
    let b = graph.attribution(&comp(COMP_B)).expect("comp-b coverage");
    assert!(b.entries.iter().any(|c| c.producer == PROBE));
    assert!(b.entries.iter().any(|c| c.role == ProducerRole::Probe));
}

#[test]
fn silence_is_not_absence() {
    let graph = built_graph();

    // comp-e: only the adapter contributed, and the adapter declares no
    // fs.* coverage. Zero fs.read events means UNOBSERVED.
    assert_eq!(
        graph.silence(&comp(COMP_E), EventKind::FsRead),
        Some(SilenceMeaning::Unobserved),
        "an adapter's silence about the filesystem is not evidence of no reads"
    );

    // comp-a: the fsatrace observer contributed and declares fs.*, so
    // zero fs.stat events is evidence of absence — bounded by fidelity.
    assert_eq!(
        graph.silence(&comp(COMP_A), EventKind::FsStat),
        Some(SilenceMeaning::ObservedAbsent)
    );

    // fs.dirlist is declared partial by that observer: treat with the
    // same suspicion as unobserved.
    assert_eq!(
        graph.silence(&comp(COMP_A), EventKind::FsDirlist),
        Some(SilenceMeaning::PartiallyObserved(vec![
            "directory listings are sampled, not exhaustive".to_string()
        ]))
    );

    // Not silent at all: comp-a has fs.read events.
    assert_eq!(graph.silence(&comp(COMP_A), EventKind::FsRead), None);
}

#[test]
fn an_unregistered_producer_makes_every_silence_uninterpretable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    // Register the observer only. The adapter emits without a manifest.
    for m in manifests().into_iter().filter(|m| m.producer == OBSERVER) {
        store.register_producer(m).expect("register");
    }
    store.append_all(&authored_events()).expect("append");
    store.sync().expect("sync");

    let (graph, _) = store.derive_graph().expect("derive");
    let a = graph.attribution(&comp(COMP_A)).expect("comp-a");
    assert_eq!(a.unregistered.len(), 1);
    assert_eq!(a.unregistered[0].producer, ADAPTER);

    // Even though the observer covers fs.*, an unreadable producer means
    // no silence can be interpreted for this computation.
    assert_eq!(
        graph.silence(&comp(COMP_A), EventKind::FsStat),
        Some(SilenceMeaning::Unobserved)
    );
}

// ----------------------------------------------------------- defects

#[test]
fn an_edge_bearing_event_without_a_computation_id_is_reported() {
    let graph = built_graph();
    let orphans: Vec<_> = graph
        .defects()
        .iter()
        .filter(|d| matches!(d, GraphDefect::OrphanEdgeEvent { .. }))
        .collect();
    assert_eq!(orphans.len(), 1);
    match orphans[0] {
        GraphDefect::OrphanEdgeEvent { event_id, kind, .. } => {
            assert_eq!(*event_id, uuid(0x0600));
            assert_eq!(*kind, EventKind::FsRead);
        }
        other => panic!("unexpected defect {other:?}"),
    }

    // The `diagnostic` and `session.ended` events also lack a
    // computation_id, and those are legitimately infrastructural.
    assert_eq!(graph.defects().len(), 1);
}

#[test]
fn a_divergent_duplicate_event_id_makes_the_graph_say_so() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");
    for m in manifests() {
        store.register_producer(m).expect("register");
    }
    let mut events = authored_events();
    let mut twin = events[1].clone();
    twin.payload["size"] = serde_json::json!(999_999u64);
    events.push(twin);
    store.append_all(&events).expect("append");
    store.sync().expect("sync");

    let (graph, _) = store.derive_graph().expect("derive");
    assert!(
        !graph.is_deterministic(),
        "a divergent duplicate must not be presented as a reproducible replay"
    );
    assert!(graph
        .defects()
        .iter()
        .any(|d| matches!(d, GraphDefect::DivergentDuplicate { .. })));
    assert_eq!(graph.duplicate_event_ids(), &[uuid(0x0110)]);

    // And it is reflected on disk.
    store.rebuild_derived().expect("build");
    let loaded = store.load_derived().expect("load").expect("present");
    assert!(!loaded.manifest.deterministic);
}

#[test]
fn every_event_with_a_computation_id_is_accounted_for() {
    let graph = built_graph();
    assert!(
        graph.accounting_is_balanced(),
        "an event fell out of the accounting — an observation went missing"
    );

    let authored = authored_events();
    let attributed: usize = graph.computations().map(|n| n.accounting.total).sum();
    let with_comp = authored
        .iter()
        .filter(|e| e.computation_id.is_some())
        .count();
    assert_eq!(attributed, with_comp);
    assert_eq!(graph.event_count(), authored.len());
}

// -------------------------------------------------- round-trip shape

#[test]
fn the_derived_directory_round_trips_through_disk() {
    const SEED: u64 = 0xFACE_0FF1_CE00_0001;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = seeded_store(dir.path(), SEED);
    let graph = store.rebuild_derived().expect("build");
    let loaded = store.load_derived().expect("load").expect("present");

    assert_eq!(loaded.manifest.format, freshdag_store::DERIVED_FORMAT);
    assert_eq!(loaded.computations.len(), graph.computations().count());
    assert_eq!(loaded.reverse_index.len(), graph.reverse_index().count());
    assert_eq!(loaded.attribution.len(), graph.attributions().count());
    assert_eq!(loaded.defects.len(), graph.defects().len());

    // Structural round-trip: what came back equals what went out.
    assert_eq!(
        loaded.computations,
        graph.computations().cloned().collect::<Vec<_>>()
    );
    assert_eq!(
        loaded.reverse_index,
        graph.reverse_index().cloned().collect::<Vec<_>>()
    );
}

#[test]
fn a_damaged_derived_directory_is_an_error_not_an_empty_graph() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = seeded_store(dir.path(), 0x0002_0003_0004_0005);
    store.rebuild_derived().expect("build");

    std::fs::write(store.derived_dir().join("manifest.json"), b"{oops").expect("damage");
    assert!(
        store.load_derived().is_err(),
        "a damaged derived directory must not read as 'no derived state'"
    );
}
