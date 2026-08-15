//! The scenario harness: `fixtures/scenarios/*/scenario.json` executed
//! end to end against a real [`Engine`].
//!
//! This is the executable half of `docs/EVALUATION.md`. It is
//! deliberately dumb about FreshDAG's semantics — it drives the engine
//! and compares against the fixture's `expected` block, so a scenario
//! that passes here passes because the engine is right, not because the
//! harness agrees with it.
//!
//! # Producer roles
//!
//! `observation_coverage` needs a role and an `emits` list per producer,
//! and the scenario schema carries neither. The harness derives them
//! from the producer's name prefix and a fixed declaration table below.
//! The table is *fixed*, not inferred from what the producer actually
//! emitted in the scenario — deriving it from the events would make the
//! coverage-deficit rule vacuous, since every emitted kind would be
//! covered by construction.
//!
//! # Mutation
//!
//! `expected.invalidation` is modelled two ways, chosen by the fixture:
//!
//! - `mutated_dependency_keys` non-empty → each named key's scripted
//!   probe switches to its `after_mutation_yields` result. Keys with no
//!   scripted probe (the `irrelevant-file` case) change nothing.
//! - `mutated_dependency_keys` empty and `after_mutation` differs from
//!   `before_mutation` → the injected clock advances past every declared
//!   TTL. This is the `volatile-external-dep` case, whose own notes say
//!   "no world mutation required".

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use freshdag_core::artifact::ArtifactId;
use freshdag_core::dependency::{Dependency, Fingerprint, ReasonCode, TrustClass};
use freshdag_core::ir::{CoverageManifest, EventKind, EventKindPattern, IrEvent, ProducerRole};
use freshdag_core::probe::{Probe, ProbeResult};
use freshdag_store::{CoverageRegistry, DerivedGraph};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::{Engine, FixedClock, ProbeRegistry, ScriptedProbe};

const ADAPTER_EMITS: &[&str] = &[
    "session.*",
    "computation.*",
    "tool.*",
    "fs.read",
    "fs.write",
    "artifact.produced",
    "diagnostic",
];
const OBSERVER_EMITS: &[&str] = &["fs.*", "proc.*", "net.*", "diagnostic"];
const PROBE_EMITS: &[&str] = &["probe.checked", "diagnostic"];

// ------------------------------------------------------------- the file

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)] // `scenario` is the fixture's own field name
pub struct Scenario {
    pub scenario: String,
    #[allow(dead_code)]
    pub schema: String,
    #[allow(dead_code)]
    pub description: String,
    pub input_observations: Vec<IrEvent>,
    #[serde(default)]
    pub input_probes: BTreeMap<String, ProbeScript>,
    pub expected: Expected,
    #[serde(default)]
    #[allow(dead_code)]
    pub notes: Vec<String>,
}

/// A scripted probe answer, per `schemas/scenario/v0.1.json`
/// `input_probes`.
#[derive(Debug, Default, Deserialize)]
pub struct ProbeScript {
    /// Trust class of the probe's observation. Defaults to the
    /// dependency's recorded class.
    trust_class: Option<String>,
    /// Result before any mutation.
    no_change_yields: Option<String>,
    /// Result after the world mutates.
    after_mutation_yields: Option<String>,
    /// Synonym retained for `versioned-external-dep`, whose block
    /// predates the general name.
    version_bump_yields: Option<String>,
    observed_fingerprint: Option<String>,
    mutated_fingerprint: Option<String>,
    recorded_version: Option<u64>,
    #[serde(default)]
    retryable: bool,
}

#[derive(Debug, Deserialize)]
pub struct Expected {
    pub dependency_graph: Vec<Edge>,
    pub certificate_status: CertStatus,
    #[serde(default)]
    pub invalidation: Option<Invalidation>,
}

#[derive(Debug, Deserialize)]
pub struct Edge {
    pub dependency_key: String,
    pub scheme: String,
    pub trust_class: String,
}

