//! End-to-end golden test for `freshdag check`.
//!
//! Three scenarios run the real binary against a real store and a real
//! `file://` probe, and their combined output is pinned at
//! `tests/cli-golden/demo.txt`. A reflow of CLI text, a changed reason
//! sentence, or a changed exit code is a visible diff.
//!
//! # How this stays deterministic
//!
//! `.claude/rules/testing.md` forbids a test that needs a retry or a
//! wall clock. Four sources of nondeterminism exist here and each is
//! handled explicitly rather than tolerated:
//!
//! 1. **Event timestamps, ids, hashes, and the recipe hash** are
//!    constants authored below. Nothing is read from the clock or from
//!    `Uuid::new_v4`.
//! 2. **Absolute paths.** `file://` dependency keys are absolute by
//!    construction, so the scenario root is rewritten to `<TMP>` before
//!    comparison. The assertion at the end proves no other machine
//!    path survived.
//! 3. **`status.checked`** comes from the engine's clock, which in
//!    production is the system clock. The line is replaced by
//!    `<CHECKED-AT>` — but only after being *parsed as RFC 3339*, so
//!    the golden still proves the CLI emits a well-formed timestamp.
//! 4. **`cert_id`** hashes `status.checked`, so it moves with it. Same
//!    treatment: validated as `blake3:` + 64 lowercase hex, then
//!    replaced.
//!
//! Nothing else is normalized. In particular, statuses, reason codes,
//! reason order, dependency keys, coverage entries, prose, and exit
//! codes are all compared byte for byte.
//!
//! No TTL-bearing (`volatile`) dependency appears in any scenario, so
//! no *verdict* depends on the clock — only the two display fields
//! above do.

use std::path::{Path, PathBuf};
use std::process::Command;

use freshdag_core::certificate::Certificate;
use freshdag_core::dependency::{ReasonCode, ValidityStatus};
use freshdag_core::ir::{CoverageManifest, EventKind, EventKindPattern, IrEvent, ProducerRole};
use freshdag_probes::FileProbe;
use freshdag_store::Store;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

const ADAPTER: &str = "freshdag-adapter-claude";
const ADAPTER_VERSION: &str = "0.1.0";
const SESSION: &str = "sess-demo";
const RECIPE: &str = "research-account";
const RECIPE_HASH: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const ARTIFACT_HASH: &str =
    "blake3:2222222222222222222222222222222222222222222222222222222222222222";
const ARTIFACT_PATH: &str = "briefs/acme.md";

/// The adapter's declared coverage. Deliberately the honest list: it
/// claims the filesystem events it synthesizes from tool inputs, and
/// nothing about what happens inside a subprocess.
const ADAPTER_EMITS: &[&str] = &[
    "session.*",
    "computation.*",
    "tool.*",
    "fs.read",
    "fs.write",
    "artifact.produced",
    "diagnostic",
];

/// Base timestamp for every fixture event: 2026-01-01T00:00:00Z.
fn base_ts() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("valid fixed timestamp")
}

/// One second per event, from [`base_ts`].
fn offset_ts(index: usize) -> OffsetDateTime {
    base_ts() + time::Duration::seconds(i64::try_from(index).expect("small index"))
}

/// A fixed, dense event id. No `Uuid::new_v4` anywhere in this file.
fn event_id(index: usize) -> Uuid {
    Uuid::from_u128(0x7000_0000_0000_0000_0000 + u128::try_from(index).expect("small index"))
}

// ------------------------------------------------------------ fixtures

