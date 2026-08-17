//! End-to-end tests for `freshdag mark`.
//!
//! `mark` is the one command that *writes* a claim into the log, so its
//! refusals matter more than its successes. Each test below pins a case
//! where minting an artifact would assert something the store cannot
//! support.

use std::path::{Path, PathBuf};
use std::process::Command;

use freshdag_core::ir::{CoverageManifest, EventKind, EventKindPattern, IrEvent, ProducerRole};
use freshdag_store::{AppendOutcome, Store};
use time::OffsetDateTime;
use uuid::Uuid;

const ADAPTER: &str = "freshdag-adapter-claude";
const VERSION: &str = "0.1.0";
const SESSION: &str = "sess-mark";
const COMPUTATION: &str = "comp:mark-test";
const CONTENT: &str = "{\"ok\":true}\n";

fn target_tmp() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mark")
}

fn binary() -> PathBuf {
    // The integration-test binary lives in `target/<profile>/deps/`.
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("freshdag")
}

fn blake3_of(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// A store with one computation that read nothing and wrote `report.json`.
struct Fixture {
    root: PathBuf,
    store_dir: PathBuf,
    written: PathBuf,
}

impl Fixture {
    /// `recorded` is what the log claims was written; the file on disk
    /// gets `on_disk`. Passing different values stages content drift.
    fn new(name: &str, recorded: Option<&str>, on_disk: &str) -> Self {
        let root = target_tmp().join(name);
        // A directory left by a previous run would make this test lie.
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scenario root");
        let store_dir = root.join(".freshdag");
        let written = root.join("report.json");
        std::fs::write(&written, on_disk).expect("write artifact file");

        let mut store = Store::open(&store_dir).expect("open store");
        store
            .register_producer(manifest())
            .expect("register adapter manifest");

        let mut payload = serde_json::json!({
            "path": written.display().to_string(),
            "mode": "truncate",
            "observation": "pre-execution-intent",
            "size": on_disk.len(),
        });
        if let Some(recorded) = recorded {
            payload["hash"] = serde_json::json!(blake3_of(recorded.as_bytes()));
        }
        assert_eq!(
            store
                .append(&event(0, EventKind::FsWrite, payload))
                .expect("append fs.write"),
            AppendOutcome::Appended,
            "the fixture's own write must not be dropped"
        );
        store.sync().expect("sync");

        Self {
            root,
            store_dir,
            written,
        }
    }

    fn mark(&self, path: &Path) -> std::process::Output {
        Command::new(binary())
            .arg("mark")
            .arg("--store")
            .arg(&self.store_dir)
            .arg(path)
            .output()
            .expect("run freshdag mark")
    }
}

fn event(index: u128, kind: EventKind, payload: serde_json::Value) -> IrEvent {
    IrEvent {
        event_id: Uuid::from_u128(0x7000_0000_0000_0000_0000 + index),
        producer: ADAPTER.to_string(),
        producer_version: VERSION.to_string(),
        session_id: SESSION.to_string(),
        computation_id: Some(COMPUTATION.to_string()),
        parent_id: None,
        causal_inputs: None,
        ts: OffsetDateTime::UNIX_EPOCH,
        kind,
        payload,
    }
}

fn manifest() -> CoverageManifest {
    CoverageManifest {
        producer: ADAPTER.to_string(),
        version: VERSION.to_string(),
        role: ProducerRole::Adapter,
        platforms: Vec::new(),
        emits: vec![
            EventKindPattern::new("fs.read"),
            EventKindPattern::new("fs.write"),
        ],
        partial: std::collections::BTreeMap::new(),
        capabilities: std::collections::BTreeMap::new(),
        known_limitations: Vec::new(),
    }
}

/// The happy path: a recorded write whose bytes still match yields an
/// artifact attributed to the recording computation.
#[test]
fn marking_a_recorded_write_records_an_artifact() {
    let fixture = Fixture::new("records", Some(CONTENT), CONTENT);
    let out = fixture.mark(&fixture.written);
    assert!(
        out.status.success(),
        "mark failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let store = Store::open(&fixture.store_dir).expect("reopen store");
    let events = store.read_log().expect("read log");
    let artifact = events
        .iter()
        .find(|e| e.kind == EventKind::ArtifactProduced)
        .expect("an artifact.produced event was appended");

    assert_eq!(
        artifact.computation_id.as_deref(),
        Some(COMPUTATION),
        "the artifact is attributed to the computation that wrote the file"
    );
    assert_eq!(
        artifact.payload["content_hash"].as_str(),
        Some(blake3_of(CONTENT.as_bytes()).as_str()),
        "content_hash is the bytes on disk"
    );
    assert_eq!(
        artifact.payload["artifact_id"], artifact.payload["content_hash"],
        "artifact identity is content-addressed"
    );
    // `execution-ir.md §Event Payloads` requires `kind`, and the engine
    // refuses a certificate without it.
    assert_eq!(
        artifact.payload["kind"].as_str(),
        Some("application/json"),
        "kind is derived from the extension"
    );

    // The CLI must also have published its own manifest, or its event
    // would make every artifact `producer-missing-from-coverage`.
    assert!(
        store
            .coverage()
            .manifests()
            .any(|(k, _)| k.producer == "freshdag-cli"),
        "mark publishes the CLI's coverage manifest"
    );
}

/// Refusing is the point: with no recorded write there is no
/// computation to attribute the artifact to, and inventing one would
/// fabricate provenance.
#[test]
fn marking_a_file_the_store_never_saw_written_is_refused() {
    let fixture = Fixture::new("unrecorded", Some(CONTENT), CONTENT);
    let stranger = fixture.root.join("not-produced-here.json");
    std::fs::write(&stranger, CONTENT).expect("write stranger");

    let out = fixture.mark(&stranger);
    assert!(
        !out.status.success(),
        "marking an unrecorded file must fail"
    );
    assert!(
        out.status.code().is_some_and(|c| c > 2),
        "a refusal is a tool error, not a validity verdict; got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nothing in this store records writing"),
        "stderr should say why: {stderr}"
    );

    let store = Store::open(&fixture.store_dir).expect("reopen store");
    assert!(
        !store
            .read_log()
            .expect("read log")
            .iter()
            .any(|e| e.kind == EventKind::ArtifactProduced),
        "a refused mark must append nothing"
    );
}

/// The file changed after the computation wrote it, so that computation
/// did not produce these bytes. Recording it anyway would attach real
/// provenance to content it never saw — an invariant-#7 failure on the
/// artifact's own identity.
#[test]
fn marking_a_file_that_drifted_since_its_write_is_refused() {
    let fixture = Fixture::new("drifted", Some(CONTENT), "{\"ok\":false}\n");

    let out = fixture.mark(&fixture.written);
    assert!(!out.status.success(), "drifted content must not be marked");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("has changed since the computation that wrote it"),
        "stderr should name the drift: {stderr}"
    );

    let store = Store::open(&fixture.store_dir).expect("reopen store");
    assert!(
        !store
            .read_log()
            .expect("read log")
            .iter()
            .any(|e| e.kind == EventKind::ArtifactProduced),
        "a refused mark must append nothing"
    );
}

/// `Edit`/`MultiEdit` hook payloads carry only a splice, so the adapter
/// emits `fs.write` with no hash. There is nothing to compare against,
/// and refusing every edited file would make `mark` useless — so this
/// proceeds, and the artifact still gets the bytes actually on disk.
#[test]
fn a_write_with_no_recorded_hash_is_markable() {
    let fixture = Fixture::new("no-hash", None, CONTENT);
    let out = fixture.mark(&fixture.written);
    assert!(
        out.status.success(),
        "mark failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let store = Store::open(&fixture.store_dir).expect("reopen store");
    let events = store.read_log().expect("read log");
    let artifact = events
        .iter()
        .find(|e| e.kind == EventKind::ArtifactProduced)
        .expect("artifact recorded");
    assert_eq!(
        artifact.payload["content_hash"].as_str(),
        Some(blake3_of(CONTENT.as_bytes()).as_str())
    );
}
