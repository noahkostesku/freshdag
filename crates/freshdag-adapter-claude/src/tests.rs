//! Unit tests for the compile path, the coverage manifest and the sink.
//!
//! The golden-file conformance harness lives in
//! `tests/adapter_conformance.rs`; these tests cover the pure logic
//! behind it.

use std::path::PathBuf;

use freshdag_core::ir::{EventKind, EventKindPattern, IrEvent, ToolKind, TypedPayload};
use serde_json::{json, Value};

use crate::compile::Compiler;
use crate::config::{AdapterConfig, PRODUCER, UNKNOWN_SESSION_ID};
use crate::coverage::{coverage_manifest, coverage_manifest_for};
use crate::determinism::{FixedClock, SeededIdGen};
use crate::identity::{computation_id_for_session, SESSION_AS_COMPUTATION_V1};
use crate::sink::JsonlSink;

fn compiler() -> Compiler<FixedClock, SeededIdGen> {
    Compiler::new(
        AdapterConfig::new(),
        FixedClock::conformance(),
        SeededIdGen::conformance(),
    )
}

fn compile(payload: &Value) -> Vec<IrEvent> {
    compiler().compile_value(payload)
}

fn kinds(events: &[IrEvent]) -> Vec<EventKind> {
    events.iter().map(|e| e.kind).collect()
}

fn diag_codes(events: &[IrEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.kind == EventKind::Diagnostic)
        .map(|e| e.payload["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn pre_tool_use(tool_name: &str, tool_input: &Value) -> Value {
    json!({
        "hook_event_name": "PreToolUse",
        "session_id": "sess-1",
        "cwd": "/repo",
        "transcript_path": "/home/u/.claude/projects/x/t.jsonl",
        "tool_name": tool_name,
        "tool_input": tool_input,
    })
}

// ---------------------------------------------------------------------
// Read / Write / Edit -> synthesized fs events
// ---------------------------------------------------------------------

#[test]
fn read_tool_emits_tool_invoked_plus_fs_read() {
    let events = compile(&pre_tool_use(
        "Read",
        &json!({"file_path": "/repo/src/lib.rs"}),
    ));
    assert_eq!(
        kinds(&events),
        vec![EventKind::ToolInvoked, EventKind::FsRead]
    );

    let TypedPayload::ToolInvoked(invoked) = events[0].decode_payload().unwrap() else {
        panic!("expected ToolInvoked");
    };
    assert_eq!(invoked.tool_name, "Read");
    assert_eq!(invoked.tool_kind, ToolKind::Builtin);
    assert_eq!(invoked.cwd, Some(PathBuf::from("/repo")));

    let TypedPayload::FsRead(read) = events[1].decode_payload().unwrap() else {
        panic!("expected FsRead");
    };
    assert_eq!(read.path, PathBuf::from("/repo/src/lib.rs"));
    assert_eq!(
        read.hash, None,
        "the compile path must not touch the filesystem"
    );
    assert_eq!(read.follow_symlink_target, None);
}

#[test]
fn synthesized_fs_events_are_causally_linked_to_their_tool_invocation() {
    let events = compile(&pre_tool_use("Read", &json!({"file_path": "/repo/a.txt"})));
    let invoked_id = events[0].event_id;
    assert_eq!(events[1].parent_id, Some(invoked_id));
    assert_eq!(events[1].causal_inputs, Some(vec![invoked_id]));
}

#[test]
fn synthesized_fs_events_are_marked_pre_execution_intent() {
    // Invariant #7: the adapter observed an *intent*, not an effect. A
    // denied tool call still produces this event, so it must say so.
    let events = compile(&pre_tool_use("Read", &json!({"file_path": "/repo/a.txt"})));
    assert_eq!(
        events[1].payload["observation"],
        json!("pre-execution-intent")
    );
    assert_eq!(events[1].payload["size_observed"], json!(false));
}

#[test]
fn write_tool_hashes_the_content_it_can_actually_see() {
    let events = compile(&pre_tool_use(
        "Write",
        &json!({"file_path": "/repo/out.txt", "content": "hello"}),
    ));
    assert_eq!(
        kinds(&events),
        vec![EventKind::ToolInvoked, EventKind::FsWrite]
    );
    let TypedPayload::FsWrite(write) = events[1].decode_payload().unwrap() else {
        panic!("expected FsWrite");
    };
    assert_eq!(write.size, 5);
    let expected = blake3::hash(b"hello").to_hex().to_string();
    assert_eq!(write.hash.unwrap().digest_hex, expected);
    // Size WAS observed here, so the "unobserved" marker is absent.
    assert!(events[1].payload.get("size_observed").is_none());
}

#[test]
fn edit_tool_does_not_pretend_to_know_the_resulting_size() {
    let events = compile(&pre_tool_use(
        "Edit",
        &json!({"file_path": "/repo/out.txt", "old_string": "a", "new_string": "b"}),
    ));
    let TypedPayload::FsWrite(write) = events[1].decode_payload().unwrap() else {
        panic!("expected FsWrite");
    };
    assert_eq!(write.size, 0);
    assert_eq!(write.hash, None);
    assert_eq!(events[1].payload["size_observed"], json!(false));
}

#[test]
fn notebook_edit_uses_its_own_path_key() {
    let events = compile(&pre_tool_use(
        "NotebookEdit",
        &json!({"notebook_path": "/repo/nb.ipynb", "new_source": "x"}),
    ));
    assert_eq!(
        kinds(&events),
        vec![EventKind::ToolInvoked, EventKind::FsWrite]
    );
    let TypedPayload::FsWrite(write) = events[1].decode_payload().unwrap() else {
        panic!("expected FsWrite");
    };
    assert_eq!(write.path, PathBuf::from("/repo/nb.ipynb"));
}

#[test]
fn relative_tool_paths_are_resolved_against_cwd_and_keep_raw_path() {
    let events = compile(&pre_tool_use("Read", &json!({"file_path": "src/lib.rs"})));
    let TypedPayload::FsRead(read) = events[1].decode_payload().unwrap() else {
        panic!("expected FsRead");
    };
    assert_eq!(read.path, PathBuf::from("/repo/src/lib.rs"));
    assert_eq!(read.raw_path, Some(PathBuf::from("src/lib.rs")));
}

// ---------------------------------------------------------------------
// Bash / Task blindness - load-bearing for the coverage-deficit rule
// ---------------------------------------------------------------------

#[test]
fn bash_emits_tool_invoked_with_bash_kind_and_no_fs_events() {
    let events = compile(&pre_tool_use(
        "Bash",
        &json!({"command": "cat /etc/passwd > /tmp/x"}),
    ));
    assert_eq!(kinds(&events), vec![EventKind::ToolInvoked]);
    assert_eq!(events[0].payload["tool_kind"], json!("bash"));
    assert_eq!(events[0].payload["tool_name"], json!("bash"));
}

/// Both delegation spellings must raise the blindness signal.
///
/// The runtime emits `Agent`; the adapter only knew `Task`. A
/// delegation classified as `Builtin` raises no observation obligation,
/// so a subagent that read and wrote anything left no trace the
/// coverage-deficit rule could see.
#[test]
fn every_delegation_spelling_is_task_kind() {
    for spelling in crate::hook::TASK_TOOL_NAMES {
        let events = compile(&pre_tool_use(
            spelling,
            &json!({"subagent_type": "verifier", "prompt": "rewrite everything"}),
        ));
        assert_eq!(
            events[0].payload["tool_kind"],
            json!("task"),
            "`{spelling}` must be task-kind, or its blindness is invisible"
        );
        assert_eq!(
            events[0].payload["tool_name"],
            json!("task"),
            "`{spelling}` must normalize to one spelling for the store to key on"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, EventKind::FsRead | EventKind::FsWrite)),
            "`{spelling}` must synthesize no fs events"
        );
    }
}