/// A scenario: a store, a working tree, and the artifact to ask about.
struct Scenario {
    name: &'static str,
    root: PathBuf,
    store_dir: PathBuf,
    events: Vec<IrEvent>,
    /// Files to (re)write *after* the log records their fingerprints,
    /// which is how drift is staged.
    mutate_after: Vec<(PathBuf, &'static str)>,
}

impl Scenario {
    fn new(name: &'static str) -> Self {
        let root = target_tmp().join(name);
        // A stale directory from a previous run would make the test
        // depend on history. Start from nothing, every time.
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear scenario root");
        }
        std::fs::create_dir_all(root.join("work")).expect("create work dir");
        Self {
            name,
            store_dir: root.join("store"),
            root,
            events: Vec::new(),
            mutate_after: Vec::new(),
        }
    }

    /// A file the computation read, recorded at the fingerprint the
    /// file has *now*.
    fn read(mut self, file: &str, contents: &str) -> Self {
        let path = self.root.join("work").join(file);
        std::fs::write(&path, contents).expect("write dependency file");
        let hash = FileProbe::hash_file(&path).expect("hash dependency file");
        self.events.push(self.event(
            self.events.len(),
            ADAPTER,
            EventKind::FsRead,
            serde_json::json!({
                "path": path,
                "size": contents.len(),
                "hash": hash.to_string(),
            }),
        ));
        self
    }

    /// Rewrite a dependency after its fingerprint was recorded — the
    /// world moving on under a finished artifact.
    fn then_change(mut self, file: &str, contents: &'static str) -> Self {
        self.mutate_after
            .push((self.root.join("work").join(file), contents));
        self
    }

    /// A `bash` invocation. Only an observer can see what a subprocess
    /// touches, and v0 ships none for macOS, so this creates an
    /// obligation nothing here can discharge.
    fn bash(mut self, command: &'static str) -> Self {
        self.events.push(self.event(
            self.events.len(),
            ADAPTER,
            EventKind::ToolInvoked,
            serde_json::json!({
                "tool_name": "bash",
                "tool_kind": "bash",
                "tool_input": { "command": command },
                "cwd": self.root.join("work"),
            }),
        ));
        self
    }

    fn event(
        &self,
        index: usize,
        producer: &str,
        kind: EventKind,
        payload: serde_json::Value,
    ) -> IrEvent {
        IrEvent {
            // Fixed, dense ids: reproducible and totally ordered.
            event_id: event_id(index),
            producer: producer.to_string(),
            producer_version: ADAPTER_VERSION.to_string(),
            session_id: SESSION.to_string(),
            computation_id: Some(self.computation()),
            parent_id: None,
            causal_inputs: None,
            ts: offset_ts(index),
            kind,
            payload,
        }
    }

    fn computation(&self) -> String {
        format!("comp:{}", self.name)
    }

    /// Materialize the store: lifecycle events around the body, the
    /// adapter's coverage manifest, and any staged mutation.
    fn build(mut self) -> Self {
        let body: Vec<IrEvent> = std::mem::take(&mut self.events);

        let mut events = vec![self.event(
            0,
            ADAPTER,
            EventKind::ComputationStarted,
            serde_json::json!({ "recipe_id": RECIPE, "recipe_hash": RECIPE_HASH }),
        )];
        for (i, mut e) in body.into_iter().enumerate() {
            e.event_id = event_id(i + 1);
            e.ts = offset_ts(i + 1);
            events.push(e);
        }
        let n = events.len();
        events.push(self.event(
            n,
            ADAPTER,
            EventKind::FsWrite,
            serde_json::json!({
                "path": ARTIFACT_PATH,
                "size": 4213,
                "mode": "truncate",
                "hash": ARTIFACT_HASH,
            }),
        ));
        events.push(self.event(
            n + 1,
            ADAPTER,
            EventKind::ArtifactProduced,
            serde_json::json!({
                "artifact_id": ARTIFACT_HASH,
                "path": ARTIFACT_PATH,
                "content_hash": ARTIFACT_HASH,
                "kind": "text/markdown",
                "size": 4213,
                "produced_by": self.computation(),
                "comparator": "exact",
            }),
        ));
        events.push(self.event(
            n + 2,
            ADAPTER,
            EventKind::ComputationEnded,
            serde_json::json!({ "status": "ok" }),
        ));

        let mut store = Store::open(&self.store_dir).expect("open store");
        store
            .register_producer(manifest(ADAPTER, ProducerRole::Adapter, ADAPTER_EMITS))
            .expect("register adapter coverage");
        store.append_all(&events).expect("append events");
        store.sync().expect("sync store");

        for (path, contents) in &self.mutate_after {
            std::fs::write(path, contents).expect("mutate dependency");
        }
        self
    }

