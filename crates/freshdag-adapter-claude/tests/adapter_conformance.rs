//! Golden-file conformance harness for
//! `fixtures/adapter-conformance/claude/`.
//!
//! Each fixture directory contains:
//!
//! - `payload.json` — one raw Claude Code hook payload, exactly as it
//!   would arrive on stdin. It is read as **bytes**, not parsed, so a
//!   fixture may deliberately hold invalid JSON.
//! - `expected.jsonl` — the canonical IR event stream, one event per
//!   line, compared **byte for byte**.
//! - `README.md` — what invariant the fixture pins (optional).
//!
//! Determinism comes from injecting
//! [`freshdag_adapter_claude::determinism::FixedClock`] and
//! [`freshdag_adapter_claude::determinism::SeededIdGen`]; nothing in the
//! compile path reads the wall clock or mints a random UUID.
//!
//! Regenerate goldens with `FRESHDAG_BLESS=1 cargo test -p
//! freshdag-adapter-claude`. Review the diff — a golden that changes
//! without a deliberate behavior change is a regression.
//!
//! Adding a fixture requires ZERO changes to this file.

use std::fs;
use std::path::{Path, PathBuf};

use freshdag_adapter_claude::compile::Compiler;
use freshdag_adapter_claude::config::AdapterConfig;
use freshdag_adapter_claude::determinism::{FixedClock, SeededIdGen};
use freshdag_core::ir::IrEvent;

fn fixtures_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `.../crates/freshdag-adapter-claude`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
        .join("adapter-conformance")
        .join("claude")
}

fn discover(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = fs::read_dir(root).unwrap_or_else(|e| panic!("read {}: {e}", root.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() && path.join("payload.json").exists() {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn blessing() -> bool {
    std::env::var("FRESHDAG_BLESS").is_ok_and(|v| v == "1")
}

/// Compile a fixture's payload with the deterministic clock and id
/// generator, and render it as canonical JSONL.
fn render(payload: &str) -> String {
    let mut compiler = Compiler::new(
        AdapterConfig::new(),
        FixedClock::conformance(),
        SeededIdGen::conformance(),
    );
    let events = compiler.compile_str(payload);
    let mut out = String::new();
    for event in &events {
        out.push_str(&serde_json::to_string(event).expect("IR events serialize"));
        out.push('\n');
    }
    out
}

#[test]
fn golden_ir_streams_match() {
    let root = fixtures_root();
    let fixtures = discover(&root);
    assert!(
        fixtures.len() >= 3,
        "the adapter contract requires adversarial fixture coverage; found {} under {}",
        fixtures.len(),
        root.display()
    );

    let mut failures = Vec::new();
    for dir in &fixtures {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let payload = fs::read_to_string(dir.join("payload.json"))
            .unwrap_or_else(|e| panic!("{name}: read payload.json: {e}"));
        let actual = render(&payload);
        let expected_path = dir.join("expected.jsonl");

        if blessing() {
            fs::write(&expected_path, &actual)
                .unwrap_or_else(|e| panic!("{name}: write expected.jsonl: {e}"));
            continue;
        }

        let expected = fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| panic!("{name}: read expected.jsonl: {e} (bless to create)"));
        if expected != actual {
            failures.push(format!(
                "--- fixture `{name}` ---\nexpected:\n{expected}\nactual:\n{actual}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} conformance fixture(s) diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn goldens_are_reproducible_within_a_single_run() {
    // Determinism is the whole point: two compiles of the same bytes
    // must be byte-identical. Any ambient clock or random UUID in the
    // compile path breaks this.
    for dir in discover(&fixtures_root()) {
        let payload = fs::read_to_string(dir.join("payload.json")).unwrap();
        assert_eq!(
            render(&payload),
            render(&payload),
            "{} is not reproducible",
            dir.display()
        );
    }
}

#[test]
fn every_golden_event_is_a_well_formed_ir_event() {
    for dir in discover(&fixtures_root()) {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let expected = match fs::read_to_string(dir.join("expected.jsonl")) {
            Ok(s) => s,
            Err(_) if blessing() => continue,
            Err(e) => panic!("{name}: {e}"),
        };
        assert!(!expected.is_empty(), "{name}: no event was emitted at all");
        for (i, line) in expected.lines().enumerate() {
            let event: IrEvent =
                serde_json::from_str(line).unwrap_or_else(|e| panic!("{name} line {}: {e}", i + 1));
            assert_eq!(event.producer, freshdag_adapter_claude::PRODUCER);
            assert_eq!(
                event.event_id.get_version_num(),
                7,
                "{name} line {}: event_id must be a UUIDv7",
                i + 1
            );
        }
    }
}

#[test]
fn goldens_never_leak_claude_code_vocabulary_into_graph_bearing_events() {
    // Invariants #1/#2/#14 enforced on the wire, not just in the types.
    //
    // Exempt by design: `tool_input`/`tool_output` (declared opaque by
    // execution-ir.md) and `diagnostic` events (whose payload is
    // explicitly "producer-defined fields" and whose whole job is to
    // say what the adapter could not classify).
    let banned = ["hook_event_name", "transcript_path", "tool_use_id"];
    for dir in discover(&fixtures_root()) {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let Ok(expected) = fs::read_to_string(dir.join("expected.jsonl")) else {
            continue;
        };
        for line in expected.lines() {
            let mut event: serde_json::Value = serde_json::from_str(line).unwrap();
            if event["kind"] == "diagnostic" {
                continue;
            }
            if let Some(payload) = event["payload"].as_object_mut() {
                payload.remove("tool_input");
                payload.remove("tool_output");
            }
            let rendered = event.to_string();
            for word in banned {
                assert!(
                    !rendered.contains(word),
                    "{name}: `{word}` leaked: {rendered}"
                );
            }
        }
    }
}

#[test]
fn goldens_never_disclose_hook_payload_values_in_diagnostics() {
    // A `transcript_path` VALUE is a filesystem path into the user's
    // conversation history. Naming the key is debugging; printing the
    // value is disclosure.
    for dir in discover(&fixtures_root()) {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let Ok(expected) = fs::read_to_string(dir.join("expected.jsonl")) else {
            continue;
        };
        for line in expected.lines() {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            if event["kind"] != "diagnostic" {
                continue;
            }
            let rendered = event["payload"].to_string();
            assert!(
                !rendered.contains(".claude/projects"),
                "{name}: a transcript path value leaked into a diagnostic: {rendered}"
            );
        }
    }
}
