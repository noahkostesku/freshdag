//! Golden-file conformance harness for `fixtures/observer-conformance/`.
//!
//! `docs/contracts/observer-contract.md §Testing` defines observer
//! conformance in terms of this fixture set. It did not exist until now,
//! which is why a real defect — the `m|<dst>|<src>` move line parsed as
//! a single path, emitting a write at a path that cannot exist — shipped
//! behind a `partial` note asserting the opposite. A fixture is evidence;
//! a note is a claim.
//!
//! # Layout
//!
//! ```text
//! fixtures/observer-conformance/fsatrace/
//!   conformant/<case>/     behaviour that MEETS the contract
//!     trace.txt            raw fsatrace output, read as bytes
//!     expected.jsonl       canonical IR stream, compared byte for byte
//!     README.md            what the case pins (optional)
//!   known-gap/<case>/      behaviour that does NOT meet the contract
//!     trace.txt
//!     expected.jsonl       what this backend emits TODAY
//!     gap.md               REQUIRED: which clause it fails, and why
//! ```
//!
//! # Why `known-gap/` exists
//!
//! Three of the adversarial fixtures §Testing names — the rename dance,
//! the mmap read, the symlink swap — exercise Required Behavior clauses
//! this backend does not implement. Goldening them under `conformant/`
//! would assert conformance that does not exist. Deleting them would
//! lose the only executable record of the gap.
//!
//! So they are goldened as-is under `known-gap/`, each with a `gap.md`
//! naming the clause it fails. The effect is that the gap is machine-
//! visible instead of prose: when someone implements the clause, the
//! golden fails loudly and the case is promoted to `conformant/`. A
//! passing `known-gap/` fixture means "still broken, still known"; a
//! failing one means "someone fixed it, move the directory."
//!
//! Adding a fixture requires ZERO changes to this file.
//!
//! Regenerate goldens with:
//! `FRESHDAG_BLESS=1 cargo test -p freshdag-observer`
//! Review the diff. A golden that moves without a deliberate behaviour
//! change is a regression — and under `known-gap/`, a golden that moves
//! is the good news.

use std::fs;
use std::path::{Path, PathBuf};

use freshdag_observer::determinism::{FixedClock, SeededIdGen};
use freshdag_observer::linux::parse_fsatrace_lines_with;

/// Session id every fixture runs under, so goldens do not encode a
/// caller's identity.
const FIXTURE_SESSION: &str = "conformance-session";
/// Producer version every fixture runs under, so goldens do not move
/// when the crate version does.
const FIXTURE_VERSION: &str = "0.0.0-conformance";

fn fixtures_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `.../crates/freshdag-observer`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("repo root")
        .join("fixtures")
        .join("observer-conformance")
        .join("fsatrace")
}

fn blessing() -> bool {
    std::env::var("FRESHDAG_BLESS").is_ok_and(|v| v == "1")
}

/// Every case directory, in `(kind, path)` form where `kind` is
/// `"conformant"` or `"known-gap"`.
fn discover() -> Vec<(&'static str, PathBuf)> {
    let root = fixtures_root();
    let mut out = Vec::new();
    for kind in ["conformant", "known-gap"] {
        let dir = root.join(kind);
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
            let path = entry.expect("dir entry").path();
            if path.is_dir() && path.join("trace.txt").is_file() {
                out.push((kind, path));
            }
        }
    }
    out.sort();
    out
}

/// Render a fixture's IR stream: one canonical JSON object per line.
fn render(trace: &str) -> String {
    let clock = FixedClock::conformance();
    let mut ids = SeededIdGen::conformance();
    let events =
        parse_fsatrace_lines_with(trace, FIXTURE_SESSION, FIXTURE_VERSION, &clock, &mut ids);
    let mut out = String::new();
    for e in &events {
        out.push_str(&serde_json::to_string(e).expect("an IrEvent serializes"));
        out.push('\n');
    }
    out
}