    /// Run `freshdag check` and return `(exit code, stdout)`.
    fn check(&self, extra: &[&str]) -> (i32, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_freshdag"));
        cmd.arg("check")
            .arg(ARTIFACT_PATH)
            .arg("--store")
            .arg(&self.store_dir)
            .args(extra);
        let out = cmd.output().expect("run freshdag");
        let code = out.status.code().unwrap_or_else(|| {
            panic!(
                "freshdag was killed by a signal; stderr:\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (
            code,
            String::from_utf8(out.stdout).expect("stdout is utf-8"),
        )
    }
}

fn manifest(producer: &str, role: ProducerRole, emits: &[&str]) -> CoverageManifest {
    CoverageManifest {
        producer: producer.to_string(),
        version: ADAPTER_VERSION.to_string(),
        role,
        platforms: Vec::new(),
        emits: emits.iter().map(|s| EventKindPattern::new(*s)).collect(),
        partial: std::collections::BTreeMap::new(),
        capabilities: std::collections::BTreeMap::new(),
        known_limitations: Vec::new(),
    }
}

fn target_tmp() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cli-golden")
}

// ------------------------------------------------------- normalization

/// Replace the two fields that cannot be reproduced, and the scenario
/// root. Everything replaced is validated first, so normalization
/// cannot hide a malformed value.
fn normalize(raw: &str, root: &Path) -> String {
    let root = root.to_string_lossy().to_string();
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        let line = line.replace(&root, "<TMP>");
        let line = redact(&line, "Checked", "<CHECKED-AT>", |v| {
            OffsetDateTime::parse(v, &Rfc3339).is_ok()
        })
        .or_else(|| {
            redact(&line, "Certificate", "<CERT-ID>", |v| {
                v.strip_prefix("blake3:").is_some_and(|hex| {
                    hex.len() == 64
                        && hex
                            .chars()
                            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
                })
            })
        })
        .unwrap_or(line);
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Rewrite `"<label><pad><value>"` to `"<label><pad><placeholder>"`,
/// asserting the value was well-formed. Padding is preserved so a
/// change to the label column still shows up in the golden.
fn redact(
    line: &str,
    label: &str,
    placeholder: &str,
    valid: impl Fn(&str) -> bool,
) -> Option<String> {
    let rest = line.strip_prefix(label)?;
    if !rest.starts_with(' ') {
        return None;
    }
    let value = rest.trim_start();
    assert!(
        valid(value),
        "`{label}` field is malformed and would have been hidden by \
         normalization: {value:?}"
    );
    Some(format!(
        "{label}{}{placeholder}",
        " ".repeat(rest.len() - value.len())
    ))
}

// ------------------------------------------------------------- the test

fn section(title: &str, scenario: &Scenario, code: i32, stdout: &str) -> String {
    format!(
        "=== {title} ===\n\
         $ freshdag check {ARTIFACT_PATH} --store <TMP>/store\n\
         {}\n[exit {code}]\n",
        normalize(stdout, &scenario.root).trim_end_matches('\n')
    )
}

#[test]
fn check_demo_matches_the_golden() {
    let mut demo = String::new();

    // 1. Valid — every dependency verified unchanged at exact trust.
    let valid = Scenario::new("valid")
        .read("ICP.md", "ideal customer profile v3\n")
        .read("pricing.md", "seat-based, 2026-01\n")
        .build();
    let (code, stdout) = valid.check(&[]);
    assert_eq!(code, 0, "a fully verified artifact must exit 0\n{stdout}");
    demo.push_str(&section("valid: nothing changed", &valid, code, &stdout));
    demo.push('\n');

    // 2. Stale — one dependency drifted under a finished artifact.
    let stale = Scenario::new("stale")
        .read("ICP.md", "ideal customer profile v3\n")
        .read("pricing.md", "seat-based, 2026-01\n")
        .then_change("pricing.md", "usage-based, 2026-02\n")
        .build();
    let (code, stdout) = stale.check(&[]);
    assert_eq!(code, 1, "observed drift must exit 1\n{stdout}");
    demo.push_str(&section(
        "stale: a dependency drifted",
        &stale,
        code,
        &stdout,
    ));
    demo.push('\n');

    // 3. Unknown by coverage deficit — the macOS honesty case. Every
    //    dependency the adapter *could* see still matches; the artifact
    //    is unknown because a subprocess ran that nothing observed.
    let unknown = Scenario::new("unknown-coverage")
        .read("ICP.md", "ideal customer profile v3\n")
        .bash("python enrich.py > /tmp/out")
        .build();
    let (code, stdout) = unknown.check(&[]);
    assert_eq!(
        code, 2,
        "an unobserved subprocess must exit 2, never 0\n{stdout}"
    );
    demo.push_str(&section(
        "unknown: a subprocess nobody observed",
        &unknown,
        code,
        &stdout,
    ));

    let golden_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cli-golden")
        .join("demo.txt");
    let golden = std::fs::read_to_string(&golden_path).unwrap_or_default();

    if demo != golden {
        // Written under `target/`, not next to the golden: a stray
        // `.actual` beside the checked-in file invites someone to
        // "fix" a regression by copying it over without reading it.
        let actual = target_tmp().join("demo.actual.txt");
        std::fs::write(&actual, &demo).expect("write actual output");
        panic!(
            "CLI output does not match {}.\nActual output written to {}.\n\
             Diff the two: if the change is intended, copy it over; if not, \
             the CLI regressed.\n\n--- actual ---\n{demo}",
            golden_path.display(),
            actual.display()
        );
    }

    // Normalization must not have papered over a machine-specific path.
    for leak in [env!("CARGO_MANIFEST_DIR"), env!("CARGO_TARGET_TMPDIR")] {
        assert!(
            !demo.contains(leak),
            "the golden contains a path from this machine: {leak}"
        );
    }
}

#[test]
fn json_output_is_the_certificate_and_keeps_the_exit_code() {
    let scenario = Scenario::new("json")
        .read("ICP.md", "ideal customer profile v3\n")
        .then_change("ICP.md", "ideal customer profile v4\n")
        .build();

    let (code, stdout) = scenario.check(&["--json"]);
    assert_eq!(code, 1, "--json must not change the verdict\n{stdout}");

    // Deserializing through `Certificate` is the real assertion: a
    // reason code outside the closed vocabulary would fail here.
    let cert: Certificate = serde_json::from_str(&stdout).expect("stdout is a certificate");
    assert_eq!(cert.status.value, ValidityStatus::Stale);
    assert_eq!(
        cert.status
            .reasons
            .iter()
            .map(|r| r.reason)
            .collect::<Vec<_>>(),
        vec![ReasonCode::Drift]
    );
    assert_eq!(cert.schema, "freshdag.certificate/v0.1");
    assert!(cert.status.reasons[0].dependency_key.ends_with("ICP.md"));
}

#[test]
fn the_json_and_prose_paths_agree_on_the_verdict() {
    let scenario = Scenario::new("agreement")
        .read("ICP.md", "ideal customer profile v3\n")
        .bash("python enrich.py")
        .build();

    let (prose_code, prose) = scenario.check(&[]);
    let (json_code, json) = scenario.check(&["--json"]);
    assert_eq!(prose_code, json_code, "the two renderings disagree");
    assert_eq!(prose_code, 2);

    let cert: Certificate = serde_json::from_str(&json).expect("certificate");
    assert_eq!(cert.status.value, ValidityStatus::Unknown);
    assert!(cert
        .status
        .reasons
        .iter()
        .any(|r| r.reason == ReasonCode::CoverageDeficit));
    // The prose path must name the code it is explaining.
    assert!(prose.contains("coverage-deficit"));
}

#[test]
fn likely_valid_is_not_reusable_without_an_explicit_opt_in() {
    // No fixture here produces `likely-valid` (it needs a heuristic or
    // volatile edge, which needs a probe that reports one), so this
    // guards the flag's *other* half: `--accept-likely-valid` must not
    // promote a stale or unknown verdict.
    let scenario = Scenario::new("accept-flag")
        .read("ICP.md", "ideal customer profile v3\n")
        .bash("python enrich.py")
        .build();

    let (code, _) = scenario.check(&["--accept-likely-valid"]);
    assert_eq!(
        code, 2,
        "--accept-likely-valid must not turn unknown into fresh"
    );
}

#[test]
fn a_missing_store_is_a_tool_error_not_a_verdict() {
    let root = target_tmp().join("no-store");
    let _ = std::fs::remove_dir_all(&root);
    let out = Command::new(env!("CARGO_BIN_EXE_freshdag"))
        .args(["check", "whatever", "--store"])
        .arg(&root)
        .output()
        .expect("run freshdag");
    assert!(
        out.status.code().is_some_and(|c| c > 2),
        "a missing store exited {:?}; that is in the verdict range",
        out.status.code()
    );
    assert!(
        !root.exists(),
        "`check` created a store; a query must not fabricate one"
    );
}

#[test]
fn an_unknown_artifact_is_a_tool_error_not_a_verdict() {
    let scenario = Scenario::new("no-such-artifact")
        .read("ICP.md", "ideal customer profile v3\n")
        .build();
    let (code, _) = {
        let out = Command::new(env!("CARGO_BIN_EXE_freshdag"))
            .args(["check", "nope.md", "--store"])
            .arg(&scenario.store_dir)
            .output()
            .expect("run freshdag");
        (out.status.code().unwrap_or(-1), out.stdout)
    };
    assert!(code > 2, "an unknown artifact exited {code}");
}

#[test]
fn unimplemented_subcommands_do_not_exit_zero() {
    for args in [
        vec!["why", "x"],
        vec!["cert", "x"],
        vec!["graph"],
        vec!["watch"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_freshdag"))
            .args(&args)
            .output()
            .expect("run freshdag");
        assert!(
            out.status.code().is_some_and(|c| c > 2),
            "`freshdag {}` exited {:?}; a stub must not report success",
            args.join(" "),
            out.status.code()
        );
        assert!(
            out.stdout.is_empty(),
            "`freshdag {}` wrote to stdout; a stub has nothing to say there",
            args.join(" ")
        );
    }
}

#[test]
fn a_bad_flag_does_not_look_like_a_validity_verdict() {
    // clap's own default failure code is 2, which this CLI defines as
    // `unknown`. A typo must not read as a verdict.
    let out = Command::new(env!("CARGO_BIN_EXE_freshdag"))
        .args(["check", "x", "--not-a-flag"])
        .output()
        .expect("run freshdag");
    assert!(
        out.status.code().is_some_and(|c| c > 2),
        "a usage error exited {:?}",
        out.status.code()
    );
}

#[test]
fn help_and_version_exit_zero() {
    for flag in ["--help", "--version"] {
        let out = Command::new(env!("CARGO_BIN_EXE_freshdag"))
            .arg(flag)
            .output()
            .expect("run freshdag");
        assert_eq!(
            out.status.code(),
            Some(0),
            "`freshdag {flag}` did not exit 0"
        );
    }
}