#[derive(Debug, Deserialize)]
pub struct CertStatus {
    pub value: String,
    #[serde(default)]
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Invalidation {
    pub before_mutation: Option<String>,
    pub after_mutation: Option<String>,
    #[serde(default)]
    pub after_mutation_reason_codes: Vec<String>,
    #[serde(default)]
    pub mutated_dependency_keys: Vec<String>,
}

// -------------------------------------------------------------- running

pub fn scenarios_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("repo root")
        .join("fixtures")
        .join("scenarios")
}

pub fn discover() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(scenarios_root()).expect("read scenarios dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() && path.join("scenario.json").is_file() {
            out.push(path.join("scenario.json"));
        }
    }
    out.sort();
    out
}

fn role_of(producer: &str) -> (ProducerRole, &'static [&'static str]) {
    if producer.starts_with("freshdag-adapter-") {
        (ProducerRole::Adapter, ADAPTER_EMITS)
    } else if producer.starts_with("freshdag-observer-") {
        (ProducerRole::Observer, OBSERVER_EMITS)
    } else if producer.starts_with("freshdag-probe-") {
        (ProducerRole::Probe, PROBE_EMITS)
    } else {
        panic!(
            "scenario names producer `{producer}`, whose role the harness cannot derive; \
             name it freshdag-adapter-*, freshdag-observer-* or freshdag-probe-*"
        )
    }
}

fn coverage_for(events: &[IrEvent]) -> CoverageRegistry {
    let mut registry = CoverageRegistry::in_memory();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for event in events {
        let key = (event.producer.clone(), event.producer_version.clone());
        if !seen.insert(key) {
            continue;
        }
        let (role, emits) = role_of(&event.producer);
        registry
            .register_at(
                CoverageManifest {
                    producer: event.producer.clone(),
                    version: event.producer_version.clone(),
                    role,
                    platforms: Vec::new(),
                    emits: emits.iter().map(|s| EventKindPattern::new(*s)).collect(),
                    partial: BTreeMap::new(),
                    capabilities: BTreeMap::new(),
                    known_limitations: Vec::new(),
                },
                OffsetDateTime::UNIX_EPOCH,
            )
            .expect("register manifest");
    }
    registry
}

fn scheme_of(key: &str, recorded: Option<&Dependency>) -> String {
    recorded.map_or_else(
        || {
            key.split_once("://").map_or_else(
                || panic!("input_probes key `{key}` has no scheme"),
                |(scheme, _)| scheme.to_string(),
            )
        },
        |dep| dep.scheme.clone(),
    )
}

fn trust_of(name: &str) -> TrustClass {
    match name {
        "exact" => TrustClass::Exact,
        "versioned" => TrustClass::Versioned,
        "heuristic" => TrustClass::Heuristic,
        "volatile" => TrustClass::Volatile,
        other => panic!("unknown trust class `{other}` in input_probes"),
    }
}

impl ProbeScript {
    fn trust(&self, recorded: Option<&Dependency>) -> TrustClass {
        self.trust_class.as_deref().map_or_else(
            || recorded.map_or(TrustClass::Exact, |d| d.trust_class),
            trust_of,
        )
    }

    fn matched_fingerprint(&self, recorded: Option<&Dependency>) -> Fingerprint {
        self.observed_fingerprint.as_deref().map_or_else(
            || {
                recorded.map_or_else(
                    || "custom:unscripted".parse().expect("fingerprint"),
                    |d| d.fingerprint.clone(),
                )
            },
            |s| s.parse().expect("observed_fingerprint parses"),
        )
    }

    fn drifted_fingerprint(&self, recorded: Option<&Dependency>) -> Fingerprint {
        if let Some(raw) = &self.mutated_fingerprint {
            return raw.parse().expect("mutated_fingerprint parses");
        }
        if let Some(version) = self.recorded_version {
            return format!("version:{}", version + 1)
                .parse()
                .expect("version fingerprint");
        }
        let _ = recorded;
        "custom:mutated".parse().expect("fingerprint")
    }