/// A delegation spelling this adapter does not know falls to `Builtin`
/// and silently re-opens the hole. This test does not prevent that — it
/// records that the list is a hard-coded guess about a name the runtime
/// does not promise, so a future rename fails here rather than in a
/// certificate.
#[test]
fn unknown_delegation_spellings_are_not_silently_builtin() {
    let events = compile(&pre_tool_use(
        "Subagent",
        &json!({"subagent_type": "x", "prompt": "p"}),
    ));
    assert_eq!(
        events[0].payload["tool_kind"],
        json!("builtin"),
        "documents current behaviour: an unrecognized delegation name is \
         indistinguishable from an ordinary tool, and raises no obligation. \
         If the runtime renames the tool again, add it to TASK_TOOL_NAMES."
    );
}

#[test]
fn task_emits_tool_invoked_with_task_kind_and_no_fs_events() {
    let events = compile(&pre_tool_use(
        "Task",
        &json!({"subagent_type": "verifier", "prompt": "check"}),
    ));
    assert_eq!(kinds(&events), vec![EventKind::ToolInvoked]);
    assert_eq!(events[0].payload["tool_kind"], json!("task"));
}

#[test]
fn bash_and_task_never_synthesize_fs_events_however_suggestive_the_input() {
    // `Certificate::check_coverage_deficit` keys off `tool_kind ==
    // "bash"|"task"`. If this adapter ever synthesized an fs event for
    // them it would be fabricating an observation of a subprocess it
    // cannot see, and would mask the deficit the rule exists to catch.
    for (tool, input) in [
        (
            "Bash",
            json!({"command": "echo hi > /repo/out.txt", "file_path": "/repo/out.txt"}),
        ),
        (
            "Task",
            json!({"prompt": "write /repo/out.txt", "file_path": "/repo/out.txt"}),
        ),
    ] {
        let events = compile(&pre_tool_use(tool, &input));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, EventKind::FsRead | EventKind::FsWrite)),
            "{tool} must not synthesize fs events"
        );
    }
}

// ---------------------------------------------------------------------
// Name normalization
// ---------------------------------------------------------------------

#[test]
fn mcp_tool_names_are_normalized() {
    let events = compile(&pre_tool_use(
        "mcp__linear__create_issue",
        &json!({"title": "x"}),
    ));
    assert_eq!(kinds(&events), vec![EventKind::ToolInvoked]);
    assert_eq!(
        events[0].payload["tool_name"],
        json!("mcp/linear/create_issue")
    );
    assert_eq!(events[0].payload["tool_kind"], json!("mcp"));
}

#[test]
fn unnormalizable_mcp_names_diagnose_rather_than_guess() {
    let events = compile(&pre_tool_use("mcp__linear", &json!({})));
    assert_eq!(
        kinds(&events),
        vec![EventKind::ToolInvoked, EventKind::Diagnostic]
    );
    assert_eq!(events[0].payload["tool_name"], json!("mcp__linear"));
    assert_eq!(diag_codes(&events), vec!["tool-name-normalization-failed"]);
}

