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
use freshdag_core::ir::IrEvent;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Expected {
    invariant_check: String,
    reason: String,
    /// `"pass"` | `"fail"`. Required iff the fixture ships an
    /// `events.json`, which is what makes
    /// [`Certificate::check_coverage_deficit`] runnable — that rule
    /// needs an event stream, not just the document.
    #[serde(default)]
    coverage_check: Option<String>,
    /// When `coverage_check` is `"fail"`, the expected `InvariantError`
    /// variant name.
    #[serde(default)]
    coverage_reason: Option<String>,
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
        InvariantError::CoverageDeficit { .. } => "CoverageDeficit",
        InvariantError::ProducerMissingFromCoverage { .. } => "ProducerMissingFromCoverage",
    }
}

/// Run `Certificate::check_coverage_deficit` for fixtures that ship an
/// `events.json`.
///
/// The coverage-deficit rule is the one place where the certificate
/// alone is not enough: it asks what the computation *did*, so it needs
/// the event stream. A fixture supplies one when it wants that rule
/// exercised; fixtures without `events.json` are unaffected.
fn check_coverage_deficit_fixture(
    fixture: &Path,
    cert: &Certificate,
    expected: &Expected,
    failures: &mut Vec<String>,
) {
    let events_path = fixture.join("events.json");
    if !events_path.exists() {
        if expected.coverage_check.is_some() {
            failures.push(format!(
                "{}: expected.coverage_check is set but there is no events.json to check against",
                fixture.display()
            ));
        }
        return;
    }

    let events: Vec<IrEvent> = match fs::read_to_string(&events_path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            failures.push(format!(
                "{}: events.json unreadable: {e}",
                fixture.display()
            ));
            return;
        }
    };

    let Some(coverage_check) = expected.coverage_check.as_deref() else {
        failures.push(format!(
            "{}: ships events.json but expected.json has no coverage_check",
            fixture.display()
        ));
        return;
    };

    match (coverage_check, cert.check_coverage_deficit(&events)) {
        ("pass", Ok(())) => {}
        ("pass", Err(err)) => failures.push(format!(
            "{}: expected coverage pass but check_coverage_deficit returned {err}",
            fixture.display()
        )),
        ("fail", Ok(())) => failures.push(format!(
            "{}: expected coverage fail ({}) but check_coverage_deficit returned Ok \
             — invariant #7 regression",
            fixture.display(),
            expected.coverage_reason.as_deref().unwrap_or("?"),
        )),
        ("fail", Err(err)) => {
            let variant = invariant_error_variant_name(&err);
            let want = expected.coverage_reason.as_deref().unwrap_or("");
            if variant != want {
                failures.push(format!(
                    "{}: expected coverage variant {want} but got {variant}: {err}",
                    fixture.display()
                ));
            }
        }
        (other, _) => failures.push(format!(
            "{}: expected.coverage_check must be `pass` or `fail`; got `{other}`",
            fixture.display()
        )),
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

        check_coverage_deficit_fixture(fixture, &cert, &expected, &mut failures);
    }

    assert!(
        failures.is_empty(),
        "certificate conformance failures ({} of {}):\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n")
    );
}
