//! Integration test — walks `fixtures/probe-conformance/https/` and
//! asserts every fixture's scripted endpoint produces the recorded
//! verdict.
//!
//! Adding a fixture requires ZERO changes to this file. Drop a directory
//! containing `scenario.json` and `expected.json` anywhere under the
//! fixture root and it is picked up. That is the point: the fixture set
//! is the contract's executable form, and it should be cheaper to extend
//! than the code that reads it.
//!
//! No network I/O. Fixtures drive
//! [`ScriptedTransport`](freshdag_probes::https::transport::ScriptedTransport),
//! an in-memory sequence of pre-decided turns.
//!
//! See `fixtures/probe-conformance/https/README.md` for the schema and
//! for what the adversarial fixtures are trying to prove.

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use freshdag_core::dependency::{Fingerprint, TrustClass};
use freshdag_core::probe::ProbeResult;
use freshdag_probes::https::headers::Headers;
use freshdag_probes::https::report::DiagnosticCode;
use freshdag_probes::https::transport::{
    HttpResponse, RepeatingBody, ScriptedTransport, ScriptedTurn, TransportError,
};
use freshdag_probes::{ContentHashFallback, HttpsProbe, HttpsProbeConfig};
use serde::Deserialize;

// --------------------------------------------------------------------
// Fixture schema
// --------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    /// Prose. Not asserted on; present so a fixture explains itself.
    #[allow(dead_code)]
    description: String,
    /// The dependency key. Stays constant across checks — a redirect
    /// target is metadata, never identity.
    key: String,
    #[serde(default)]
    config: ConfigSpec,
    checks: Vec<CheckSpec>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigSpec {
    max_redirects: Option<u8>,
    max_fetch_bytes: Option<u64>,
    allow_plaintext_http: Option<bool>,
    prefer_head: Option<bool>,
    content_hash_fallback: Option<FallbackSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum FallbackSpec {
    Never,
    Always,
    ForKeys { keys: Vec<String> },
}

impl ConfigSpec {
    fn build(&self) -> HttpsProbeConfig {
        let defaults = HttpsProbeConfig::default();
        HttpsProbeConfig {
            max_redirects: self.max_redirects.unwrap_or(defaults.max_redirects),
            max_fetch_bytes: self.max_fetch_bytes.unwrap_or(defaults.max_fetch_bytes),
            allow_plaintext_http: self
                .allow_plaintext_http
                .unwrap_or(defaults.allow_plaintext_http),
            prefer_head: self.prefer_head.unwrap_or(defaults.prefer_head),
            content_hash_fallback: match &self.content_hash_fallback {
                None | Some(FallbackSpec::Never) => ContentHashFallback::Never,
                Some(FallbackSpec::Always) => ContentHashFallback::Always,
                Some(FallbackSpec::ForKeys { keys }) => {
                    ContentHashFallback::for_keys(keys.iter().cloned())
                }
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckSpec {
    /// Names this check in failure output.
    name: String,
    /// Fingerprint on the certificate at the time of this check.
    recorded_fingerprint: Fingerprint,
    /// The endpoint's scripted turns, consumed in order.
    turns: Vec<TurnSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TurnSpec {
    /// Reply with a response.
    Respond {
        status: u16,
        /// Wire order is preserved and duplicates are significant, so
        /// this is a list of pairs, not a map.
        #[serde(default)]
        headers: Vec<(String, String)>,
        #[serde(default)]
        body: Option<BodySpec>,
    },
    /// Fail before any response.
    Fail { error: TransportErrorSpec },
}

#[derive(Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum BodySpec {
    /// Literal UTF-8 body.
    Text(String),
    /// `len` copies of one byte — lets a fixture describe an oversized
    /// body without shipping one.
    Repeated { repeat_byte: char, len: u64 },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum TransportErrorSpec {
    Timeout,
    Dns,
    Connect,
    Tls,
    Body,
    InvalidRequest,
    Other,
}

impl From<TransportErrorSpec> for TransportError {
    fn from(spec: TransportErrorSpec) -> Self {
        match spec {
            TransportErrorSpec::Timeout => Self::Timeout,
            TransportErrorSpec::Dns => Self::Dns,
            TransportErrorSpec::Connect => Self::Connect,
            TransportErrorSpec::Tls => Self::Tls,
            TransportErrorSpec::Body => Self::Body,
            TransportErrorSpec::InvalidRequest => Self::InvalidRequest,
            TransportErrorSpec::Other => Self::Other,
        }
    }
}

impl TurnSpec {
    fn build(self) -> ScriptedTurn {
        match self {
            Self::Fail { error } => ScriptedTurn::Fail(error.into()),
            Self::Respond {
                status,
                headers,
                body,
            } => {
                let headers: Headers = headers.into_iter().collect();
                let response = HttpResponse::new(status, headers);
                let response = match body {
                    None => response,
                    Some(BodySpec::Text(text)) => {
                        response.with_body(Cursor::new(text.into_bytes()))
                    }
                    Some(BodySpec::Repeated { repeat_byte, len }) => {
                        response.with_body(RepeatingBody::new(repeat_byte as u8, len))
                    }
                };
                ScriptedTurn::Respond(response)
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Expected {
    checks: Vec<ExpectedCheck>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedCheck {
    /// `match`, `drift`, or `unknown`.
    result: String,
    /// Asserted when present.
    #[serde(default)]
    observed_fingerprint: Option<Fingerprint>,
    #[serde(default)]
    trust_class: Option<TrustClass>,
    /// Exact bytes of `Unknown::reason`. Asserted when present, because
    /// this string lands in the `cert_id` preimage.
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    retryable: Option<bool>,
    /// The full set of diagnostic codes, order-insensitive. Asserted
    /// when present.
    #[serde(default)]
    diagnostics: Option<Vec<DiagnosticCode>>,
    /// HTTP requests the check should have issued.
    #[serde(default)]
    requests: Option<u32>,
}

// --------------------------------------------------------------------
// Discovery
// --------------------------------------------------------------------

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent() // .../crates
        .expect("crates dir")
        .parent() // repo root
        .expect("repo root")
        .join("fixtures")
        .join("probe-conformance")
        .join("https")
}

/// Any directory containing a `scenario.json` is a fixture, at any
/// depth. Categories are therefore free to add.
fn discover_fixtures(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            }
        }
        if dir.join("scenario.json").is_file() {
            out.push(dir);
        }
    }
    out.sort();
    out
}

// --------------------------------------------------------------------
// Execution
// --------------------------------------------------------------------

fn result_kind(result: &ProbeResult) -> &'static str {
    match result {
        ProbeResult::Match { .. } => "match",
        ProbeResult::Drift { .. } => "drift",
        ProbeResult::Unknown { .. } => "unknown",
    }
}

/// Run one fixture, appending human-readable failures rather than
/// panicking, so a single run reports every mismatch in the set.
fn run_fixture(fixture: &Path, failures: &mut Vec<String>) {
    let label = fixture
        .strip_prefix(fixtures_root().parent().unwrap_or(fixture))
        .unwrap_or(fixture)
        .display()
        .to_string();

    let scenario: Scenario = match read_json(&fixture.join("scenario.json")) {
        Ok(s) => s,
        Err(e) => {
            failures.push(format!("{label}: scenario.json: {e}"));
            return;
        }
    };
    let expected: Expected = match read_json(&fixture.join("expected.json")) {
        Ok(e) => e,
        Err(e) => {
            failures.push(format!("{label}: expected.json: {e}"));
            return;
        }
    };

    if scenario.checks.len() != expected.checks.len() {
        failures.push(format!(
            "{label}: scenario has {} checks but expected.json has {}",
            scenario.checks.len(),
            expected.checks.len()
        ));
        return;
    }

    let config = scenario.config.build();

    for (check, want) in scenario.checks.into_iter().zip(expected.checks.iter()) {
        let name = format!("{label}[{}]", check.name);
        run_check(&scenario.key, &config, check, want, &name, failures);
    }
}

/// Run one check of one fixture against its expectation.
fn run_check(
    key: &str,
    config: &HttpsProbeConfig,
    check: CheckSpec,
    want: &ExpectedCheck,
    name: &str,
    failures: &mut Vec<String>,
) {
    let turns: Vec<ScriptedTurn> = check.turns.into_iter().map(TurnSpec::build).collect();
    // A fresh transport per check: the probe is stateless by contract,
    // so nothing may carry over between checks except the recorded
    // fingerprint the fixture hands it.
    let transport = Arc::new(ScriptedTransport::new(turns));
    let probe = HttpsProbe::with_transport(transport, config.clone());
    let outcome = probe.check_detailed(key, &check.recorded_fingerprint, None);

    let got = result_kind(&outcome.result);
    if got != want.result {
        failures.push(format!(
            "{name}: expected {} but got {got}: {:?}",
            want.result, outcome.result
        ));
        return;
    }

    // Invariant #7, restated for every fixture regardless of what it
    // claims to test: an `unknown` expectation must never be satisfied
    // by a `Match`, and the equality above is the only thing standing
    // between those two.
    if want.result == "unknown" {
        assert!(
            !matches!(outcome.result, ProbeResult::Match { .. }),
            "{name}: fixture expected unknown; a Match here is an invariant-#7 violation"
        );
    }

    match &outcome.result {
        ProbeResult::Match {
            observed_fp,
            observed_trust_class,
        }
        | ProbeResult::Drift {
            observed_fp,
            observed_trust_class,
        } => {
            if let Some(expected_fp) = &want.observed_fingerprint {
                if observed_fp != expected_fp {
                    failures.push(format!(
                        "{name}: observed_fingerprint {observed_fp} != expected {expected_fp}"
                    ));
                }
            }
            if let Some(expected_class) = want.trust_class {
                if *observed_trust_class != expected_class {
                    failures.push(format!(
                        "{name}: trust_class {observed_trust_class:?} != expected {expected_class:?}"
                    ));
                }
            }
        }
        ProbeResult::Unknown { reason, retryable } => {
            if let Some(expected_reason) = &want.reason {
                if reason != expected_reason {
                    failures.push(format!(
                        "{name}: reason {reason:?} != expected {expected_reason:?}"
                    ));
                }
            }
            if let Some(expected_retryable) = want.retryable {
                if *retryable != expected_retryable {
                    failures.push(format!(
                        "{name}: retryable {retryable} != expected {expected_retryable}"
                    ));
                }
            }
        }
    }

    if let Some(expected_diagnostics) = &want.diagnostics {
        let mut expected_codes = expected_diagnostics.clone();
        expected_codes.sort_unstable();
        expected_codes.dedup();
        let got_codes = outcome.diagnostic_codes();
        if got_codes != expected_codes {
            failures.push(format!(
                "{name}: diagnostics {:?} != expected {:?}",
                codes_as_wire(&got_codes),
                codes_as_wire(&expected_codes)
            ));
        }
    }

    if let Some(expected_requests) = want.requests {
        if outcome.cost.requests != expected_requests {
            failures.push(format!(
                "{name}: issued {} requests, expected {expected_requests}",
                outcome.cost.requests
            ));
        }
    }
}

fn codes_as_wire(codes: &[DiagnosticCode]) -> Vec<&'static str> {
    codes.iter().map(|c| c.as_wire_str()).collect()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("malformed {}: {e}", path.display()))
}

#[test]
fn every_https_probe_fixture_matches_its_expected_outcome() {
    let root = fixtures_root();
    let fixtures = discover_fixtures(&root);
    assert!(
        !fixtures.is_empty(),
        "expected at least one fixture under {}",
        root.display()
    );

    let mut failures = Vec::new();
    for fixture in &fixtures {
        run_fixture(fixture, &mut failures);
    }

    assert!(
        failures.is_empty(),
        "https probe conformance failures ({} across {} fixtures):\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n")
    );
}

/// Every fixture must be reproducible: running it twice yields the same
/// verdict and, for `Unknown`, the byte-identical reason.
///
/// `Unknown::reason` becomes a certificate reason's `detail`, which is
/// inside the `cert_id` preimage. Nondeterminism there produces
/// certificates that cannot be reproduced, with nothing looking wrong.
#[test]
fn every_https_probe_fixture_is_deterministic_across_runs() {
    let fixtures = discover_fixtures(&fixtures_root());
    let mut first = BTreeMap::new();
    let mut second = BTreeMap::new();
    run_all_into(&fixtures, &mut first);
    run_all_into(&fixtures, &mut second);
    assert_eq!(
        first, second,
        "fixture verdicts differed between two identical runs"
    );
}

/// Collect `(fixture/check, verdict-description)` for every check.
fn run_all_into(fixtures: &[PathBuf], out: &mut BTreeMap<String, String>) {
    for fixture in fixtures {
        let Ok(scenario) = read_json::<Scenario>(&fixture.join("scenario.json")) else {
            continue;
        };
        let config = scenario.config.build();
        for check in scenario.checks {
            let key = format!("{}::{}", fixture.display(), check.name);
            let turns: Vec<ScriptedTurn> = check.turns.into_iter().map(TurnSpec::build).collect();
            let transport = Arc::new(ScriptedTransport::new(turns));
            let probe = HttpsProbe::with_transport(transport, config.clone());
            let outcome = probe.check_detailed(&scenario.key, &check.recorded_fingerprint, None);
            let description = match &outcome.result {
                ProbeResult::Match {
                    observed_fp,
                    observed_trust_class,
                } => format!("match/{observed_fp}/{observed_trust_class:?}"),
                ProbeResult::Drift {
                    observed_fp,
                    observed_trust_class,
                } => format!("drift/{observed_fp}/{observed_trust_class:?}"),
                ProbeResult::Unknown { reason, retryable } => {
                    format!("unknown/{reason}/{retryable}")
                }
            };
            let diagnostics = codes_as_wire(&outcome.diagnostic_codes()).join(",");
            out.insert(key, format!("{description} [{diagnostics}]"));
        }
    }
}
