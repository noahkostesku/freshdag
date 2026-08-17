//! End-to-end tests for `freshdag coverage`.
//!
//! This command is a measuring instrument, and the observer investment
//! decision turns on what it reports. A coverage report that flatters
//! the system is worse than no report, so these tests pin the
//! pessimistic direction: blind tool calls count against coverage,
//! obligations are undischargeable until an `Observer` says otherwise,
//! and metrics that cannot be computed are named rather than
//! approximated.

use std::path::PathBuf;
use std::process::Command;

use freshdag_core::ir::{CoverageManifest, EventKind, EventKindPattern, IrEvent, ProducerRole};
use freshdag_store::{AppendOutcome, Store};
use time::OffsetDateTime;
use uuid::Uuid;

const ADAPTER: &str = "freshdag-adapter-claude";
const OBSERVER: &str = "freshdag-observer-fsatrace";
const VERSION: &str = "0.1.0";
const SESSION: &str = "sess-coverage";
const COMPUTATION: &str = "comp:coverage-test";

fn target_tmp() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("coverage")
}

fn binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("freshdag")
}

fn event(index: u128, kind: EventKind, payload: serde_json::Value) -> IrEvent {
    event_from(ADAPTER, index, kind, payload)
}

fn event_from(producer: &str, index: u128, kind: EventKind, payload: serde_json::Value) -> IrEvent {
    IrEvent {
        event_id: Uuid::from_u128(0x7000_0000_0000_0000_0000 + index),
        producer: producer.to_string(),
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

fn manifest(producer: &str, role: ProducerRole, emits: &[&str]) -> CoverageManifest {
    CoverageManifest {
        producer: producer.to_string(),
        version: VERSION.to_string(),
        role,
        platforms: Vec::new(),
        emits: emits.iter().map(|s| EventKindPattern::new(*s)).collect(),
        partial: std::collections::BTreeMap::new(),
        capabilities: std::collections::BTreeMap::new(),
        known_limitations: Vec::new(),
    }
}

/// A store with two bash calls (blind), one Read (visible) that
/// fingerprinted, and one Read that did not.
fn fixture(name: &str, with_observer: bool) -> PathBuf {
    let root = target_tmp().join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scenario root");
    let store_dir = root.join(".freshdag");

    let mut store = Store::open(&store_dir).expect("open store");
    store
        .register_producer(manifest(
            ADAPTER,
            ProducerRole::Adapter,
            &["tool.*", "fs.read", "fs.write"],
        ))
        .expect("register adapter");
    if with_observer {
        store
            .register_producer(manifest(
                OBSERVER,
                ProducerRole::Observer,
                &["fs.read", "fs.write"],
            ))
            .expect("register observer");
    }

    let mut index = 0u128;
    let append = |store: &mut Store, e: IrEvent| {
        assert_eq!(
            store.append(&e).expect("append"),
            AppendOutcome::Appended,
            "fixture events must not be dropped"
        );
    };

    for _ in 0..2 {
        append(
            &mut store,
            event(
                index,
                EventKind::ToolInvoked,
                serde_json::json!({"tool_kind": "bash", "tool_name": "bash"}),
            ),
        );
        index += 1;
    }
    append(
        &mut store,
        event(
            index,
            EventKind::ToolInvoked,
            serde_json::json!({"tool_kind": "builtin", "tool_name": "Read"}),
        ),
    );
    index += 1;

    // One read that carries a fingerprint -> becomes an edge.
    append(
        &mut store,
        event(
            index,
            EventKind::FsRead,
            serde_json::json!({
                "path": "/tmp/seen.txt",
                "size": 3,
                "hash": "blake3:1111111111111111111111111111111111111111111111111111111111111111",
                "read_kind": "direct",
                "impure": false,
                "observation": "pre-execution-intent",
            }),
        ),
    );
    index += 1;

    // One read with no fingerprint -> excluded, and still unproven.
    append(
        &mut store,
        event(
            index,
            EventKind::FsRead,
            serde_json::json!({
                "path": "/tmp/unseen.txt",
                "size": 0,
                "read_kind": "direct",
                "impure": false,
                "size_observed": false,
                "observation": "pre-execution-intent",
            }),
        ),
    );

    // A *registered* observer covers nothing on its own. Coverage is
    // attributed from producers that actually emitted for the
    // computation, which is the conservative reading the engine uses:
    // an observer that never ran observed nothing.
    if with_observer {
        index += 1;
        append(
            &mut store,
            event_from(
                OBSERVER,
                index,
                EventKind::FsRead,
                serde_json::json!({
                    "path": "/tmp/subprocess-read.txt",
                    "size": 5,
                    "hash": "blake3:2222222222222222222222222222222222222222222222222222222222222222",
                    "read_kind": "direct",
                    "impure": false,
                }),
            ),
        );
    }

    store.sync().expect("sync");
    store_dir
}

fn report(store_dir: &PathBuf) -> serde_json::Value {
    let out = Command::new(binary())
        .arg("coverage")
        .arg("--store")
        .arg(store_dir)
        .arg("--json")
        .output()
        .expect("run freshdag coverage");
    assert!(
        out.status.success(),
        "coverage failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("report is JSON")
}

/// The headline number must count blind tool calls against coverage.
/// A `bash` call that happened to touch nothing still counts, because
/// the store cannot know it touched nothing — that unknowability is
/// precisely what is being measured.
#[test]
fn blind_tool_calls_count_against_observed_coverage() {
    let store = fixture("blind", false);
    let r = report(&store);

    assert_eq!(r["total_tool_calls"], 3);
    assert_eq!(r["blind_tool_calls"], 2, "two bash calls are blind");
    assert_eq!(r["tool_calls"]["bash"], 2);
    assert_eq!(r["tool_calls"]["builtin"], 1);
    assert_eq!(r["obligations"], 2, "each bash raises an obligation");
}

/// An unfingerprinted read is not a dependency, and must not be counted
/// as one — but it still implies a dependency exists, unproven.
#[test]
fn an_unfingerprinted_read_is_excluded_but_still_unproven() {
    let store = fixture("excluded", false);
    let r = report(&store);

    assert_eq!(
        r["dependencies"], 1,
        "only the fingerprinted read is an edge"
    );
    assert_eq!(r["excluded"]["no-fingerprint"], 1);
    assert_eq!(
        r["unproven"], 1,
        "a missing fingerprint still implies a dependency at that key"
    );
}

/// Without an Observer-role producer declaring fs coverage, bash
/// obligations cannot be discharged. Reporting otherwise would be the
/// coverage-deficit rule's whole purpose defeated.
#[test]
fn obligations_are_undischargeable_without_an_observer() {
    let r = report(&fixture("no-observer", false));
    assert_eq!(r["obligations_dischargeable"], false);
    assert_eq!(r["obligations"], 2);
}

/// An fs-covering Observer that actually contributed to the computation
/// discharges the obligation. Registration alone is not enough — the
/// engine attributes coverage from producers that emitted, so an
/// observer that never ran observed nothing.
#[test]
fn an_fs_covering_observer_that_ran_makes_obligations_dischargeable() {
    let r = report(&fixture("with-observer", true));
    assert_eq!(
        r["obligations_dischargeable"], true,
        "an Observer declaring fs.read/fs.write discharges the obligation"
    );
    assert!(
        r["registered_producers"]
            .as_array()
            .expect("array")
            .iter()
            .any(|p| p.as_str().is_some_and(|s| s.starts_with(OBSERVER))),
        "the observer is listed among registered producers"
    );
}

/// Five of the six Tier-1 metrics cannot be computed from a store
/// today. The report must say so by name rather than substitute a
/// computable number for an uncomputable one.
#[test]
fn uncomputable_metrics_are_named_not_approximated() {
    let store = fixture("metrics", false);
    let out = Command::new(binary())
        .arg("coverage")
        .arg("--store")
        .arg(&store)
        .output()
        .expect("run freshdag coverage");
    let text = String::from_utf8_lossy(&out.stdout);

    for metric in [
        "cache hit rate",
        "wall time saved",
        "$ saved",
        "undeclared-dep catch count",
        // Withdrawn after review: the store does not attribute edges to
        // producers, so the documented definition cannot be computed.
        "coverage silence rate",
    ] {
        let line = text
            .lines()
            .find(|l| l.trim_start().starts_with(metric))
            .unwrap_or_else(|| panic!("`{metric}` is missing from the report:\n{text}"));
        assert!(
            line.contains("not computable"),
            "`{metric}` must be named as not computable, got: {line}"
        );
    }
}

/// Only `builtin` tools can yield filesystem evidence. `mcp`, `skill`,
/// and a `tool.invoked` carrying no `tool_kind` at all produce none —
/// counting them as observable credits the adapter for evidence it never
/// produced, and biases the headline ratio optimistic.
#[test]
fn non_builtin_tool_kinds_all_count_as_blind() {
    let root = target_tmp().join("kinds");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");
    let store_dir = root.join(".freshdag");
    let mut store = Store::open(&store_dir).expect("open");
    store
        .register_producer(manifest(ADAPTER, ProducerRole::Adapter, &["tool.*"]))
        .expect("register");

    for (i, payload) in [
        serde_json::json!({"tool_kind": "mcp", "tool_name": "mcp__x__y"}),
        serde_json::json!({"tool_kind": "skill", "tool_name": "s"}),
        serde_json::json!({"tool_name": "mystery"}),
        serde_json::json!({"tool_kind": "builtin", "tool_name": "Read"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            store
                .append(&event(i as u128, EventKind::ToolInvoked, payload))
                .expect("append"),
            AppendOutcome::Appended
        );
    }
    store.sync().expect("sync");

    let r = report(&store_dir);
    assert_eq!(r["total_tool_calls"], 4);
    assert_eq!(
        r["blind_tool_calls"], 3,
        "only the builtin call can yield fs evidence; got {r}"
    );
}

/// Dischargeability is decided per computation, exactly as the engine
/// decides it. `OR`-ing a single flag across the store made one
/// computation's observer vouch for another's, so the report claimed
/// "dischargeable" while `check` reported `coverage-deficit` on the same
/// data — optimistic, and contradicting the tool it describes.
#[test]
fn dischargeable_requires_every_obligated_computation_to_be_covered() {
    let root = target_tmp().join("per-comp");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");
    let store_dir = root.join(".freshdag");
    let mut store = Store::open(&store_dir).expect("open");
    store
        .register_producer(manifest(
            ADAPTER,
            ProducerRole::Adapter,
            &["tool.*", "fs.read"],
        ))
        .expect("register adapter");
    store
        .register_producer(manifest(
            OBSERVER,
            ProducerRole::Observer,
            &["fs.read", "fs.write"],
        ))
        .expect("register observer");

    // comp:A raises an obligation and has NO observer contribution.
    let mut a = event(
        0,
        EventKind::ToolInvoked,
        serde_json::json!({"tool_kind": "bash", "tool_name": "bash"}),
    );
    a.computation_id = Some("comp:A".to_string());
    assert_eq!(store.append(&a).expect("append"), AppendOutcome::Appended);

    // comp:B raises one and the observer did contribute.
    let mut b = event(
        1,
        EventKind::ToolInvoked,
        serde_json::json!({"tool_kind": "bash", "tool_name": "bash"}),
    );
    b.computation_id = Some("comp:B".to_string());
    assert_eq!(store.append(&b).expect("append"), AppendOutcome::Appended);
    let mut obs = event_from(
        OBSERVER,
        2,
        EventKind::FsRead,
        serde_json::json!({
            "path": "/tmp/seen-by-observer.txt", "size": 1,
            "hash": "blake3:3333333333333333333333333333333333333333333333333333333333333333",
            "read_kind": "direct", "impure": false,
        }),
    );
    obs.computation_id = Some("comp:B".to_string());
    assert_eq!(store.append(&obs).expect("append"), AppendOutcome::Appended);
    store.sync().expect("sync");

    let r = report(&store_dir);
    assert_eq!(r["computations_with_obligations"], 2);
    assert_eq!(r["computations_with_dischargeable_obligations"], 1);
    assert_eq!(
        r["obligations_dischargeable"], false,
        "one covered computation must not vouch for an uncovered one"
    );
}

/// A report is not a verdict. Whatever it finds, it exits 0 — a low
/// number is a fact about the world, not a tool failure.
#[test]
fn coverage_exits_zero_however_bad_the_news() {
    let store = fixture("exit-code", false);
    let out = Command::new(binary())
        .arg("coverage")
        .arg("--store")
        .arg(&store)
        .output()
        .expect("run freshdag coverage");
    assert_eq!(out.status.code(), Some(0));
}

/// Pointing it at a directory that is not a store is a usage error,
/// and must not be mistaken for "this session had no coverage".
#[test]
fn a_missing_store_is_a_usage_error_not_a_zero_coverage_report() {
    let empty = target_tmp().join("not-a-store");
    std::fs::create_dir_all(&empty).expect("dir");
    let out = Command::new(binary())
        .arg("coverage")
        .arg("--store")
        .arg(&empty)
        .output()
        .expect("run freshdag coverage");
    assert!(!out.status.success());
    assert!(
        out.status.code().is_some_and(|c| c > 2),
        "a tool error, not a verdict; got {:?}",
        out.status.code()
    );
}
