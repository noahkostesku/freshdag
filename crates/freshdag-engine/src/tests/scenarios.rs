//! `fixtures/scenarios/*` executed end to end.
//!
//! This is what `freshdag test fixtures/*` will call once the CLI lands
//! (W7). Until then it runs in `cargo test -p freshdag-engine`.

use super::harness::{assert_graph, assert_status, discover, Run};

/// Every scenario, in one test, reporting every failure rather than the
/// first. A single failing assertion tells you less than the shape of
/// all of them.
#[test]
fn every_scenario_matches_its_expected_block() {
    let files = discover();
    assert!(
        files.len() >= 6,
        "expected at least the six v0 scenarios, found {}",
        files.len()
    );

    let mut failures: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for file in files {
        let run = Run::load(&file);
        seen.push(run.name.clone());

        let outcome = run.check();
        assert_graph(
            &run.name,
            &outcome.certificate.depends_on,
            &run.expected.dependency_graph,
            &mut failures,
        );
        assert_status(
            &run.name,
            "initial",
            &outcome.certificate.status,
            &run.expected.certificate_status.value,
            &run.expected.certificate_status.reason_codes,
            &mut failures,
        );

        let Some(inval) = &run.expected.invalidation else {
            continue;
        };
        if let Some(before) = &inval.before_mutation {
            assert_status(
                &run.name,
                "before_mutation",
                &outcome.certificate.status,
                before,
                &run.expected.certificate_status.reason_codes,
                &mut failures,
            );
        }
        if !run.mutate() {
            continue;
        }
        let Some(after) = &inval.after_mutation else {
            continue;
        };
        let mutated = run.check();
        assert_status(
            &run.name,
            "after_mutation",
            &mutated.certificate.status,
            after,
            &inval.after_mutation_reason_codes,
            &mut failures,
        );

        // Certificates are immutable: rechecking produces a successor.
        if after != &run.expected.certificate_status.value
            && mutated.certificate.cert_id == outcome.certificate.cert_id
        {
            failures.push(format!(
                "{}: the successor certificate reuses the predecessor's cert_id",
                run.name
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "scenarios {seen:?} produced {} failure(s):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// `docs/EVALUATION.md §Reason-pinned assertions`: any fixture whose
/// expected status is not `valid` must pin the *why*, otherwise a broken
/// engine that always answers `stale` passes.
#[test]
fn every_non_valid_expectation_pins_a_reason() {
    let mut failures = Vec::new();
    for file in discover() {
        let run = Run::load(&file);
        if run.expected.certificate_status.value != "valid"
            && run.expected.certificate_status.reason_codes.is_empty()
        {
            failures.push(format!("{}: certificate_status pins no reason", run.name));
        }
        let Some(inval) = &run.expected.invalidation else {
            continue;
        };
        if let Some(after) = &inval.after_mutation {
            if after != "valid" && inval.after_mutation_reason_codes.is_empty() {
                failures.push(format!(
                    "{}: after_mutation is `{after}` but pins no reason",
                    run.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