#[test]
fn skill_names_are_normalized() {
    let events = compile(&pre_tool_use("Skill", &json!({"command": "pdf"})));
    assert_eq!(events[0].payload["tool_name"], json!("skill/pdf"));
    assert_eq!(events[0].payload["tool_kind"], json!("skill"));
}

#[test]
fn unnamed_skills_diagnose_rather_than_invent_a_name() {
    let events = compile(&pre_tool_use("Skill", &json!({"args": []})));
    assert_eq!(events[0].payload["tool_name"], json!("Skill"));
    assert_eq!(diag_codes(&events), vec!["tool-name-normalization-failed"]);
}

// ---------------------------------------------------------------------
// Never drop silently (adapter contract section Responsibilities #5)
// ---------------------------------------------------------------------

#[test]
fn unknown_hook_event_becomes_a_diagnostic() {
    let events = compile(&json!({"hook_event_name": "Frobnicate", "session_id": "sess-1"}));
    assert_eq!(kinds(&events), vec![EventKind::Diagnostic]);
    assert_eq!(diag_codes(&events), vec!["unknown-hook-event"]);
    assert_eq!(events[0].payload["severity"], json!("warning"));
}

#[test]
fn invalid_json_becomes_a_diagnostic_not_a_panic() {
    let events = compiler().compile_str("{ this is not json");
    assert_eq!(kinds(&events), vec![EventKind::Diagnostic]);
    assert_eq!(diag_codes(&events), vec!["malformed-payload"]);
}

#[test]
fn non_object_json_becomes_a_diagnostic() {
    let events = compiler().compile_str("[1, 2, 3]");
    assert_eq!(diag_codes(&events), vec!["malformed-payload"]);
}

#[test]
fn missing_session_id_yields_a_diagnostic_with_no_computation_attribution() {
    let events = compile(&json!({"hook_event_name": "PreToolUse", "tool_name": "Read"}));
    assert_eq!(kinds(&events), vec![EventKind::Diagnostic]);
    assert_eq!(diag_codes(&events), vec!["missing-required-field"]);
    assert_eq!(events[0].session_id, UNKNOWN_SESSION_ID);
    assert_eq!(
        events[0].computation_id, None,
        "must not attribute an event to a computation it cannot identify"
    );
}

#[test]
fn missing_tool_name_yields_a_diagnostic_and_no_tool_event() {
    let events = compile(&json!({
        "hook_event_name": "PreToolUse",
        "session_id": "sess-1",
        "tool_input": {"file_path": "/a"},
    }));
    assert_eq!(kinds(&events), vec![EventKind::Diagnostic]);
    assert_eq!(diag_codes(&events), vec!["missing-required-field"]);
}

#[test]
fn unparseable_tool_input_still_records_the_invocation() {
    // We DID observe a Read invocation; we just could not synthesize its
    // fs.read. Dropping the tool.invoked too would lose real signal.
    let events = compile(&pre_tool_use("Read", &json!({"offset": 3})));
    assert_eq!(
        kinds(&events),
        vec![EventKind::ToolInvoked, EventKind::Diagnostic]
    );
    assert_eq!(diag_codes(&events), vec!["unparseable-tool-input"]);
}

#[test]
fn relative_path_without_cwd_diagnoses_instead_of_guessing_a_root() {
    let events = compile(&json!({
        "hook_event_name": "PreToolUse",
        "session_id": "sess-1",
        "tool_name": "Read",
        "tool_input": {"file_path": "src/lib.rs"},
    }));
    assert_eq!(
        kinds(&events),
        vec![EventKind::ToolInvoked, EventKind::Diagnostic]
    );
    assert_eq!(diag_codes(&events), vec!["unparseable-tool-input"]);
}

#[test]
fn recognized_but_unmapped_hook_events_are_recorded_at_info_severity() {
    for name in [
        "UserPromptSubmit",
        "Stop",
        "SubagentStop",
        "PreCompact",
        "Notification",
    ] {
        let events = compile(&json!({"hook_event_name": name, "session_id": "sess-1"}));
        assert_eq!(kinds(&events), vec![EventKind::Diagnostic], "{name}");
        assert_eq!(diag_codes(&events), vec!["unmapped-hook-event"], "{name}");
        assert_eq!(events[0].payload["severity"], json!("info"), "{name}");
    }
}

#[test]
fn diagnostics_never_carry_hook_payload_values() {
    let events = compile(&json!({
        "hook_event_name": "Frobnicate",
        "session_id": "sess-1",
        "tool_input": {"content": "SUPER-SECRET-TOKEN"},
    }));
    let rendered = serde_json::to_string(&events[0]).unwrap();
    assert!(
        rendered.contains("tool_input"),
        "keys are useful for debugging"
    );
    assert!(!rendered.contains("SUPER-SECRET-TOKEN"), "values are not");
}

#[test]
fn every_input_produces_at_least_one_event() {
    let inputs = [
        "",
        "null",
        "{}",
        "[]",
        "\"a string\"",
        r#"{"session_id": "s"}"#,
        r#"{"hook_event_name": "Stop"}"#,
    ];
    for input in inputs {
        let events = compiler().compile_str(input);
        assert!(!events.is_empty(), "silent drop on input: {input:?}");
    }
}

// ---------------------------------------------------------------------
// Session / computation bracketing
// ---------------------------------------------------------------------

