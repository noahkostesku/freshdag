//! Integration test — walks `fixtures/certificate-conformance/` and
//! asserts every fixture matches its `expected.json` outcome.
//!
//! Adding a fixture (positive or illegal) requires ZERO changes to
//! this file. That is the point: the fixture set is the trust model,
//! and it grows monotonically.
//!
//! See `fixtures/certificate-conformance/README.md`.

use std::fs;
use std::path::{Path, PathBuf};

use freshdag_core::certificate::{Certificate, InvariantError};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Expected {
    invariant_check: String,
    reason: String,
}

fn fixtures_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `.../crates/freshdag-core`.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent() // .../crates
        .unwrap()
        .parent() // .../
        .unwrap()
        .join("fixtures")
        .join("certificate-conformance")
}

fn discover_fixtures(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for kind in ["positive", "illegal"] {
        let kind_dir = root.join(kind);
        if !kind_dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&kind_dir).expect("read fixtures dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn invariant_error_variant_name(err: &InvariantError) -> &'static str {
    match err {
        InvariantError::SchemaMismatch(_) => "SchemaMismatch",
        InvariantError::ValidWithLowerTrust { .. } => "ValidWithLowerTrust",
        InvariantError::MissingRecipeHash { .. } => "MissingRecipeHash",
        InvariantError::MissingReasons { .. } => "MissingReasons",
        InvariantError::NakedVolatile(_) => "NakedVolatile",
        InvariantError::EmptyObservationCoverage => "EmptyObservationCoverage",
    }
}

#[test]
fn every_conformance_fixture_matches_its_expected_outcome() {
    let root = fixtures_root();
    let fixtures = discover_fixtures(&root);
    assert!(
        !fixtures.is_empty(),
        "expected at least one fixture under {}",
        root.display()
    );

    let mut failures = Vec::new();

    for fixture in &fixtures {
        let cert_path = fixture.join("certificate.json");
        let expected_path = fixture.join("expected.json");

        let cert_text = match fs::read_to_string(&cert_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!(
                    "{}: cannot read {}: {e}",
                    fixture.display(),
                    cert_path.display()
                ));
                continue;
            }
        };
        let expected_text = match fs::read_to_string(&expected_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!(
                    "{}: cannot read {}: {e}",
                    fixture.display(),
                    expected_path.display()
                ));
                continue;
            }
        };
        let expected: Expected = match serde_json::from_str(&expected_text) {
            Ok(x) => x,
            Err(e) => {
                failures.push(format!(
                    "{}: expected.json is malformed: {e}",
                    fixture.display()
                ));
                continue;
            }
        };

        let parse: Result<Certificate, _> = serde_json::from_str(&cert_text);
        let cert = match parse {
            Ok(c) => c,
            Err(e) => {
                // If a fixture's JSON can't even parse as a Certificate,
                // that is a fixture-authoring error — not what we're
                // checking here.
                failures.push(format!(
                    "{}: certificate.json failed to parse as Certificate: {e}",
                    fixture.display()
                ));
                continue;
            }
        };

        let outcome = cert.check_invariants();
        match (expected.invariant_check.as_str(), outcome) {
            ("pass", Ok(())) => { /* expected */ }
            ("pass", Err(err)) => failures.push(format!(
                "{}: expected pass ({}) but check_invariants returned {err}",
                fixture.display(),
                expected.reason,
            )),
            ("fail", Ok(())) => failures.push(format!(
                "{}: expected fail ({}) but check_invariants returned Ok — invariant regression",
                fixture.display(),
                expected.reason,
            )),
            ("fail", Err(err)) => {
                let variant = invariant_error_variant_name(&err);
                if variant != expected.reason {
                    failures.push(format!(
                        "{}: expected variant {} but got {}: {err}",
                        fixture.display(),
                        expected.reason,
                        variant,
                    ));
                }
            }
            (other, _) => failures.push(format!(
                "{}: expected.invariant_check must be `pass` or `fail`; got `{other}`",
                fixture.display()
            )),
        }
    }

    assert!(
        failures.is_empty(),
        "certificate conformance failures ({} of {}):\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n")
    );
}
