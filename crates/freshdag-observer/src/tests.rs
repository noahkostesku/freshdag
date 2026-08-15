//! Tests for the observer trait, fsatrace parser, stub observer, and
//! scripted test double.

use freshdag_core::ir::{EventKind, IrEvent, TypedPayload};

use crate::linux::{parse_fsatrace_lines, FsatraceObserver};
use crate::observer::{CommandInvocation, Observer, ObserverError};
use crate::replay::ScriptedObserver;
use crate::stub::StubObserver;

// --------------------------------------------------------------------
// fsatrace line parser
// --------------------------------------------------------------------

#[test]
fn parses_read_and_write_lines() {
    let trace = "\
r|/abs/path/ICP.md
w|/abs/path/brief.md
m|/abs/path/appended.md
t|/abs/path/touched.md
d|/abs/path/deleted.md
q|/abs/path/statted.md
";
    let events = parse_fsatrace_lines(trace, "sess-abc", "0.1.0");
    // Only r/w/m/t currently emit IR events; d/q are silently skipped
    // (documented gap; visible via the coverage manifest).
    assert_eq!(events.len(), 4);

    assert_eq!(events[0].kind, EventKind::FsRead);
    match events[0].decode_payload().unwrap() {
        TypedPayload::FsRead(r) => assert_eq!(r.path.to_str().unwrap(), "/abs/path/ICP.md"),
        other => panic!("expected FsRead, got {other:?}"),
    }
    assert_eq!(events[1].kind, EventKind::FsWrite);
    assert_eq!(events[2].kind, EventKind::FsWrite);
    assert_eq!(events[3].kind, EventKind::FsWrite);

    // Every event carries the session and producer identity.
    for e in &events {
        assert_eq!(e.session_id, "sess-abc");
        assert_eq!(e.producer, "freshdag-observer-fsatrace");
        assert_eq!(e.producer_version, "0.1.0");
    }
}

#[test]
fn skips_malformed_lines() {
    let trace = "\
not-a-fsatrace-line
r|
r|/abs/path/ok.md
";
    let events = parse_fsatrace_lines(trace, "s", "v");
    // The first line has no `|`; the second has an empty path; only
    // the third parses.
    assert_eq!(events.len(), 1);
}

#[test]
fn empty_trace_yields_no_events() {
    let events = parse_fsatrace_lines("", "s", "v");
    assert!(events.is_empty());
}

// --------------------------------------------------------------------
// FsatraceObserver — platform gating
// --------------------------------------------------------------------

#[test]
fn fsatrace_observer_discover_fails_off_linux() {
    if cfg!(target_os = "linux") {
        // On Linux discover may succeed or fail depending on fsatrace
        // presence; skip in that case (documented in observer contract).
        return;
    }
    let err = FsatraceObserver::discover("sess").unwrap_err();
    match err {
        ObserverError::NotSupportedOnPlatform(msg) => {
            assert!(msg.contains("Linux-only"), "unexpected message: {msg}");
        }
        other => panic!("expected NotSupportedOnPlatform, got {other:?}"),
    }
}

#[test]
fn fsatrace_observer_observe_fails_off_linux() {
    if cfg!(target_os = "linux") {
        return;
    }
    let observer = FsatraceObserver::with_binary("/nonexistent/fsatrace", "sess");
    let invocation = CommandInvocation::new("/bin/true", std::env::temp_dir());
    let err = observer.observe(&invocation).unwrap_err();
    assert!(matches!(err, ObserverError::NotSupportedOnPlatform(_)));
}

#[test]
fn fsatrace_coverage_manifest_declares_platforms_and_limits() {
    let observer = FsatraceObserver::with_binary("/nonexistent/fsatrace", "sess");
    let cov = observer.coverage();
    assert_eq!(cov.producer, "freshdag-observer-fsatrace");
    assert!(cov.platforms.iter().any(|p| p.contains("linux")));
    assert!(cov.covers(EventKind::FsRead));
    assert!(cov.covers(EventKind::FsWrite));
    // Explicitly does NOT cover renames or stat in W6.1.
    assert!(!cov.covers(EventKind::FsRename));
    assert!(!cov.covers(EventKind::FsStat));
    // Known limitations are surfaced honestly.
    assert!(!cov.known_limitations.is_empty());
}

// --------------------------------------------------------------------
// StubObserver — coverage manifest is the load-bearing bit
// --------------------------------------------------------------------

#[test]
fn stub_observer_declares_zero_fs_coverage() {
    let obs = StubObserver::new();
    let cov = obs.coverage();
    assert_eq!(cov.producer, "freshdag-observer-stub");
    // The whole point: the manifest is honest that we cover nothing.
    assert!(!cov.covers(EventKind::FsRead));
    assert!(!cov.covers(EventKind::FsWrite));
    assert!(!cov.covers(EventKind::ProcSpawn));
    // And known limitations name the coverage-deficit implication.
    assert!(cov
        .known_limitations
        .iter()
        .any(|l| l.contains("coverage-deficit") || l.contains("unknown")));
}

#[test]
fn stub_observer_runs_subprocess_without_events() {
    let obs = StubObserver::new();
    // Use `true` (or `cmd /c exit` on Windows if we ever ship there).
    let program = if cfg!(target_os = "windows") {
        std::path::PathBuf::from("cmd")
    } else {
        std::path::PathBuf::from("/usr/bin/true")
    };
    let cwd = std::env::temp_dir();
    let inv = CommandInvocation::new(program, cwd);
    let run = obs.observe(&inv).unwrap();
    assert_eq!(run.events, Vec::<IrEvent>::new());
    // exit code may be None on very old kernels; on our target it's 0.
    if let Some(code) = run.exit_code {
        assert_eq!(code, 0);
    }
}

// --------------------------------------------------------------------
// ScriptedObserver — deterministic double for cross-platform tests
// --------------------------------------------------------------------

#[test]
fn scripted_observer_returns_authored_events() {
    let events = vec![freshdag_core::ir::IrEvent {
        event_id: uuid::Uuid::new_v4(),
        producer: "test".to_string(),
        producer_version: "0".to_string(),
        session_id: "s".to_string(),
        computation_id: None,
        parent_id: None,
        causal_inputs: None,
        ts: time::OffsetDateTime::now_utc(),
        kind: EventKind::FsRead,
        payload: serde_json::json!({ "path": "/x", "size": 0 }),
    }];
    let cov = ScriptedObserver::empty_coverage("test");
    let obs = ScriptedObserver::new(events.clone(), cov).with_exit_code(Some(42));
    let run = obs
        .observe(&CommandInvocation::new("/bin/true", std::env::temp_dir()))
        .unwrap();
    assert_eq!(run.events, events);
    assert_eq!(run.exit_code, Some(42));
}