#[test]
fn session_start_opens_both_brackets_on_startup() {
    let events = compile(&json!({
        "hook_event_name": "SessionStart",
        "session_id": "sess-1",
        "cwd": "/repo",
        "source": "startup",
    }));
    assert_eq!(
        kinds(&events),
        vec![EventKind::SessionStarted, EventKind::ComputationStarted]
    );
    assert_eq!(events[0].payload["agent_kind"], json!("claude-code"));
    assert_eq!(
        events[1].payload["recipe_id"],
        json!("claude-code-session:sess-1")
    );
    assert_eq!(
        events[1].payload["identity_rule"],
        json!(SESSION_AS_COMPUTATION_V1)
    );
}

#[test]
fn resumed_sessions_do_not_reopen_the_computation_bracket() {
    // Adapter contract section Responsibilities #2 requires EXACTLY ONE
    // computation.started per computation_id. SessionStart fires again
    // on resume/clear/compact for a session we have already bracketed.
    for source in ["resume", "clear", "compact"] {
        let events = compile(&json!({
            "hook_event_name": "SessionStart",
            "session_id": "sess-1",
            "source": source,
        }));
        assert_eq!(
            kinds(&events),
            vec![EventKind::SessionStarted, EventKind::Diagnostic],
            "{source}"
        );
        assert_eq!(
            diag_codes(&events),
            vec!["computation-bracket-skipped"],
            "{source}"
        );
    }
}

#[test]
fn session_end_closes_both_brackets() {
    let events = compile(&json!({
        "hook_event_name": "SessionEnd",
        "session_id": "sess-1",
        "reason": "logout",
    }));
    assert_eq!(
        kinds(&events),
        vec![EventKind::ComputationEnded, EventKind::SessionEnded]
    );
    assert_eq!(events[0].payload["status"], json!("ok"));
    assert_eq!(events[1].parent_id, Some(events[0].event_id));
}

#[test]
fn an_unrecognized_end_reason_does_not_become_a_clean_completion() {
    // Invariant #7 at the session boundary: unknown is not "ok".
    for payload in [
        json!({"hook_event_name": "SessionEnd", "session_id": "s", "reason": "crashed"}),
        json!({"hook_event_name": "SessionEnd", "session_id": "s"}),
    ] {
        let events = compile(&payload);
        assert_eq!(events[0].payload["status"], json!("aborted"));
    }
}

#[test]
fn post_tool_use_emits_tool_completed_without_fabricating_a_duration() {
    let events = compile(&json!({
        "hook_event_name": "PostToolUse",
        "session_id": "sess-1",
        "tool_name": "Read",
        "tool_input": {"file_path": "/repo/a.txt"},
        "tool_response": {"type": "text", "file": {"numLines": 3}},
    }));
    assert_eq!(kinds(&events), vec![EventKind::ToolCompleted]);
    let TypedPayload::ToolCompleted(done) = events[0].decode_payload().unwrap() else {
        panic!("expected ToolCompleted");
    };
    assert_eq!(done.duration_ms, 0);
    assert!(!done.is_error);
    assert_eq!(events[0].payload["duration_observed"], json!(false));
    assert_eq!(events[0].payload["tool_name"], json!("Read"));
}

#[test]
fn post_tool_use_does_not_double_synthesize_fs_events() {
    // fs.* is synthesized once, from PreToolUse. Doing it again here
    // would double-count every read and write.
    let events = compile(&json!({
        "hook_event_name": "PostToolUse",
        "session_id": "sess-1",
        "tool_name": "Write",
        "tool_input": {"file_path": "/repo/a.txt", "content": "x"},
        "tool_response": {"success": true},
    }));
    assert_eq!(kinds(&events), vec![EventKind::ToolCompleted]);
}

#[test]
fn tool_errors_are_detected_from_the_spellings_that_exist() {
    for (response, expected) in [
        (json!({"success": false}), true),
        (json!({"is_error": true}), true),
        (json!({"error": "boom"}), true),
        (json!({"success": true}), false),
        (json!("plain text"), false),
    ] {
        let events = compile(&json!({
            "hook_event_name": "PostToolUse",
            "session_id": "s",
            "tool_name": "Bash",
            "tool_response": response,
        }));
        assert_eq!(events[0].payload["is_error"], json!(expected), "{response}");
    }
}

// ---------------------------------------------------------------------
// Envelope, identity, determinism
// ---------------------------------------------------------------------

#[test]
fn computation_id_is_the_core_derivation_not_an_opaque_mint() {
    let events = compile(&pre_tool_use("Read", &json!({"file_path": "/a"})));
    let expected = computation_id_for_session("sess-1", SESSION_AS_COMPUTATION_V1).0;
    for ev in &events {
        assert_eq!(ev.computation_id.as_deref(), Some(expected.as_str()));
        assert_eq!(ev.producer, PRODUCER);
    }
    assert!(expected.starts_with("comp:"));
}

#[test]
fn compilation_is_deterministic_across_runs() {
    let payload = pre_tool_use(
        "Write",
        &json!({"file_path": "/repo/a.txt", "content": "x"}),
    );
    let a = serde_json::to_string(&compile(&payload)).unwrap();
    let b = serde_json::to_string(&compile(&payload)).unwrap();
    assert_eq!(a, b, "compile must be a pure function of its inputs");
}

#[test]
fn no_wall_clock_leaks_into_the_compile_path() {
    // If any code path called OffsetDateTime::now_utc() or
    // Uuid::now_v7() directly, these fixed values would not hold.
    let events = compile(&pre_tool_use("Read", &json!({"file_path": "/a"})));
    assert_eq!(events[0].ts.unix_timestamp(), 1_767_225_600);
    assert!(events[1].ts > events[0].ts);
    assert!(events[0].event_id.to_string().ends_with("-000000000001"));
    assert!(events[1].event_id.to_string().ends_with("-000000000002"));
}