#[test]
fn every_fixture_matches_its_golden_ir_stream() {
    let cases = discover();
    assert!(
        !cases.is_empty(),
        "no fixtures under {} — observer-contract §Testing defines \
         conformance in terms of this set, so an empty set is a hole, \
         not a pass",
        fixtures_root().display()
    );

    let mut failures = Vec::new();
    for (kind, case) in &cases {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let trace = fs::read_to_string(case.join("trace.txt"))
            .unwrap_or_else(|e| panic!("{kind}/{name}: read trace.txt: {e}"));
        let actual = render(&trace);
        let golden_path = case.join("expected.jsonl");

        if blessing() {
            fs::write(&golden_path, &actual).expect("write golden");
            continue;
        }

        let expected = fs::read_to_string(&golden_path).unwrap_or_else(|e| {
            panic!("{kind}/{name}: read expected.jsonl: {e} (bless with FRESHDAG_BLESS=1)")
        });
        if expected != actual {
            failures.push(format!(
                "--- {kind}/{name}\nexpected:\n{expected}\nactual:\n{actual}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} fixture(s) drifted from their goldens:\n\n{}\n\
         If this is a deliberate behaviour change, re-bless with \
         FRESHDAG_BLESS=1 and review the diff. If a `known-gap/` case \
         failed, the gap may be CLOSED — check gap.md and promote the \
         case to conformant/.",
        failures.len(),
        failures.join("\n")
    );
}

/// A `known-gap/` case must say which clause it fails.
///
/// Without this, `known-gap/` becomes a place to park anything
/// inconvenient. The `gap.md` is the thing a reader finds when they ask
/// "why is the golden for the rename dance wrong?"
#[test]
fn every_known_gap_names_the_clause_it_fails() {
    for (kind, case) in discover() {
        if kind != "known-gap" {
            continue;
        }
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let gap = case.join("gap.md");
        let body = fs::read_to_string(&gap).unwrap_or_else(|e| {
            panic!(
                "known-gap/{name} has no gap.md ({e}). A goldened non-conformance \
                 must name the clause it fails, or it is indistinguishable from \
                 correct behaviour."
            )
        });
        assert!(
            body.contains("Required Behavior") || body.contains("Correctness Pitfalls"),
            "known-gap/{name}: gap.md must cite the observer-contract clause it \
             fails (Required Behavior #N or Correctness Pitfalls #N)"
        );
    }
}

/// The three adversarial fixtures §Testing names must all be present.
///
/// Named explicitly rather than counted, so deleting one to make the
/// suite green fails here instead of silently shrinking coverage.
#[test]
fn the_three_adversarial_fixtures_the_contract_names_all_exist() {
    let present: Vec<String> = discover()
        .iter()
        .map(|(_, p)| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    for required in ["rename-dance", "mmap-read", "symlink-swap"] {
        assert!(
            present.iter().any(|n| n == required),
            "observer-contract §Testing requires a `{required}` fixture; \
             present: {present:?}"
        );
    }
}

/// Goldens must be reproducible, not merely present.
///
/// Guards the determinism injection itself: if `parse_fsatrace_lines_with`
/// ever reached for a wall clock or a random id again, this fails while
/// the golden comparison above might still pass on a lucky first write.
#[test]
fn rendering_a_fixture_twice_is_byte_identical() {
    for (kind, case) in discover() {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let trace = fs::read_to_string(case.join("trace.txt")).expect("read trace");
        assert_eq!(
            render(&trace),
            render(&trace),
            "{kind}/{name} is not reproducible; the parser reached for \
             ambient time or randomness"
        );
    }
}

/// No fixture may emit a path containing the trace's field separator.
///
/// The defect this whole fixture set was stood up over. Asserted across
/// every case, including ones not written to test it.
#[test]
fn no_fixture_emits_a_path_containing_a_separator() {
    for (kind, case) in discover() {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let trace = fs::read_to_string(case.join("trace.txt")).expect("read trace");
        for line in render(&trace).lines() {
            let v: serde_json::Value = serde_json::from_str(line).expect("golden line is JSON");
            for field in ["path", "raw_path"] {
                if let Some(p) = v["payload"][field].as_str() {
                    assert!(
                        !p.contains('|'),
                        "{kind}/{name}: emitted {field} {p:?} contains a field \
                         separator — two trace fields were concatenated"
                    );
                }
            }
        }
    }
}

/// Sanity guard on the harness itself: the fixture root must be the one
/// the contract names, not a path that silently drifted.
#[test]
fn the_fixture_root_is_where_the_contract_says_it_is() {
    let root = fixtures_root();
    assert!(
        root.ends_with(Path::new("fixtures/observer-conformance/fsatrace")),
        "fixture root drifted: {}",
        root.display()
    );
    assert!(root.is_dir(), "{} does not exist", root.display());
}
