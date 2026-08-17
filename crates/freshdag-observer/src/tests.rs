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
m|/abs/path/single-path-move.md
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

/// A move line carries two paths (`m|<destination>|<source>`). The
/// parser must never concatenate them into one.
///
/// Regression test. The previous parser split on the first `|` only and
/// emitted `FsWrite { path: "/abs/path/brief.md|/abs/path/brief.tmp" }`
/// — a write at a path that cannot exist, while the real rename target
/// got nothing. That is a fabricated observation, and an observer that
/// invents events is worse than one that misses them: it is the
/// direction invariant #7 forbids.
#[test]
fn a_two_path_move_never_emits_a_concatenated_path() {
    let trace = "\
w|/abs/path/brief.tmp
m|/abs/path/brief.md|/abs/path/brief.tmp
";
    let events = parse_fsatrace_lines(trace, "s", "v");

    // The temp-path write survives; the move emits nothing, because the
    // synthetic write at the rename target (observer-contract §Required
    // Behavior #3) is not implemented and the manifest declares that.
    assert_eq!(events.len(), 1, "the move line must not emit an event");
    match events[0].decode_payload().unwrap() {
        TypedPayload::FsWrite(w) => {
            assert_eq!(w.path.to_str().unwrap(), "/abs/path/brief.tmp");
        }
        other => panic!("expected FsWrite, got {other:?}"),
    }

    // The specific defect: no emitted path may contain a separator.
    for e in &events {
        if let TypedPayload::FsWrite(w) = e.decode_payload().unwrap() {
            assert!(
                !w.path.to_str().unwrap().contains('|'),
                "emitted a concatenated path: {:?}",
                w.path
            );
        }
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
    // The manifest always names the platforms the backend supports…
    assert!(cov.platforms.iter().any(|p| p.contains("linux")));
    // …but `emits` describes what it delivers *here*, because that is
    // the field consumers compute coverage deficits from.
    if cfg!(target_os = "linux") {
        assert!(cov.covers(EventKind::FsRead));
        assert!(cov.covers(EventKind::FsWrite));
        // Both fs kinds are genuinely partial — declaring them without
        // a partiality note would overclaim (invariant #7 at the
        // source).
        assert!(cov.partial_note(EventKind::FsRead).is_some());
        assert!(cov.partial_note(EventKind::FsWrite).is_some());
    } else {
        // observe() always errors off-Linux, so claiming fs.* here
        // would be fabricated coverage.
        assert!(cov.emits.is_empty());
        assert!(!cov.covers(EventKind::FsRead));
        assert!(!cov.covers(EventKind::FsWrite));
    }
    // Explicitly does NOT cover renames or stat in W6.1, on any
    // platform.
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
    let cov = ScriptedObserver::full_fs_coverage("test");
    let obs = ScriptedObserver::new(events.clone(), cov).with_exit_code(Some(42));
    let run = obs
        .observe(&CommandInvocation::new("/bin/true", std::env::temp_dir()))
        .unwrap();
    assert_eq!(run.events, events);
    assert_eq!(run.exit_code, Some(42));
}

#[test]
fn scripted_coverage_constructors_are_not_confusable() {
    // Regression guard: `zero_coverage` used to be named
    // `empty_coverage` while returning blanket `fs.*` coverage. A test
    // reaching for "this producer claims nothing" silently got "this
    // producer claims it sees every filesystem event," which makes any
    // coverage-deficit assertion pass vacuously.
    let zero = ScriptedObserver::zero_coverage("p");
    assert!(zero.emits.is_empty());
    assert!(!zero.covers(EventKind::FsRead));
    assert!(!zero.covers(EventKind::FsWrite));

    let full = ScriptedObserver::full_fs_coverage("p");
    assert!(full.covers(EventKind::FsRead));
    assert!(full.covers(EventKind::FsWrite));
    assert!(full.covers(EventKind::FsRename));
    // `fs.*` is not a universal wildcard.
    assert!(!full.covers(EventKind::ToolInvoked));
    assert!(!full.covers(EventKind::ProcSpawn));
}

// --------------------------------------------------------------------
// Coverage-deficit smoke tests (W6.2)
//
// certificate-contract §Coverage-Deficit is the machine-checked form of
// invariant #7: a `bash`/`task` invocation with no producer declaring
// fs.* coverage MUST NOT certify as `valid`. Wave 1 exercised this with
// synthetic fixtures; these tests drive it from a real `Observer`
// implementation's event stream and its own published
// `CoverageManifest`, converted via the canonical
// `impl From<&CoverageManifest> for CoverageEntry`.
//
// Every test below is deterministic and platform-independent: the
// scripted observer never spawns a process and never touches the
// filesystem.
// --------------------------------------------------------------------

mod coverage_deficit {
    use std::path::PathBuf;

    use freshdag_core::artifact::{Artifact, ArtifactId};
    use freshdag_core::certificate::{
        Certificate, CoverageEntry, InvariantError, ProducedBy, Status, CERTIFICATE_SCHEMA_V0_1,
    };
    use freshdag_core::computation::ComputationId;
    use freshdag_core::dependency::ValidityStatus;
    use freshdag_core::ir::{
        CoverageManifest, EventKind, EventKindPattern, Hash, HashAlgo, IrEvent, ProducerRole,
        ToolInvoked, ToolKind,
    };

    use crate::observer::{CommandInvocation, Observer};
    use crate::replay::ScriptedObserver;
    use crate::stub::StubObserver;

    const OBSERVER_PRODUCER: &str = "freshdag-observer-scripted";
    const ADAPTER_PRODUCER: &str = "freshdag-adapter-claude";

    // ---------------- fixture helpers (deterministic) ----------------

    fn ts() -> time::OffsetDateTime {
        time::OffsetDateTime::UNIX_EPOCH
    }

    fn a_hash() -> Hash {
        Hash {
            algo: HashAlgo::Blake3,
            digest_hex: "a".repeat(64),
        }
    }

    fn ir_event(producer: &str, kind: EventKind, payload: serde_json::Value) -> IrEvent {
        IrEvent {
            event_id: uuid::Uuid::new_v4(),
            producer: producer.to_string(),
            producer_version: "0.1.0".to_string(),
            session_id: "sess-w62".to_string(),
            computation_id: Some("comp:w62".to_string()),
            parent_id: None,
            causal_inputs: None,
            ts: ts(),
            kind,
            payload,
        }
    }

    /// A `tool.invoked` event built from the *typed* core payload, so
    /// the wire key (`tool_kind`) and its values (`"bash"`, `"task"`)
    /// come from `freshdag-core`, not from a hand-written string that
    /// could drift away from what `check_coverage_deficit` reads.
    fn tool_invoked(producer: &str, tool_kind: ToolKind) -> IrEvent {
        let payload = serde_json::to_value(ToolInvoked {
            tool_name: match tool_kind {
                ToolKind::Bash => "Bash",
                ToolKind::Task => "Task",
                _ => "Other",
            }
            .to_string(),
            tool_kind,
            tool_input: serde_json::json!({ "command": "python3 build.py > out.txt" }),
            cwd: Some(PathBuf::from("/repo")),
        })
        .expect("ToolInvoked serializes");
        ir_event(producer, EventKind::ToolInvoked, payload)
    }

    /// The coverage an adapter publishes for the Claude Code runtime.
    /// Mirrors adapter-contract §Required Behavior #4's example: it
    /// sees `tool.*` and synthesizes `fs.read`/`fs.write` from the
    /// Read/Write/Edit tools, but filesystem effects *inside* a Bash
    /// subprocess are observer territory.
    fn adapter_manifest(emits: &[&str]) -> CoverageManifest {
        CoverageManifest {
            role: ProducerRole::Adapter,
            producer: ADAPTER_PRODUCER.to_string(),
            version: "0.1.0".to_string(),
            platforms: vec![],
            emits: emits.iter().map(|s| EventKindPattern::from(*s)).collect(),
            partial: std::collections::BTreeMap::new(),
            capabilities: std::collections::BTreeMap::new(),
            known_limitations: vec![],
        }
    }

    /// An observer-role manifest, for pinning rules that turn on
    /// fidelity rather than vantage point.
    fn observer_manifest(emits: &[&str]) -> CoverageManifest {
        CoverageManifest {
            role: ProducerRole::Observer,
            producer: "freshdag-observer-example".to_string(),
            version: "0.1.0".to_string(),
            platforms: vec![],
            emits: emits.iter().map(|s| EventKindPattern::from(*s)).collect(),
            partial: std::collections::BTreeMap::new(),
            capabilities: std::collections::BTreeMap::new(),
            known_limitations: vec![],
        }
    }

    /// A `Valid` certificate carrying the given coverage entries.
    ///
    /// `status.reasons` is empty because `Valid` is the only status
    /// that permits it (invariant #6) — so no `ValidityReason` is
    /// constructed anywhere in this module.
    fn valid_cert(observation_coverage: Vec<CoverageEntry>) -> Certificate {
        Certificate {
            cert_id: a_hash(),
            schema: CERTIFICATE_SCHEMA_V0_1.to_string(),
            artifact: Artifact {
                id: ArtifactId::from_hash(&a_hash()),
                path: Some("out.txt".to_string()),
                kind: "text/plain".to_string(),
                content_hash: a_hash(),
                size: 12,
            },
            produced_by: ProducedBy {
                computation_id: ComputationId::derive("build", "build.py", "v1"),
                recipe: Some("build".to_string()),
                recipe_hash: Some(a_hash()),
                adapter: "freshdag-adapter-claude/0.1.0".to_string(),
                started: ts(),
                ended: ts(),
            },
            depends_on: vec![],
            comparator: None,
            status: Status {
                value: ValidityStatus::Valid,
                checked: ts(),
                reasons: vec![],
            },
            observation_coverage,
        }
    }

    fn observe(obs: &dyn Observer) -> (Vec<IrEvent>, CoverageManifest) {
        let run = obs
            .observe(&CommandInvocation::new("/bin/true", std::env::temp_dir()))
            .expect("scripted observation never fails");
        (run.events, obs.coverage())
    }

    // ---------------- the negative case (the deliverable) ------------

    /// `emits: []` + a bash `tool.invoked` ⇒ the certificate cannot be
    /// `valid`. Note that `check_invariants` passes: the coverage
    /// deficit is the *only* thing standing between this certificate
    /// and a fabricated `valid`.
    #[test]
    fn zero_coverage_observer_cannot_certify_bash_as_valid() {
        let observer = ScriptedObserver::new(
            vec![tool_invoked(OBSERVER_PRODUCER, ToolKind::Bash)],
            ScriptedObserver::zero_coverage(OBSERVER_PRODUCER),
        );
        let (events, manifest) = observe(&observer);
        assert!(
            manifest.emits.is_empty(),
            "precondition: the producer must genuinely declare nothing"
        );

        let entry = CoverageEntry::from(&manifest);
        assert!(
            entry.emits.is_empty(),
            "From<&CoverageManifest> keeps emits"
        );
        assert!(!entry.covers(EventKind::FsRead));

        let cert = valid_cert(vec![entry]);
        cert.check_invariants()
            .expect("nothing else about this certificate is malformed");

        assert_eq!(
            cert.check_coverage_deficit(&events).unwrap_err(),
            InvariantError::CoverageDeficit {
                tool_kind: "bash".to_string()
            }
        );
    }

    /// The contract puts the same obligation on `task` (subagent
    /// delegation) as on `bash`.
    #[test]
    fn zero_coverage_observer_cannot_certify_task_as_valid() {
        let observer = ScriptedObserver::new(
            vec![tool_invoked(OBSERVER_PRODUCER, ToolKind::Task)],
            ScriptedObserver::zero_coverage(OBSERVER_PRODUCER),
        );
        let (events, manifest) = observe(&observer);

        let cert = valid_cert(vec![CoverageEntry::from(&manifest)]);
        assert_eq!(
            cert.check_coverage_deficit(&events).unwrap_err(),
            InvariantError::CoverageDeficit {
                tool_kind: "task".to_string()
            }
        );
    }

    // ---------------- the mirror-image positive case -----------------

    /// Identical event stream; only the manifest differs. Without this
    /// pair the negative test proves nothing about specificity — it
    /// could be failing for any reason.
    #[test]
    fn fs_covering_observer_permits_valid_bash() {
        let events = vec![tool_invoked(OBSERVER_PRODUCER, ToolKind::Bash)];

        let deficient = ScriptedObserver::new(
            events.clone(),
            ScriptedObserver::zero_coverage(OBSERVER_PRODUCER),
        );
        let covering = ScriptedObserver::new(
            events.clone(),
            ScriptedObserver::full_fs_coverage(OBSERVER_PRODUCER),
        );

        let (deficient_events, deficient_manifest) = observe(&deficient);
        let (covering_events, covering_manifest) = observe(&covering);
        assert_eq!(
            deficient_events, covering_events,
            "the two runs must differ only in declared coverage"
        );

        assert!(valid_cert(vec![CoverageEntry::from(&deficient_manifest)])
            .check_coverage_deficit(&deficient_events)
            .is_err());
        valid_cert(vec![CoverageEntry::from(&covering_manifest)])
            .check_coverage_deficit(&covering_events)
            .expect("a producer declaring fs.* discharges the obligation");
    }

    /// A tool kind with no observation obligation (`builtin` — the
    /// adapter sees its filesystem effects directly) does not trip the
    /// rule even under zero coverage.
    #[test]
    fn non_delegating_tool_kind_does_not_trip_the_rule() {
        let observer = ScriptedObserver::new(
            vec![tool_invoked(OBSERVER_PRODUCER, ToolKind::Builtin)],
            ScriptedObserver::zero_coverage(OBSERVER_PRODUCER),
        );
        let (events, manifest) = observe(&observer);
        valid_cert(vec![CoverageEntry::from(&manifest)])
            .check_coverage_deficit(&events)
            .expect("builtin tools carry no observer obligation");
    }

    // ---------------- producer-membership ----------------------------

    /// An event whose producer is absent from `observation_coverage` is
    /// rejected outright — the certificate cannot interpret silences
    /// from a producer it never heard of.
    #[test]
    fn producer_missing_from_coverage_is_rejected() {
        let observer = ScriptedObserver::new(
            vec![ir_event(
                "freshdag-observer-unregistered",
                EventKind::FsRead,
                serde_json::json!({ "path": "/repo/in.txt", "size": 3 }),
            )],
            ScriptedObserver::full_fs_coverage(OBSERVER_PRODUCER),
        );
        let (events, manifest) = observe(&observer);

        // Coverage is otherwise generous (blanket fs.*), so the only
        // thing that can fail here is producer membership.
        let cert = valid_cert(vec![CoverageEntry::from(&manifest)]);
        assert_eq!(
            cert.check_coverage_deficit(&events).unwrap_err(),
            InvariantError::ProducerMissingFromCoverage {
                producer: "freshdag-observer-unregistered".to_string()
            }
        );
    }

    // ---------------- the real macOS posture -------------------------

    /// End-to-end shape of a v0 macOS run: the adapter sees `tool.*`,
    /// the stub observer sees nothing below the tool layer, and the
    /// computation shelled out. The certificate contract says this is
    /// `unknown`, not `valid` — and `check_coverage_deficit` is what
    /// makes that mechanical.
    #[test]
    fn stub_observer_plus_adapter_cannot_certify_bash_as_valid() {
        let stub = StubObserver::new();
        let stub_manifest = stub.coverage();
        assert!(stub_manifest.emits.is_empty());

        let events = vec![tool_invoked(ADAPTER_PRODUCER, ToolKind::Bash)];
        let cert = valid_cert(vec![
            CoverageEntry::from(&adapter_manifest(&["session.*", "computation.*", "tool.*"])),
            CoverageEntry::from(&stub_manifest),
        ]);

        assert_eq!(
            cert.check_coverage_deficit(&events).unwrap_err(),
            InvariantError::CoverageDeficit {
                tool_kind: "bash".to_string()
            },
            "macOS + Bash must not certify as valid"
        );
    }

    /// The fsatrace backend's own manifest does not discharge a `bash`
    /// obligation on **any** platform, for two different reasons.
    ///
    /// Off Linux, `observe` cannot run at all, so the manifest declares
    /// no coverage; a manifest still claiming `fs.*` there would launder
    /// zero observation into a `valid` certificate.
    ///
    /// On Linux it declares `fs.read` coverage but flags it
    /// `under-approximates` — mmap reads are not emitted, statically
    /// linked processes are invisible — and ADR 0011 requires every
    /// matching `partial` entry to be `over-approximates` before an
    /// obligation is discharged.
    ///
    /// This assertion used to read `result.expect("on Linux fsatrace
    /// really does cover fs.*")`. It inverted with the ADR 0011
    /// migration, and that is **the expected finding, not a
    /// regression** (ADR 0011, Amendment, Correction 3): an observer
    /// that cannot see mmap reads or statically linked processes cannot
    /// answer the question the coverage-deficit rule asks. The way to
    /// make this test assert discharge again is to close the observer
    /// gap — emit mmap reads, cover static binaries — not to relabel
    /// the note. Under invariant #15 a spurious `unknown` is the price
    /// we accept; a spurious `valid` is what invariant #7 forbids.
    #[test]
    fn fsatrace_manifest_does_not_discharge_a_bash_obligation() {
        let manifest =
            crate::linux::FsatraceObserver::with_binary("/nonexistent/fsatrace", "sess").coverage();
        let events = vec![tool_invoked("freshdag-observer-fsatrace", ToolKind::Bash)];
        let entry = CoverageEntry::from(&manifest);

        // Precondition, so a passing test cannot be an artefact of an
        // empty manifest on whichever platform this runs on.
        if cfg!(target_os = "linux") {
            assert!(
                entry.covers(EventKind::FsRead),
                "on Linux the manifest must still *claim* fs.read; the \
                 deficit has to come from its fidelity, not its absence"
            );
            assert!(
                !entry.discharges(EventKind::FsRead),
                "under-approximating fs.read must not discharge"
            );
        } else {
            assert!(!entry.covers(EventKind::FsRead));
        }

        assert_eq!(
            valid_cert(vec![entry])
                .check_coverage_deficit(&events)
                .unwrap_err(),
            InvariantError::CoverageDeficit {
                tool_kind: "bash".to_string()
            }
        );
    }

    /// The Linux arm of the test above cannot run on other platforms, so
    /// pin the rule it depends on directly: a manifest shaped exactly
    /// like the Linux fsatrace one — observer role, `fs.read` declared,
    /// `under-approximates` — must not discharge.
    #[test]
    fn an_under_approximating_observer_does_not_discharge_anywhere() {
        let mut manifest = observer_manifest(&["fs.read", "fs.write"]);
        manifest.partial.insert(
            "fs.read".to_string(),
            freshdag_core::ir::PartialCoverage::new(
                freshdag_core::ir::PartialReason::UnderApproximates,
                "mmap reads are not emitted; statically linked processes are invisible",
            ),
        );
        let entry = CoverageEntry::from(&manifest);

        assert!(entry.covers(EventKind::FsRead), "it does claim the kind");
        assert!(
            !entry.discharges(EventKind::FsRead),
            "but a missed-event admission must not discharge it"
        );
    }

    // ---------------- REGRESSION: the gap this workstream found ------

    /// Regression test for the invariant-#7 hole this workstream found
    /// and Wave 2 Phase A closed. It began life as a characterization
    /// test asserting `is_ok()` — the defective behavior — and was
    /// inverted once `ProducerRole` landed. **Do not weaken it.**
    ///
    /// certificate-contract §Coverage-Deficit requires an *observer*
    /// producer to discharge the obligation created by a `bash`/`task`
    /// invocation. The original implementation accepted **any**
    /// producer whose `emits` matched `fs.read`/`fs.write`, and
    /// `CoverageEntry` carried no way to tell the difference.
    ///
    /// That was not hypothetical: adapter-contract §Required Behavior
    /// #4's canonical manifest declares exactly
    /// `emits: [..., "fs.read", "fs.write"]` with
    /// `partial: { "fs.read": "only from Read tool; subprocess reads
    /// via observer" }` — the adapter's own `partial` note states the
    /// fact that should disqualify it, with nothing enforcing it. So
    /// once real producer manifests were attached to certificates, the
    /// Claude adapter's fs claim would have discharged the obligation
    /// for a Bash invocation it explicitly did not observe, making the
    /// rule vacuous on exactly the platform (macOS) it exists to
    /// protect.
    ///
    /// The fix is `ProducerRole`: only `role: Observer` discharges.
    /// This test pins the *role* half of the rule — vantage point —
    /// which is what it was written for and what it still checks.
    ///
    /// It used to add that a `partial`-based rule "would mean nothing
    /// could ever discharge the obligation", because the fsatrace
    /// observer "legitimately carries `partial` notes". ADR 0011,
    /// Amendment, Corrections 1-3 withdrew both halves of that: those
    /// notes describe events that are *not emitted*, so they are
    /// `under-approximates`, and ADR 0011 deliberately added the
    /// fidelity check the sentence argued against. Role and fidelity
    /// are now both required, and this observer currently satisfies
    /// only the first — which is the expected post-migration state, not
    /// a regression.
    #[test]
    fn adapter_fs_claim_does_not_discharge_bash_obligation() {
        let stub_manifest = StubObserver::new().coverage();
        let adapter = adapter_manifest(&["tool.*", "fs.read", "fs.write"]);
        let events = vec![tool_invoked(ADAPTER_PRODUCER, ToolKind::Bash)];

        // Precondition: the adapter really does claim fs coverage, so
        // this test fails for the intended reason rather than because
        // the manifest was empty.
        let adapter_entry = CoverageEntry::from(&adapter);
        assert!(adapter_entry.covers(EventKind::FsRead));
        assert!(matches!(adapter_entry.role, ProducerRole::Adapter));

        let cert = valid_cert(vec![adapter_entry, CoverageEntry::from(&stub_manifest)]);

        assert!(
            matches!(
                cert.check_coverage_deficit(&events),
                Err(InvariantError::CoverageDeficit { ref tool_kind }) if tool_kind == "bash"
            ),
            "an adapter's fs.* claim must not discharge a bash obligation \
             it cannot see inside (invariant #7)"
        );
    }
}