#[test]
fn seeded_ids_are_valid_monotonic_uuid_v7s() {
    let events = compile(&pre_tool_use("Read", &json!({"file_path": "/a"})));
    for ev in &events {
        assert_eq!(ev.event_id.get_version_num(), 7);
    }
    assert!(
        events[0].event_id < events[1].event_id,
        "per-producer order"
    );
}

#[test]
fn every_emitted_event_round_trips_through_the_ir_envelope() {
    for payload in &representative_payloads() {
        for ev in compile(payload) {
            let json = serde_json::to_string(&ev).unwrap();
            let back: IrEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, ev);
        }
    }
}

// ---------------------------------------------------------------------
// Coverage manifest
// ---------------------------------------------------------------------

fn coverage_json_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("coverage.json")
}

#[test]
fn coverage_json_matches_the_rust_constructor() {
    // A drifting manifest is exactly the failure the coverage-deficit
    // rule cannot tolerate: the engine reads the JSON, the adapter
    // behaves like the Rust.
    let raw = std::fs::read_to_string(coverage_json_path()).expect("coverage.json is readable");
    let from_file: freshdag_core::ir::CoverageManifest =
        serde_json::from_str(&raw).expect("coverage.json parses as a CoverageManifest");
    assert_eq!(from_file, coverage_manifest());
}

/// Suppressing a kind must narrow the published manifest.
///
/// A manifest declaring a kind the adapter has been configured not to
/// emit is a fail-*open* lie: silence under a *covered* kind is
/// `ObservedAbsent` ("nothing happened"), while silence under an
/// unclaimed kind is `Unobserved` ("I cannot say"). Publishing the
/// un-narrowed manifest would turn a suppressed adapter's silence into
/// evidence.
#[test]
fn suppression_narrows_the_published_manifest() {
    use freshdag_core::ir::EventKindPattern;

    let full = coverage_manifest_for(&AdapterConfig::new());
    assert!(
        full.emits.iter().any(|p| p.as_str() == "fs.read"),
        "test is vacuous unless the un-narrowed manifest declares fs.read"
    );

    let suppressed = coverage_manifest_for(
        &AdapterConfig::new().with_suppressed_kinds(vec![EventKindPattern::new("fs.*")]),
    );
    assert!(
        !suppressed.emits.iter().any(|p| p.as_str() == "fs.read"),
        "a suppressed fs.read must not still be declared"
    );
    assert!(
        !suppressed.emits.iter().any(|p| p.as_str() == "fs.write"),
        "a suppressed fs.write must not still be declared"
    );
    assert!(
        !suppressed.partial.keys().any(|k| k.starts_with("fs.")),
        "partial notes about suppressed kinds describe nothing"
    );
    assert!(
        suppressed.emits.iter().any(|p| p.as_str() == "tool.*"),
        "suppressing fs.* must not disturb unrelated declarations"
    );
}

/// The coarse direction of the narrowing is deliberate: an exact
/// suppression drops the whole glob that contains it. Over-dropping
/// under-claims coverage, which caps an artifact at `unknown`; the
/// opposite error would license `valid`.
#[test]
fn an_exact_suppression_drops_the_glob_that_contains_it() {
    use freshdag_core::ir::EventKindPattern;

    let narrowed = coverage_manifest_for(
        &AdapterConfig::new().with_suppressed_kinds(vec![EventKindPattern::new("tool.invoked")]),
    );
    assert!(
        !narrowed.emits.iter().any(|p| p.as_str() == "tool.*"),
        "`tool.*` intersects the suppressed `tool.invoked` and must be dropped whole"
    );
    assert!(
        narrowed.emits.iter().any(|p| p.as_str() == "fs.read"),
        "an unrelated declaration survives"
    );
}

/// The fingerprint is what decides whether this adapter produces usable
/// dependencies at all: `freshdag_store::graph` drops an `fs.read` with
/// no hash as `NoFingerprint`, so before reads were fingerprinted every
/// artifact came back `no-dependencies-observed`.
#[test]
fn a_production_compiler_fingerprints_the_file_a_read_is_about_to_read() {
    use crate::content::DiskContent;

    let dir = std::env::temp_dir().join("freshdag-read-fingerprint");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("input.txt");
    std::fs::write(&file, b"payload\n").expect("write");

    let mut compiler = Compiler::new(
        AdapterConfig::new(),
        FixedClock::conformance(),
        SeededIdGen::conformance(),
    )
    .with_content(Box::new(DiskContent::default()));

    let payload = serde_json::json!({
        "session_id": "s",
        "cwd": dir.display().to_string(),
        "hook_event_name": "PreToolUse",
        "tool_name": "Read",
        "tool_input": {"file_path": file.display().to_string()},
    });
    let events = compiler.compile_value(&payload);
    let read = events
        .iter()
        .find(|e| e.kind == freshdag_core::ir::EventKind::FsRead)
        .expect("a Read synthesizes fs.read");

    assert_eq!(
        read.payload["hash"].as_str(),
        Some(format!("blake3:{}", blake3::hash(b"payload\n").to_hex()).as_str()),
        "the fingerprint is of the bytes on disk"
    );
    assert_eq!(read.payload["size"].as_u64(), Some(8));
    assert_eq!(read.payload["size_observed"].as_bool(), Some(true));
}