    fn result(&self, outcome: &str, recorded: Option<&Dependency>) -> ProbeResult {
        match outcome {
            "match" => ProbeResult::Match {
                observed_fp: self.matched_fingerprint(recorded),
                observed_trust_class: self.trust(recorded),
            },
            "drift" => ProbeResult::Drift {
                observed_fp: self.drifted_fingerprint(recorded),
                observed_trust_class: self.trust(recorded),
            },
            "unknown" => ProbeResult::Unknown {
                reason: "scripted-unknown".to_string(),
                retryable: self.retryable,
            },
            other => panic!("unknown scripted probe outcome `{other}`"),
        }
    }

    fn before(&self, recorded: Option<&Dependency>) -> Option<ProbeResult> {
        self.no_change_yields
            .as_deref()
            .map(|o| self.result(o, recorded))
    }

    fn after(&self, recorded: Option<&Dependency>) -> Option<ProbeResult> {
        self.after_mutation_yields
            .as_deref()
            .or(self.version_bump_yields.as_deref())
            .map(|o| self.result(o, recorded))
    }
}

/// One scenario, loaded and runnable.
pub struct Run {
    pub name: String,
    pub expected: Expected,
    pub engine: Engine,
    pub clock: Arc<FixedClock>,
    pub artifact: ArtifactId,
    scripts: BTreeMap<String, (Arc<ScriptedProbe>, ProbeScript)>,
    recorded: BTreeMap<String, Dependency>,
    max_ttl_seconds: u64,
}

impl Run {
    pub fn load(path: &Path) -> Self {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let scenario: Scenario = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()));

        let events = scenario.input_observations;
        let coverage = coverage_for(&events);

        // Replay first, so scripted probes can default their answers
        // from the dependency the store actually recorded.
        let preview = DerivedGraph::replay(events.clone(), &coverage);
        let recorded: BTreeMap<String, Dependency> = preview
            .computations()
            .flat_map(|n| n.dependencies.iter())
            .map(|d| (d.key.clone(), d.clone()))
            .collect();

        let mut by_scheme: BTreeMap<String, Arc<ScriptedProbe>> = BTreeMap::new();
        let mut scripts = BTreeMap::new();
        for (key, script) in scenario.input_probes {
            let scheme = scheme_of(&key, recorded.get(&key));
            let probe = by_scheme
                .entry(scheme.clone())
                .or_insert_with(|| Arc::new(ScriptedProbe::new(&scheme)))
                .clone();
            if let Some(result) = script.before(recorded.get(&key)) {
                probe.set(key.clone(), result);
            }
            scripts.insert(key, (probe, script));
        }

        let mut probes = ProbeRegistry::new();
        for probe in by_scheme.into_values() {
            probes
                .register(probe as Arc<dyn Probe>)
                .expect("scenario probes do not tie");
        }

        let last_ts = events
            .iter()
            .map(|e| e.ts)
            .max()
            .expect("scenario has events");
        let clock = Arc::new(FixedClock::new(last_ts + time::Duration::seconds(60)));

        let artifact = events
            .iter()
            .find(|e| e.kind == EventKind::ArtifactProduced)
            .and_then(|e| e.payload.get("artifact_id"))
            .and_then(serde_json::Value::as_str)
            .map(|id| ArtifactId(id.to_string()))
            .expect("scenario declares an artifact.produced event");

        let max_ttl_seconds = events
            .iter()
            .filter_map(|e| e.payload.get("ttl_seconds"))
            .filter_map(serde_json::Value::as_u64)
            .max()
            .unwrap_or(0);

        let engine = Engine::builder()
            .events(events)
            .coverage(coverage)
            .probes(probes)
            .clock(clock.clone())
            .build();

        Self {
            name: scenario.scenario,
            expected: scenario.expected,
            engine,
            clock,
            artifact,
            scripts,
            recorded,
            max_ttl_seconds,
        }
    }

    pub fn check(&self) -> crate::CheckOutcome {
        self.engine
            .check(&self.artifact)
            .unwrap_or_else(|e| panic!("{}: engine refused to emit: {e}", self.name))
    }

    /// Apply the fixture's mutation model. Returns false if the fixture
    /// declares no invalidation block.
    pub fn mutate(&self) -> bool {
        let Some(inval) = &self.expected.invalidation else {
            return false;
        };
        if inval.mutated_dependency_keys.is_empty() {
            if inval.after_mutation != inval.before_mutation {
                self.clock.advance(time::Duration::seconds(
                    i64::try_from(self.max_ttl_seconds).expect("ttl fits") + 60,
                ));
            }
            return true;
        }
        for key in &inval.mutated_dependency_keys {
            if let Some((probe, script)) = self.scripts.get(key) {
                if let Some(result) = script.after(self.recorded.get(key)) {
                    probe.set(key.clone(), result);
                }
            }
        }
        true
    }
}

