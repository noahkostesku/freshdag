//! Well-formedness test for `fixtures/scenarios/`. The engine that
//! consumes these lands in Wave 2; today we assert only that every
//! scenario file parses, matches the schema, and carries the
//! invariant-#6/#7 mirror at the scenario level (any non-`valid`
//! status must carry at least one `reason_codes[]` entry, and every
//! such entry parses into the closed [`ReasonCode`] set).

use std::fs;
use std::path::{Path, PathBuf};

use freshdag_core::dependency::ReasonCode;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)] // `scenario` is the canonical name
struct Scenario {
    scenario: String,
    schema: String,
    description: String,
    input_observations: Vec<serde_json::Value>,
    #[serde(default)]
    input_probes: serde_json::Value,
    expected: Expected,
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Expected {
    dependency_graph: Vec<Edge>,
    certificate_status: CertStatus,
    #[serde(default)]
    invalidation: Option<Invalidation>,
}

#[derive(Debug, Deserialize)]
struct Edge {
    dependency_key: String,
    scheme: String,
    trust_class: String,
}

#[derive(Debug, Deserialize)]
struct CertStatus {
    value: String,
    #[serde(default)]
    reason_codes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Invalidation {
    #[allow(dead_code)]
    before_mutation: Option<String>,
    #[allow(dead_code)]
    after_mutation: Option<String>,
    /// Reason-pinned assertion for `after_mutation`
    /// (docs/EVALUATION.md §Reason-pinned assertions).
    #[serde(default)]
    after_mutation_reason_codes: Vec<String>,
    #[allow(dead_code)]
    #[serde(default)]
    mutated_dependency_keys: Vec<String>,
}

const LEGAL_STATUSES: &[&str] = &["valid", "likely-valid", "stale", "unknown"];
const LEGAL_TRUST_CLASSES: &[&str] = &["exact", "versioned", "heuristic", "volatile"];

fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
        .join("scenarios")
}

fn discover_scenarios(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root).expect("read scenarios dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// Every reason code must be a member of the closed [`ReasonCode`] set,
/// not arbitrary prose. This mirrors the schema enum in
/// `schemas/scenario/v0.1.json` and keeps scenario expectations
/// machine-checkable (invariant #13).
fn check_reason_codes(file: &Path, codes: &[String], failures: &mut Vec<String>) {
    for code in codes {
        let parsed: Result<ReasonCode, _> =
            serde_json::from_value(serde_json::Value::String(code.clone()));
        if parsed.is_err() {
            failures.push(format!(
                "{}: reason_code `{code}` is not a member of ReasonCode; \
                 scenario reason codes must use the canonical kebab-case wire form",
                file.display(),
            ));
        }
    }
}

/// Every dependency-graph edge names a legal trust class and a
/// non-empty key + scheme.
fn check_edges(file: &Path, edges: &[Edge], failures: &mut Vec<String>) {
    for edge in edges {
        if !LEGAL_TRUST_CLASSES.contains(&edge.trust_class.as_str()) {
            failures.push(format!(
                "{}: edge `{}` trust_class `{}` is not one of {LEGAL_TRUST_CLASSES:?}",
                file.display(),
                edge.dependency_key,
                edge.trust_class
            ));
        }
        if edge.dependency_key.is_empty() || edge.scheme.is_empty() {
            failures.push(format!(
                "{}: edge has empty dependency_key or scheme",
                file.display()
            ));
        }
    }
}

#[test]
fn every_scenario_is_well_formed() {
    let root = scenarios_root();
    let scenarios = discover_scenarios(&root);
    assert!(
        !scenarios.is_empty(),
        "expected at least one scenario under {}",
        root.display()
    );

    let mut failures = Vec::new();

    for dir in &scenarios {
        let file = dir.join("scenario.json");
        let text = match fs::read_to_string(&file) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: cannot read: {e}", file.display()));
                continue;
            }
        };
        let scenario: Scenario = match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: parse failed: {e}", file.display()));
                continue;
            }
        };

        // Schema string is authoritative.
        if scenario.schema != "freshdag.scenario/v0.1" {
            failures.push(format!(
                "{}: schema field is `{}`, expected freshdag.scenario/v0.1",
                file.display(),
                scenario.schema
            ));
        }

        // Every scenario names itself the same as its directory.
        let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if scenario.scenario != dir_name {
            failures.push(format!(
                "{}: scenario field `{}` does not match directory `{}`",
                file.display(),
                scenario.scenario,
                dir_name
            ));
        }

        // input_observations must be non-empty — every scenario has at
        // least one triggering event.
        if scenario.input_observations.is_empty() {
            failures.push(format!(
                "{}: input_observations is empty; scenarios must exercise at least one event",
                file.display()
            ));
        }

        // expected.certificate_status.value must be one of the four
        // legal statuses.
        if !LEGAL_STATUSES.contains(&scenario.expected.certificate_status.value.as_str()) {
            failures.push(format!(
                "{}: certificate_status.value `{}` is not one of {LEGAL_STATUSES:?}",
                file.display(),
                scenario.expected.certificate_status.value
            ));
        }

        // Invariant #6 mirror: any non-valid expected status carries
        // at least one reason code.
        if scenario.expected.certificate_status.value != "valid"
            && scenario.expected.certificate_status.reason_codes.is_empty()
        // dependency_graph may be empty for coverage-deficit —
        // reason_codes are still required.
        {
            failures.push(format!(
                "{}: expected status is `{}` but reason_codes is empty; \
                 mirrors invariant #6 at the scenario level",
                file.display(),
                scenario.expected.certificate_status.value
            ));
        }

        check_reason_codes(
            &file,
            &scenario.expected.certificate_status.reason_codes,
            &mut failures,
        );
        if let Some(inv) = &scenario.expected.invalidation {
            check_reason_codes(&file, &inv.after_mutation_reason_codes, &mut failures);
        }

        check_edges(&file, &scenario.expected.dependency_graph, &mut failures);

        // Notes and description are informational; enforce non-empty
        // description so scenarios stay self-explanatory.
        if scenario.description.trim().is_empty() {
            failures.push(format!("{}: description must be non-empty", file.display()));
        }

        // input_probes, if present, is a JSON object.
        if !scenario.input_probes.is_null() && !scenario.input_probes.is_object() {
            failures.push(format!(
                "{}: input_probes must be an object when present",
                file.display()
            ));
        }

        // notes is a Vec<String> — no per-note constraints today.
        let _ = &scenario.notes;
    }

    assert!(
        failures.is_empty(),
        "scenario well-formedness failures ({} of {}):\n{}",
        failures.len(),
        scenarios.len(),
        failures.join("\n")
    );
}