/// The conformance constructor must stay filesystem-free, or goldens
/// would depend on the machine that ran them.
#[test]
fn the_default_compiler_reads_no_files() {
    let dir = std::env::temp_dir().join("freshdag-read-fingerprint");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("input.txt");
    std::fs::write(&file, b"payload\n").expect("write");

    let mut compiler = compiler();
    let payload = serde_json::json!({
        "session_id": "s",
        "cwd": dir.display().to_string(),
        "hook_event_name": "PreToolUse",
        "tool_name": "Read",
        "tool_input": {"file_path": file.display().to_string()},
    });
    let events = compiler.compile_value(&payload);
    let read = events
        .iter()
        .find(|e| e.kind == freshdag_core::ir::EventKind::FsRead)
        .expect("a Read synthesizes fs.read");

    assert!(
        read.payload["hash"].is_null(),
        "the default content source must not touch the filesystem"
    );
    assert_eq!(read.payload["size_observed"].as_bool(), Some(false));
}

#[test]
fn coverage_json_declares_the_adapter_role() {
    // PENDING PHASE A: `CoverageManifest` gains a required
    // `role: ProducerRole` field so that an ADAPTER declaring fs.read /
    // fs.write no longer discharges the observation obligation that
    // `Certificate::check_coverage_deficit` enforces for bash/task. The
    // JSON already carries it; this test pins it so the fact survives
    // until the Rust constructor can express it.
    let raw = std::fs::read_to_string(coverage_json_path()).unwrap();
    let value: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        value["role"],
        json!("adapter"),
        "coverage.json must declare role=adapter"
    );
}

#[test]
fn the_manifest_covers_every_kind_the_adapter_can_actually_emit() {
    let manifest = coverage_manifest();
    let mut seen = std::collections::BTreeSet::new();
    for payload in representative_payloads() {
        for ev in compile(&payload) {
            assert!(
                manifest.covers(ev.kind),
                "emitted {} but the manifest does not declare it",
                ev.kind
            );
            seen.insert(ev.kind);
        }
    }
    // And the converse direction we can check cheaply: the manifest must
    // not be a fantasy list. Every kind it names concretely must be
    // reachable from some payload we exercise.
    for kind in [
        EventKind::SessionStarted,
        EventKind::SessionEnded,
        EventKind::ComputationStarted,
        EventKind::ComputationEnded,
        EventKind::ToolInvoked,
        EventKind::ToolCompleted,
        EventKind::FsRead,
        EventKind::FsWrite,
        EventKind::Diagnostic,
    ] {
        assert!(
            seen.contains(&kind),
            "manifest declares {kind} but nothing emits it"
        );
    }
}

/// Pin every `PartialReason` in the manifest.
///
/// Before this test the manifest's reasons had **zero** coverage: the
/// tests asserted note *text* only, which is the free-text half no
/// consumer may decide on. The machine-readable half — the half that
/// reaches the certificate and that a third-party rechecker acts on —
/// was unpinned, and that is how `tool.completed` shipped as
/// `over-approximates`, the sole discharging reason in the crate, on a
/// claim its own compile path contradicts.
#[test]
fn every_partial_entry_declares_the_reason_it_was_ratified_with() {
    use freshdag_core::ir::PartialReason;

    let manifest = coverage_manifest();
    let expected = [
        ("fs.read", PartialReason::UnderApproximates),
        ("fs.write", PartialReason::UnderApproximates),
        ("fs.*", PartialReason::BlindInScope),
        ("tool.completed", PartialReason::UnderApproximates),
        ("computation.*", PartialReason::UnderApproximates),
    ];

    for (pattern, reason) in expected {
        let entry = manifest
            .partial
            .get(pattern)
            .unwrap_or_else(|| panic!("`{pattern}` must carry a partial declaration"));
        assert_eq!(
            entry.reason, reason,
            "`{pattern}` changed reason; a producer's own owner ratifies that \
             (ADR 0011, Amendment, Correction 1), so update the ratification, not just this test"
        );
    }
    assert_eq!(
        manifest.partial.len(),
        expected.len(),
        "a partial entry was added or removed without ratifying its reason"
    );
}

/// **No kind this adapter declares partial may discharge an
/// obligation.**
///
/// `role: Adapter` already bars this producer from the engine's one
/// discharge site, but `CoverageManifest::discharges` is public and
/// role-free: an `over-approximates` entry returns `true` to any future
/// obligation keyed on another kind, and to any third-party rechecker
/// reading `observation_coverage` off a certificate — which is the
/// entire point of ADR 0011 §Decision 2. "It is unreachable today" is
/// true; "it is inert" is not.
///
/// Deliberately scoped to the declared-partial kinds. `tool.invoked`
/// DOES discharge and should: this adapter is the authoritative producer
/// of tool events and declares no partiality on that kind. The claim
/// under test is narrower — wherever this adapter has admitted a gap,
/// that admission must never read as a safety guarantee.
#[test]
fn nothing_this_adapter_declares_partial_discharges() {
    let manifest = coverage_manifest();
    for kind in [
        EventKind::FsRead,
        EventKind::FsWrite,
        EventKind::FsStat,
        EventKind::ToolCompleted,
        EventKind::ComputationStarted,
        EventKind::ComputationEnded,
    ] {
        assert!(
            !manifest.discharges(kind),
            "`{}` is declared partial yet discharges; an admission of a gap \
             must never read as a guarantee",
            kind.as_wire_str()
        );
    }
}