// ----------------------------------------------------------- assertions

pub fn status_wire(status: freshdag_core::dependency::ValidityStatus) -> String {
    serde_json::to_value(status)
        .expect("status serializes")
        .as_str()
        .expect("status is a string")
        .to_string()
}

pub fn reason_wire(code: ReasonCode) -> String {
    code.as_wire_str().to_string()
}

/// Compare a certificate against an expected status block.
///
/// Reason assertions follow `docs/EVALUATION.md §Reason-pinned
/// assertions`: `reasons[0]` is pinned exactly (the ordering is
/// contractual, so this is well defined) and every expected code must be
/// present. Containment rather than set equality, because the engine may
/// legitimately report additional *true* reasons — the coverage-deficit
/// scenario is `unknown` both because a subprocess was unobserved and,
/// in consequence, because no dependency was observed.
pub fn assert_status(
    scenario: &str,
    phase: &str,
    actual: &freshdag_core::certificate::Status,
    expected_value: &str,
    expected_codes: &[String],
    failures: &mut Vec<String>,
) {
    let value = status_wire(actual.value);
    if value != expected_value {
        failures.push(format!(
            "{scenario} [{phase}]: status is `{value}`, expected `{expected_value}` \
             (reasons: {:?})",
            actual
                .reasons
                .iter()
                .map(|r| reason_wire(r.reason))
                .collect::<Vec<_>>()
        ));
        return;
    }

    let actual_codes: Vec<String> = actual
        .reasons
        .iter()
        .map(|r| reason_wire(r.reason))
        .collect();
    if expected_value == "valid" && !actual_codes.is_empty() {
        failures.push(format!(
            "{scenario} [{phase}]: a valid certificate carries no reasons, got {actual_codes:?}"
        ));
    }
    if let Some(first) = expected_codes.first() {
        match actual_codes.first() {
            Some(actual_first) if actual_first == first => {}
            other => failures.push(format!(
                "{scenario} [{phase}]: reasons[0] is {other:?}, expected `{first}` \
                 (full: {actual_codes:?})"
            )),
        }
    }
    for code in expected_codes {
        if !actual_codes.contains(code) {
            failures.push(format!(
                "{scenario} [{phase}]: expected reason `{code}` absent from {actual_codes:?}"
            ));
        }
    }
}

pub fn assert_graph(
    scenario: &str,
    actual: &[Dependency],
    expected: &[Edge],
    failures: &mut Vec<String>,
) {
    if actual.len() != expected.len() {
        failures.push(format!(
            "{scenario}: dependency graph has {} edge(s), expected {}: {:?}",
            actual.len(),
            expected.len(),
            actual.iter().map(|d| d.key.as_str()).collect::<Vec<_>>()
        ));
        return;
    }
    for (dep, edge) in actual.iter().zip(expected.iter()) {
        if dep.key != edge.dependency_key {
            failures.push(format!(
                "{scenario}: edge key is `{}`, expected `{}`",
                dep.key, edge.dependency_key
            ));
        }
        if dep.scheme != edge.scheme {
            failures.push(format!(
                "{scenario}: edge `{}` scheme is `{}`, expected `{}`",
                dep.key, dep.scheme, edge.scheme
            ));
        }
        let trust = serde_json::to_value(dep.trust_class)
            .expect("trust serializes")
            .as_str()
            .expect("string")
            .to_string();
        if trust != edge.trust_class {
            failures.push(format!(
                "{scenario}: edge `{}` trust class is `{}`, expected `{}`",
                dep.key, trust, edge.trust_class
            ));
        }
    }
}