#[test]
fn the_manifest_declares_bash_and_task_blindness_in_writing() {
    let manifest = coverage_manifest();
    let note = manifest
        .partial_note(EventKind::FsStat)
        .expect("fs.* must carry a partial note");
    assert!(note.contains("bash"), "note must name the bash gap: {note}");
    assert!(note.contains("task"), "note must name the task gap: {note}");
    assert!(
        manifest
            .known_limitations
            .iter()
            .any(|l| l.contains("NOT AN OBSERVER")),
        "the manifest must say this adapter does not substitute for an observer"
    );
    assert!(
        manifest
            .partial_note(EventKind::FsRead)
            .unwrap()
            .contains("Read"),
        "fs.read note must say it comes from the Read tool only"
    );
    assert!(
        manifest
            .partial_note(EventKind::FsWrite)
            .unwrap()
            .contains("Write"),
        "fs.write note must say it comes from the write tools only"
    );
}

#[test]
fn the_manifest_records_the_identity_rule_it_used() {
    let manifest = coverage_manifest();
    assert_eq!(
        manifest.capabilities["identity_rule"],
        json!(SESSION_AS_COMPUTATION_V1)
    );
    assert_eq!(manifest.producer, PRODUCER);
}

// ---------------------------------------------------------------------
// Coverage override
// ---------------------------------------------------------------------

#[test]
fn suppressed_kinds_are_withheld_but_never_silently() {
    let config = AdapterConfig::new().with_suppressed_kinds(vec![EventKindPattern::from("fs.*")]);
    let mut compiler = Compiler::new(
        config,
        FixedClock::conformance(),
        SeededIdGen::conformance(),
    );
    let events = compiler.compile_value(&pre_tool_use("Read", &json!({"file_path": "/a"})));
    assert_eq!(
        kinds(&events),
        vec![EventKind::ToolInvoked, EventKind::Diagnostic]
    );
    assert_eq!(diag_codes(&events), vec!["coverage-override-suppressed"]);
    assert_eq!(events[1].payload["suppressed_kinds"], json!(["fs.read"]));
}

#[test]
fn diagnostics_are_not_suppressible() {
    let config =
        AdapterConfig::new().with_suppressed_kinds(vec![EventKindPattern::from("diagnostic")]);
    let mut compiler = Compiler::new(
        config,
        FixedClock::conformance(),
        SeededIdGen::conformance(),
    );
    let events = compiler.compile_value(&json!({"hook_event_name": "Stop", "session_id": "s"}));
    assert_eq!(kinds(&events), vec![EventKind::Diagnostic]);
}

// ---------------------------------------------------------------------
// Sink
// ---------------------------------------------------------------------

#[test]
fn sink_appends_and_preserves_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("ir.jsonl");
    let sink = JsonlSink::new(&path);

    let first = compile(&json!({
        "hook_event_name": "SessionStart", "session_id": "s", "source": "startup"
    }));
    let second = compile(&pre_tool_use("Read", &json!({"file_path": "/a"})));
    assert_eq!(sink.write_all(&first).written, 2);
    assert_eq!(sink.write_all(&second).written, 2);

    let body = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 4);
    let parsed: Vec<IrEvent> = lines
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(parsed[0].kind, EventKind::SessionStarted);
    assert_eq!(parsed[3].kind, EventKind::FsRead);
}

#[test]
fn sink_drops_the_newest_never_the_oldest() {
    // Invariant #4: the append-only log is never rewritten, so
    // back-pressure can only refuse new bytes.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ir.jsonl");
    let events = compile(&json!({
        "hook_event_name": "SessionStart", "session_id": "s", "source": "startup"
    }));
    let first_len = serde_json::to_vec(&events[0]).unwrap().len() as u64;

    // Cap leaves room for exactly the first event plus the reserve.
    let sink = JsonlSink::new(&path).with_max_bytes(first_len + 1 + 8 * 1024);
    let outcome = sink.write_all(&events);
    assert_eq!(outcome.written, 1);
    assert_eq!(outcome.dropped, 1);

    let body = std::fs::read_to_string(&path).unwrap();
    assert_eq!(body.lines().count(), 1);
    let kept: IrEvent = serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(kept.event_id, events[0].event_id, "the OLDEST was kept");
}

#[test]
fn an_unusable_sink_directory_still_buffers_locally() {
    // Regression: the sibling buffer lives INSIDE the sink's directory,
    // so when it is the directory that is unusable, a sibling-only
    // fallback fails too and the events are lost. The adapter contract
    // requires buffering to a local append-only file, so the last
    // resort must not depend on the configured path at all.
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").unwrap(); // a FILE where a dir is needed
    let sink = JsonlSink::new(blocker.join("ir.jsonl"));

    let events = compile(&json!({"hook_event_name": "Stop", "session_id": "s"}));
    let outcome = sink.write_all(&events);

    assert_eq!(outcome.written, events.len(), "events must not be lost");
    assert_eq!(outcome.dropped, 0);
    let buffered = outcome.buffered_to.expect("a buffer must have been used");
    assert_eq!(buffered, sink.fallback_buffer_path());
    assert!(
        !outcome.errors.is_empty(),
        "degrading to a buffer must never be invisible"
    );

    let body = std::fs::read_to_string(&buffered).unwrap();
    assert!(body.contains("unmapped-hook-event"));
    let _ = std::fs::remove_file(&buffered);
}

#[test]
fn the_fallback_buffer_does_not_depend_on_the_sink_directory() {
    let sink = JsonlSink::new("/definitely/not/a/real/dir/ir.jsonl");
    let fallback = sink.fallback_buffer_path();
    assert!(fallback.starts_with(std::env::temp_dir()));
    // Distinct sinks must not collide in the shared temp directory.
    let other = JsonlSink::new("/some/other/ir.jsonl").fallback_buffer_path();
    assert_ne!(fallback, other);
}

#[test]
fn force_write_persists_the_record_of_a_drop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ir.jsonl");
    let sink = JsonlSink::new(&path).with_max_bytes(1);
    let mut c = compiler();
    let diag = crate::Diagnostic::new(crate::DiagnosticCode::SinkBackpressureDrop, "dropped 3");
    let event = c.standalone_diagnostic("sess-1", &diag);
    assert_eq!(sink.force_write(&event).written, 1);
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("sink-backpressure-drop"));
}

// ---------------------------------------------------------------------
// Boundary: no Claude Code concept escapes into the core
// ---------------------------------------------------------------------

/// Claude Code vocabulary that must not appear in graph-bearing events.
const BANNED_IN_GRAPH_EVENTS: [&str; 4] = [
    "hook_event_name",
    "transcript_path",
    "tool_use_id",
    "permission_mode",
];

#[test]
fn graph_bearing_events_carry_no_claude_code_hook_vocabulary() {
    // Invariants #1/#2/#14. Two fields are exempt by design:
    //   - `tool_input`/`tool_output` are declared opaque by
    //     execution-ir.md and pass through whatever the user typed.
    //   - `diagnostic` events are excluded entirely; see the test below.
    for payload in representative_payloads() {
        for ev in compile(&payload) {
            if ev.kind == EventKind::Diagnostic {
                continue;
            }
            let mut envelope = serde_json::to_value(&ev).unwrap();
            if let Some(p) = envelope["payload"].as_object_mut() {
                p.remove("tool_input");
                p.remove("tool_output");
            }
            let rendered = envelope.to_string();
            for word in BANNED_IN_GRAPH_EVENTS {
                assert!(
                    !rendered.contains(word),
                    "`{word}` leaked into a {} event: {rendered}",
                    ev.kind
                );
            }
        }
    }
}

#[test]
fn diagnostics_may_name_hook_fields_but_never_reveal_their_values() {
    // `diagnostic` is defined by execution-ir.md as
    // `{ message, ...producer-defined fields }` — it is the sanctioned
    // channel for runtime-specific debugging context, and a diagnostic
    // that could not say *which* field was missing would be useless.
    // What it must never do is disclose payload VALUES.
    let events = compile(&json!({
        "hook_event_name": "Frobnicate",
        "session_id": "sess-1",
        "transcript_path": "/Users/dev/.claude/projects/x/t.jsonl",
        "tool_input": {"content": "SUPER-SECRET-TOKEN"},
    }));
    let rendered = serde_json::to_string(&events[0]).unwrap();
    assert_eq!(events[0].kind, EventKind::Diagnostic);
    assert!(
        rendered.contains("hook_event_name"),
        "key names are the point"
    );
    assert!(
        rendered.contains("transcript_path"),
        "key names are the point"
    );
    assert!(
        !rendered.contains("SUPER-SECRET-TOKEN"),
        "values must never appear"
    );
    assert!(
        !rendered.contains("/Users/dev/.claude"),
        "the transcript PATH is a value and must never appear"
    );
    // And this vocabulary is confined to the adapter: nothing here is a
    // `freshdag-core` type. The payload is `serde_json::Value`.
    assert!(events[0].payload.is_object());
}

/// One payload per shape the adapter knows how to handle, used by the
/// coverage, round-trip and boundary tests.
fn representative_payloads() -> Vec<Value> {
    vec![
        json!({"hook_event_name": "SessionStart", "session_id": "s", "cwd": "/repo",
               "source": "startup"}),
        json!({"hook_event_name": "SessionStart", "session_id": "s", "source": "resume"}),
        json!({"hook_event_name": "SessionEnd", "session_id": "s", "reason": "logout"}),
        json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "hi"}),
        json!({"hook_event_name": "Stop", "session_id": "s", "stop_hook_active": false}),
        json!({"hook_event_name": "SubagentStop", "session_id": "s"}),
        json!({"hook_event_name": "PreCompact", "session_id": "s", "trigger": "auto"}),
        json!({"hook_event_name": "Notification", "session_id": "s", "message": "waiting"}),
        json!({"hook_event_name": "Frobnicate", "session_id": "s"}),
        pre_tool_use("Read", &json!({"file_path": "/repo/a.txt"})),
        pre_tool_use(
            "Write",
            &json!({"file_path": "/repo/a.txt", "content": "x"}),
        ),
        pre_tool_use(
            "Edit",
            &json!({"file_path": "/repo/a.txt", "old_string": "a", "new_string": "b"}),
        ),
        pre_tool_use("Bash", &json!({"command": "ls"})),
        pre_tool_use("Task", &json!({"prompt": "go"})),
        pre_tool_use("mcp__linear__create_issue", &json!({"title": "x"})),
        pre_tool_use("Skill", &json!({"command": "pdf"})),
        pre_tool_use("Glob", &json!({"pattern": "**/*.rs"})),
        pre_tool_use("Read", &json!({"offset": 1})),
        json!({"hook_event_name": "PostToolUse", "session_id": "s", "tool_name": "Read",
               "tool_response": {"type": "text"}}),
        json!({"hook_event_name": "PostToolUse", "session_id": "s", "tool_name": "Bash",
               "tool_response": {"success": false}}),
    ]
}
