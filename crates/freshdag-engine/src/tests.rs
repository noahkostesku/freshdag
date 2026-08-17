//! Engine tests.
//!
//! Three layers:
//!
//! 1. **Source guards** — the coverage gate cannot be bypassed by adding
//!    a second `Certificate { .. }` construction site.
//! 2. **Unit tests** — one per reason code the engine emits, plus the
//!    arbitration, anti-thrash, and TTL paths.
//! 3. **The scenario harness** — `fixtures/scenarios/*` end to end.

mod gate;
mod harness;
mod scenarios;
mod support;

use std::path::PathBuf;
use std::sync::Arc;

use freshdag_core::dependency::{
    Fingerprint, FingerprintKind, ReasonCode, TrustClass, ValidityStatus,
};
use freshdag_core::ir::{EventKind, ProducerRole};
use freshdag_core::probe::ProbeResult;

use crate::{Clock, Engine, EngineError, FixedClock, ProbeIdentity, ProbeRegistry, ScriptedProbe};

use support::{blake3_of, Fixture};

// ---------------------------------------------------------- source guards

/// Every non-test source file of this crate.
///
/// The test tree is excluded because it necessarily *names* the patterns
/// these guards look for.
fn engine_sources() -> Vec<(PathBuf, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "tests") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("tests.rs") {
                let body = std::fs::read_to_string(&path).expect("read source");
                out.push((path, body));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Count occurrences of `needle` in real code, ignoring comment lines
/// and any hit that is the tail of a longer identifier (so
/// `MalformedCertificate {` is not a `Certificate {`).
fn count_code_occurrences(body: &str, needle: &str) -> usize {
    let mut total = 0;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let bytes = line.as_bytes();
        let mut from = 0;
        while let Some(offset) = line[from..].find(needle) {
            let at = from + offset;
            let preceded_by_ident =
                at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
            // `-> Certificate {`, `impl … for Certificate {` and friends
            // are type positions, not struct literals.
            let prefix = line[..at].trim_end();
            let type_position = prefix.ends_with("->")
                || prefix.ends_with("impl")
                || prefix.ends_with(" for")
                || prefix.ends_with(" as")
                || prefix.ends_with(':');
            if !preceded_by_ident && !type_position {
                total += 1;
            }
            from = at + needle.len();
        }
    }
    total
}

/// Mechanism 1 from `seal`'s header: a `Certificate` may be built in
/// exactly one place in this crate, and that place is the function that
/// runs the coverage gate.
///
/// If this test fails, someone added a second way to mint a certificate
/// and the compile-time token in `seal` no longer covers every path.
#[test]
fn certificate_is_constructed_in_exactly_one_place() {
    let mut sites = Vec::new();
    for (path, body) in engine_sources() {
        let count = count_code_occurrences(&body, "Certificate {");
        if count > 0 {
            sites.push((path, count));
        }
    }
    assert_eq!(
        sites.len(),
        1,
        "Certificate is constructed in {sites:?}; it must be constructed only in seal.rs"
    );
    assert!(
        sites[0].0.ends_with("seal.rs"),
        "the sole construction site is {:?}, expected seal.rs",
        sites[0].0
    );
    assert_eq!(
        sites[0].1, 1,
        "seal.rs constructs a Certificate more than once"
    );
}

/// Mechanism 1's other half: the gate is called from exactly one place.
///
/// A second call site would mean a second, unaudited path from evidence
/// to a status, which is precisely what invariant #7 cannot afford.
#[test]
fn coverage_deficit_is_checked_in_exactly_one_place() {
    let mut sites = Vec::new();
    for (path, body) in engine_sources() {
        // Skip this file: it names the function in order to test for it.
        if path.ends_with("tests.rs") {
            continue;
        }
        let count = count_code_occurrences(&body, "check_coverage_deficit(");
        if count > 0 {
            sites.push((path, count));
        }
    }
    assert_eq!(
        sites.len(),
        1,
        "check_coverage_deficit is called from {sites:?}; expected only seal.rs"
    );
    assert!(sites[0].0.ends_with("seal.rs"));
}

// ------------------------------------------------------------ unit tests

/// Wave 2 ships probes for `file` and `https` only. Every `attio://`,
/// `mcp://` and `web.search` dependency therefore hits an empty
/// registry, and the code MUST be `no-probe-available` — the statement
/// "no probe ran" — rather than `probe-unknown`, which asserts one did.
#[test]
fn an_unregistered_scheme_is_no_probe_available_not_probe_unknown() {
    let fixture = Fixture::new("no-probe").with_probe_edge(
        "attio",
        "company/acme",
        "version:42",
        TrustClass::Versioned,
        None,
    );
    let outcome = fixture.engine(ProbeRegistry::new()).check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    assert_eq!(
        outcome.certificate.status.reasons[0].reason,
        ReasonCode::NoProbeAvailable
    );
    assert_eq!(
        outcome.certificate.status.reasons[0].dependency_key,
        "attio://company/acme"
    );
    assert!(
        outcome
            .events
            .iter()
            .all(|e| e.kind != EventKind::ProbeChecked),
        "no probe ran, so no probe.checked may be emitted"
    );
}

/// The contrast case: a probe ran, and could not decide.
#[test]
fn a_retryable_probe_failure_is_probe_unknown() {
    let fixture = Fixture::new("probe-unknown").with_probe_edge(
        "https",
        "acme.com/pricing",
        "etag:\"abc\"",
        TrustClass::Versioned,
        None,
    );
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("https").with_fallback(
            ProbeResult::Unknown {
                reason: "http-status=500".to_string(),
                retryable: true,
            },
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    assert_eq!(
        outcome.certificate.status.reasons[0].reason,
        ReasonCode::ProbeUnknown
    );
    assert_eq!(
        outcome.certificate.status.reasons[0].detail.as_deref(),
        Some("http-status=500"),
        "the probe's reason string becomes the non-normative detail, never the code"
    );

    // execution-ir.md: `retryable` is REQUIRED when result is "unknown".
    let checked = outcome
        .events
        .iter()
        .find(|e| e.kind == EventKind::ProbeChecked)
        .expect("probe.checked emitted");
    assert_eq!(checked.payload["result"], "unknown");
    assert_eq!(checked.payload["retryable"], true);
}

/// `retryable` MUST be absent when the result is not `unknown`.
#[test]
fn probe_checked_omits_retryable_unless_the_result_is_unknown() {
    let fp = "blake3:".to_string() + &blake3_of("notes");
    let fixture = Fixture::new("retryable-absent").with_file_edge("/repo/notes.md", &fp);
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("file").with_fallback(
            ProbeResult::Match {
                observed_fp: fp.parse().expect("fingerprint"),
                observed_trust_class: TrustClass::Exact,
            },
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    let checked = outcome
        .events
        .iter()
        .find(|e| e.kind == EventKind::ProbeChecked)
        .expect("probe.checked emitted");
    assert_eq!(checked.payload["result"], "match");
    assert!(
        checked.payload.get("retryable").is_none(),
        "retryable must be absent on a non-unknown result"
    );
}

/// probe-contract §Anti-thrash: `Unknown { retryable: false }` means the
/// signal backing the recorded trust class is gone. Demotion is explicit
/// — a `probe.trust_demoted` diagnostic — and never silent.
#[test]
fn an_unretryable_probe_failure_demotes_explicitly() {
    let fixture = Fixture::new("demotion").with_probe_edge(
        "https",
        "acme.com/pricing",
        "etag:\"abc\"",
        TrustClass::Versioned,
        None,
    );
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("https").with_fallback(
            ProbeResult::Unknown {
                reason: "malformed-etag".to_string(),
                retryable: false,
            },
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    assert_eq!(
        outcome.certificate.status.reasons[0].reason,
        ReasonCode::ProbeTrustDemoted
    );
    let diagnostic = outcome
        .events
        .iter()
        .find(|e| e.kind == EventKind::Diagnostic)
        .expect("diagnostic emitted");
    assert_eq!(diagnostic.payload["message"], "probe.trust_demoted");
    assert_eq!(diagnostic.payload["from_trust_class"], "versioned");
}

/// ARCHITECTURE §7: a `volatile` edge outside its TTL is `Unknown`.
#[test]
fn an_expired_ttl_is_unknown_with_ttl_expired() {
    let fixture = Fixture::new("ttl").with_probe_edge(
        "web.search",
        "q=acme",
        &format!("blake3:{}", blake3_of("search")),
        TrustClass::Volatile,
        Some(3600),
    );
    let engine = fixture.engine(ProbeRegistry::new());
    engine.clock.advance(time::Duration::seconds(3601));
    let outcome = engine.check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    assert_eq!(
        outcome.certificate.status.reasons[0].reason,
        ReasonCode::TtlExpired
    );
    assert_eq!(
        outcome.certificate.status.reasons[0].detail.as_deref(),
        Some("ttl_seconds=3600")
    );
}

/// A `ttl_seconds` too large to add to `observed_at` must not panic, and
/// must not be read as "not expired".
///
/// `OffsetDateTime + Duration` panics on overflow and `ttl_seconds` is
/// an unvalidated `u64` off the log. `u64::MAX` is the obvious spelling
/// of "never expires" and `300000000000` is a seconds/nanoseconds unit
/// confusion; both used to abort the process with exit 101. An expiry
/// the engine cannot compute is indistinguishable from no TTL having
/// been recorded, so it degrades to `ttl-expired` / `unknown`.
#[test]
fn an_unrepresentable_ttl_is_expired_not_a_panic() {
    for secs in [300_000_000_000_u64, u64::MAX] {
        let fixture = Fixture::new("ttl-overflow").with_probe_edge(
            "web.search",
            "q=acme",
            &format!("blake3:{}", blake3_of("search")),
            TrustClass::Volatile,
            Some(secs),
        );
        let outcome = fixture.engine(ProbeRegistry::new()).check_ok();

        assert_eq!(
            outcome.certificate.status.value,
            ValidityStatus::Unknown,
            "ttl_seconds={secs} must degrade, not be believed"
        );
        assert_eq!(
            outcome.certificate.status.reasons[0].reason,
            ReasonCode::TtlExpired
        );
        assert_eq!(
            outcome.certificate.status.reasons[0].detail.as_deref(),
            Some(format!("ttl_seconds={secs} not-representable").as_str()),
            "the detail distinguishes an unrepresentable TTL from an elapsed one"
        );
    }
}

/// The boundary case the previous test must not swallow: the largest TTL
/// that *is* representable still behaves as a live window.
#[test]
fn a_large_but_representable_ttl_is_still_inside_its_window() {
    // Fixture observations sit at UNIX_EPOCH + 20_000 days; a century of
    // seconds is comfortably inside `OffsetDateTime`'s range, so this
    // TTL must be refused by the *ceiling*, not by the arithmetic. The
    // two guards carry distinct details; confusing them would hide a
    // regression in either.
    let secs = 100 * 365 * 24 * 60 * 60;
    let build = || {
        Fixture::new("ttl-large").with_probe_edge(
            "web.search",
            "q=acme",
            &format!("blake3:{}", blake3_of("search")),
            TrustClass::Volatile,
            Some(secs),
        )
    };
    let outcome = build().engine(ProbeRegistry::new()).check_ok();
    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    assert!(
        outcome.certificate.status.reasons[0]
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("exceeds max_volatile_ttl")),
        "a representable TTL must be refused by the ceiling: {:?}",
        outcome.certificate.status.reasons[0].detail
    );

    // Raise the ceiling past it and the same TTL is a live window, so
    // the arithmetic really did represent it.
    let outcome = build()
        .engine_with(ProbeRegistry::new(), |b| {
            b.max_volatile_ttl(std::time::Duration::from_secs(secs))
        })
        .check_ok();
    assert_eq!(
        outcome.certificate.status.value,
        ValidityStatus::LikelyValid
    );
}

/// ARCHITECTURE §7's other half: inside the TTL a `volatile` edge is
/// `likely-valid` — and can never be bare `valid`, whatever else holds.
#[test]
fn a_volatile_edge_inside_ttl_is_likely_valid_and_never_valid() {
    let fixture = Fixture::new("volatile-inside").with_probe_edge(
        "web.search",
        "q=acme",
        &format!("blake3:{}", blake3_of("search")),
        TrustClass::Volatile,
        Some(3600),
    );
    let outcome = fixture.engine(ProbeRegistry::new()).check_ok();

    assert_eq!(
        outcome.certificate.status.value,
        ValidityStatus::LikelyValid,
        "invariant #8: a volatile edge caps the artifact at likely-valid"
    );
    assert_eq!(
        outcome.certificate.status.reasons[0].reason,
        ReasonCode::TrustClassVolatileCapsAtLikelyValid
    );
}

/// ADR 0009 Amendment 1: a declared TTL is evidence only within the
/// engine's configured maximum. `ttl_seconds: 1000000000` (~31 years) is
/// well-formed, representable, and unexpired — and must still be
/// `unknown`, because a producer cannot buy freshness with a large
/// integer.
#[test]
fn a_ttl_beyond_the_configured_maximum_is_not_evidence() {
    let secs = 1_000_000_000_u64;
    let fixture = Fixture::new("ttl-too-long").with_probe_edge(
        "web.search",
        "q=acme",
        &format!("blake3:{}", blake3_of("search")),
        TrustClass::Volatile,
        Some(secs),
    );
    let outcome = fixture.engine(ProbeRegistry::new()).check_ok();

    assert_eq!(
        outcome.certificate.status.value,
        ValidityStatus::Unknown,
        "a 31-year declared lifetime is an unbounded assertion, not evidence"
    );
    assert_eq!(
        outcome.certificate.status.reasons[0].reason,
        ReasonCode::TtlExpired
    );
    assert_eq!(
        outcome.certificate.status.reasons[0].detail.as_deref(),
        Some(
            format!(
                "ttl_seconds={secs} exceeds max_volatile_ttl={}",
                crate::DEFAULT_MAX_VOLATILE_TTL.as_secs()
            )
            .as_str()
        )
    );
}

/// The bound is the *engine's*, not the producer's: an operator who
/// deliberately raises it gets the longer window, and the default is a
/// default rather than a hardcode.
#[test]
fn the_volatile_ttl_ceiling_is_injectable() {
    let secs = 1_000_000_000_u64;
    let fixture = Fixture::new("ttl-too-long-raised").with_probe_edge(
        "web.search",
        "q=acme",
        &format!("blake3:{}", blake3_of("search")),
        TrustClass::Volatile,
        Some(secs),
    );
    let outcome = fixture
        .engine_with(ProbeRegistry::new(), |b| {
            b.max_volatile_ttl(std::time::Duration::from_secs(secs))
        })
        .check_ok();
    assert_eq!(
        outcome.certificate.status.value,
        ValidityStatus::LikelyValid
    );
}

/// The default ceiling is 24h and is not silently something else.
#[test]
fn the_default_volatile_ttl_ceiling_is_twenty_four_hours() {
    assert_eq!(crate::DEFAULT_MAX_VOLATILE_TTL.as_secs(), 24 * 60 * 60);
}

/// ADR 0009 Amendment 2: a `probe.checked` dated in the future satisfies
/// `now > expires_at == false` forever, so a future `observed_at` is an
/// unbounded TTL in disguise. Beyond the skew tolerance it is `unknown`.
#[test]
fn a_future_observed_at_is_not_evidence() {
    let fixture = Fixture::new("future-observation").with_probe_edge(
        "web.search",
        "q=acme",
        &format!("blake3:{}", blake3_of("search")),
        TrustClass::Volatile,
        Some(3600),
    );
    let engine = fixture.engine(ProbeRegistry::new());
    // Rewind the checking clock far behind the observation, which is
    // indistinguishable from the observation being dated far ahead.
    engine.clock.advance(-time::Duration::days(365 * 73));
    let outcome = engine.check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    assert_eq!(
        outcome.certificate.status.reasons[0].reason,
        ReasonCode::TtlExpired
    );
    assert_eq!(
        outcome.certificate.status.reasons[0].detail.as_deref(),
        Some(
            format!(
                "observed_at-in-future skew_tolerance_seconds={}",
                crate::MAX_CLOCK_SKEW.as_secs()
            )
            .as_str()
        )
    );
}

/// Ordinary skew between the producing and the checking host must not
/// invalidate anything: the tolerance exists because two machines need
/// not share a clock.
#[test]
fn skew_inside_the_tolerance_is_still_evidence() {
    let fixture = Fixture::new("small-skew").with_probe_edge(
        "web.search",
        "q=acme",
        &format!("blake3:{}", blake3_of("search")),
        TrustClass::Volatile,
        Some(3600),
    );
    let engine = fixture.engine(ProbeRegistry::new());
    // The fixture's clock sits 300s after the observation; move it to
    // 30s *before*, which is inside the 60s tolerance.
    engine.clock.advance(-time::Duration::seconds(300 + 30));
    let outcome = engine.check_ok();
    assert_eq!(
        outcome.certificate.status.value,
        ValidityStatus::LikelyValid
    );
}

/// The regression D2 is: the volatile-inside-TTL rule must be a positive
/// branch keyed on the trust class, not a fallback in the
/// arbitration-failure path.
///
/// While it lived in `Err(no_probe)`, two edges at the same trust class,
/// inside the same TTL, over the same absent resource disagreed on the
/// strength of their scheme alone — `web.search://…` (nothing
/// registered) reported `likely-valid` and `file:///…` (a probe
/// registered, answering `Unknown` because the file is gone) reported
/// `unknown`. Registering an unrelated probe must not move the verdict.
#[test]
fn a_volatile_edge_inside_ttl_ignores_whether_a_probe_is_registered() {
    let fingerprint = format!("blake3:{}", blake3_of("volatile"));
    let run = |register: bool| {
        let fixture = Fixture::new("volatile-probe-independence").with_probe_edge(
            "https",
            "acme.com/pricing",
            &fingerprint,
            TrustClass::Volatile,
            Some(3600),
        );
        let mut registry = ProbeRegistry::new();
        if register {
            registry
                .register(Arc::new(ScriptedProbe::new("https").with_fallback(
                    ProbeResult::Unknown {
                        reason: "no such resource".to_string(),
                        retryable: false,
                    },
                )))
                .expect("register");
        }
        let outcome = fixture.engine(registry).check_ok();
        (
            outcome.certificate.status.value,
            outcome
                .certificate
                .status
                .reasons
                .iter()
                .map(|r| r.reason)
                .collect::<Vec<_>>(),
        )
    };

    let (unprobed_status, unprobed_reasons) = run(false);
    let (probed_status, probed_reasons) = run(true);
    assert_eq!(
        unprobed_status,
        ValidityStatus::LikelyValid,
        "ARCHITECTURE §7: inside its TTL a volatile edge is likely-valid"
    );
    assert_eq!(
        (probed_status, &probed_reasons),
        (unprobed_status, &unprobed_reasons),
        "registering an unrelated probe for the scheme changed the verdict"
    );
    assert_eq!(
        unprobed_reasons,
        vec![ReasonCode::TrustClassVolatileCapsAtLikelyValid]
    );
}

/// The contrast that keeps the test above honest: at a *non*-volatile
/// trust class the same registered probe does decide the edge, so the
/// positive branch is keyed on the trust class and nothing else.
#[test]
fn a_non_volatile_edge_still_depends_on_its_probe() {
    let fingerprint = format!("blake3:{}", blake3_of("volatile"));
    let fixture = Fixture::new("non-volatile-probe-dependence").with_probe_edge(
        "https",
        "acme.com/pricing",
        &fingerprint,
        TrustClass::Versioned,
        Some(3600),
    );
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("https").with_fallback(
            ProbeResult::Unknown {
                reason: "no such resource".to_string(),
                retryable: true,
            },
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();
    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    assert_eq!(
        outcome.certificate.status.reasons[0].reason,
        ReasonCode::ProbeUnknown
    );
}

/// certificate-contract §Coverage-Deficit: a `bash` invocation with no
/// observer-role producer in `observation_coverage` forbids `valid`,
/// even when every declared dependency is `exact` and matches.
#[test]
fn an_undischarged_bash_obligation_downgrades_a_would_be_valid_certificate() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("deficit")
        .with_file_edge("/repo/notes.md", &fp)
        .with_bash_invocation();
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("file").with_fallback(
            ProbeResult::Match {
                observed_fp: fp.parse().expect("fingerprint"),
                observed_trust_class: TrustClass::Exact,
            },
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    assert_eq!(
        outcome.certificate.status.value,
        ValidityStatus::Unknown,
        "every edge matched at exact trust, yet the subprocess was unobserved"
    );
    let deficit = outcome
        .certificate
        .status
        .reasons
        .iter()
        .find(|r| r.reason == ReasonCode::CoverageDeficit)
        .expect("coverage-deficit reason");
    assert_eq!(
        deficit.dependency_key, "",
        "artifact-scoped reasons carry the empty-string sentinel"
    );
}

/// The same shape with an observer present reaches `valid`. Without this
/// test the previous one would pass on an engine that never says `valid`.
#[test]
fn an_observer_discharges_the_bash_obligation() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("discharged")
        .with_file_edge("/repo/notes.md", &fp)
        .with_bash_invocation()
        .with_observer();
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("file").with_fallback(
            ProbeResult::Match {
                observed_fp: fp.parse().expect("fingerprint"),
                observed_trust_class: TrustClass::Exact,
            },
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Valid);
    assert!(outcome.certificate.status.reasons.is_empty());
}

/// An adapter's `fs.*` declaration must NOT discharge the obligation
/// (ADR 0006). This is the invariant-#7 hole the role field closed.
#[test]
fn an_adapters_fs_declaration_does_not_discharge_the_obligation() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("adapter-fs")
        .with_file_edge("/repo/notes.md", &fp)
        .with_bash_invocation()
        .with_adapter_emits(&["fs.*", "tool.*", "computation.*", "artifact.produced"]);
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("file").with_fallback(
            ProbeResult::Match {
                observed_fp: fp.parse().expect("fingerprint"),
                observed_trust_class: TrustClass::Exact,
            },
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    assert!(outcome
        .certificate
        .status
        .reasons
        .iter()
        .any(|r| r.reason == ReasonCode::CoverageDeficit));
}

/// The general deficit rule: `deficit = observed_effect_kinds -
/// covered_effect_kinds` over `fs.*`/`proc.*`/`net.*`.
#[test]
fn an_uncovered_effect_kind_downgrades_the_certificate() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("effect-deficit")
        .with_file_edge("/repo/notes.md", &fp)
        .with_net_fetch()
        // The adapter declares fs.* but not net.*, so the net.fetch it
        // emitted is an effect nobody claims coverage for.
        .with_adapter_emits(&["fs.*", "tool.*", "computation.*", "artifact.produced"]);
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("file").with_fallback(
            ProbeResult::Match {
                observed_fp: fp.parse().expect("fingerprint"),
                observed_trust_class: TrustClass::Exact,
            },
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    let deficit = outcome
        .certificate
        .status
        .reasons
        .iter()
        .find(|r| r.reason == ReasonCode::CoverageDeficit)
        .expect("coverage-deficit reason");
    assert_eq!(
        deficit.detail.as_deref(),
        Some("uncovered-effect-kinds=net.fetch")
    );
}

/// A producer that emitted events without registering a manifest makes
/// its own silences uninterpretable. That is evidence, so the status
/// downgrades rather than the certificate being refused.
#[test]
fn an_unregistered_producer_downgrades_with_producer_missing_from_coverage() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("unregistered")
        .with_file_edge("/repo/notes.md", &fp)
        .with_unregistered_producer();
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("file").with_fallback(
            ProbeResult::Match {
                observed_fp: fp.parse().expect("fingerprint"),
                observed_trust_class: TrustClass::Exact,
            },
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    let reason = outcome
        .certificate
        .status
        .reasons
        .iter()
        .find(|r| r.reason == ReasonCode::ProducerMissingFromCoverage)
        .expect("producer-missing-from-coverage reason");
    assert_eq!(
        reason.detail.as_deref(),
        Some("producer=some-unregistered-tool")
    );
}

/// probe-contract §Probe Arbitration: a tie fails loudly with a
/// diagnostic rather than silently picking either probe.
#[test]
fn an_arbitration_tie_emits_a_diagnostic_and_yields_no_probe_available() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("tie").with_probe_edge(
        "https",
        "acme.com/pricing",
        &fp,
        TrustClass::Versioned,
        None,
    );
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("https").with_priority(5)))
        .expect("register");
    // A different triple, so registration succeeds; both match this key
    // at the same priority, so selection is genuinely ambiguous.
    registry
        .register(Arc::new(
            ScriptedProbe::new("https")
                .with_priority(5)
                .with_host_pattern("*"),
        ))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Unknown);
    assert_eq!(
        outcome.certificate.status.reasons[0].reason,
        ReasonCode::NoProbeAvailable
    );
    let diagnostic = outcome
        .events
        .iter()
        .find(|e| e.kind == EventKind::Diagnostic)
        .expect("diagnostic emitted");
    assert_eq!(diagnostic.payload["message"], "probe.arbitration_tie");
}

/// probe-contract §Anti-thrash, "Probe removal": the engine treats
/// dependencies previously observed by an uninstalled probe as
/// `Unknown`. It does NOT fall through to a lower-trust probe.
#[test]
fn probe_removal_does_not_fall_through_to_another_probe() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("removal").with_file_edge("/repo/notes.md", &fp);
    let mut registry = ProbeRegistry::new();
    let high = registry
        .register(Arc::new(
            ScriptedProbe::new("file")
                .with_priority(10)
                .with_fallback(ProbeResult::Match {
                    observed_fp: fp.parse().expect("fingerprint"),
                    observed_trust_class: TrustClass::Exact,
                }),
        ))
        .expect("register");
    registry
        .register(Arc::new(
            ScriptedProbe::new("file")
                .with_priority(1)
                .with_fallback(ProbeResult::Match {
                    observed_fp: fp.parse().expect("fingerprint"),
                    observed_trust_class: TrustClass::Heuristic,
                }),
        ))
        .expect("register");

    let mut engine = fixture.engine(registry);
    let first = engine.check_ok();
    assert_eq!(first.certificate.status.value, ValidityStatus::Valid);

    assert!(engine.engine.deregister_probe(&high));
    let second = engine.check_ok();
    assert_eq!(second.certificate.status.value, ValidityStatus::Unknown);
    assert_eq!(
        second.certificate.status.reasons[0].reason,
        ReasonCode::NoProbeAvailable
    );
    assert_eq!(
        second.certificate.status.reasons[0].detail.as_deref(),
        Some("probe-removed=file#10"),
        "the removed probe is named; the surviving file#1 probe is not consulted"
    );
}

/// The adversarial input for the anti-thrash protocol, at engine level:
/// a source that alternates trust classes on every check.
///
/// The dependency is recorded `heuristic`. The probe answers `Match` but
/// alternates its observed trust class `versioned, heuristic, versioned,
/// heuristic`. Every check MUST report `likely-valid` with
/// `trust-class-heuristic-caps-at-likely-valid`; a single check reporting
/// `valid` would mean one higher-trust observation had been adopted, in
/// violation of the N=2 rule and of invariant #8.
#[test]
fn flapping_observations_never_flip_the_certificate_status() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("flap").with_probe_edge(
        "https",
        "acme.com/pricing",
        &fp,
        TrustClass::Heuristic,
        None,
    );
    let probe = Arc::new(ScriptedProbe::new("https"));
    let mut registry = ProbeRegistry::new();
    registry
        .register(probe.clone() as Arc<dyn freshdag_core::probe::Probe>)
        .expect("register");
    let engine = fixture.engine(registry);

    let flap = [
        TrustClass::Versioned,
        TrustClass::Heuristic,
        TrustClass::Versioned,
        TrustClass::Heuristic,
        TrustClass::Versioned,
        TrustClass::Heuristic,
    ];
    let mut statuses = Vec::new();
    for observed in flap {
        probe.set(
            "https://acme.com/pricing",
            ProbeResult::Match {
                observed_fp: fp.parse().expect("fingerprint"),
                observed_trust_class: observed,
            },
        );
        let outcome = engine.check_ok();
        statuses.push(outcome.certificate.status.value);
        assert_eq!(
            outcome.certificate.status.reasons[0].reason,
            ReasonCode::TrustClassHeuristicCapsAtLikelyValid
        );
    }
    assert!(
        statuses.iter().all(|s| *s == ValidityStatus::LikelyValid),
        "the certificate status oscillated under a flapping probe: {statuses:?}"
    );
}

/// Two consecutive higher-trust observations DO adopt the escalation in
/// the ledger — the protocol is not "never escalate" — while the
/// certificate keeps using the store's recorded class until a
/// persistence design lands.
#[test]
fn a_stable_escalation_is_adopted_in_the_ledger_but_not_on_the_certificate() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("escalate").with_probe_edge(
        "https",
        "acme.com/pricing",
        &fp,
        TrustClass::Heuristic,
        None,
    );
    let probe = Arc::new(
        ScriptedProbe::new("https").with_fallback(ProbeResult::Match {
            observed_fp: fp.parse().expect("fingerprint"),
            observed_trust_class: TrustClass::Versioned,
        }),
    );
    let mut registry = ProbeRegistry::new();
    registry
        .register(probe as Arc<dyn freshdag_core::probe::Probe>)
        .expect("register");
    let engine = fixture.engine(registry);

    let first = engine.check_ok();
    let second = engine.check_ok();
    for outcome in [&first, &second] {
        assert_eq!(
            outcome.certificate.status.value,
            ValidityStatus::LikelyValid,
            "the certificate never leaves the recorded heuristic class in v0"
        );
    }
    // ADR 0007 Amendment P1: the payload carries inputs only. The
    // recorded class is what the store observed and does not move; the
    // observed class is what the probe saw. The adopted class — a
    // *derived* value — never enters the log.
    let checked: Vec<&freshdag_core::ir::IrEvent> = [&first, &second]
        .iter()
        .flat_map(|o| o.events.iter())
        .filter(|e| e.kind == EventKind::ProbeChecked)
        .collect();
    let recorded: Vec<&str> = checked
        .iter()
        .map(|e| e.payload["trust_class"].as_str().expect("trust_class"))
        .collect();
    assert_eq!(
        recorded,
        vec!["heuristic", "heuristic"],
        "probe.checked.trust_class is the class the STORE recorded; it is \
         not the ledger's adopted class and does not move under escalation"
    );
    let observed: Vec<&str> = checked
        .iter()
        .map(|e| {
            e.payload["observed_trust_class"]
                .as_str()
                .expect("observed_trust_class")
        })
        .collect();
    assert_eq!(
        observed,
        vec!["versioned", "versioned"],
        "the fold's input is preserved, so TrustLedger::replay can \
         reconstruct the N=2 adoption from the log alone"
    );

    // The adoption is still auditable — as a diagnostic, which no fold
    // over `probe.checked` consumes.
    let escalation = [&first, &second]
        .iter()
        .flat_map(|o| o.events.iter())
        .find(|e| {
            e.kind == EventKind::Diagnostic && e.payload["message"] == "probe.trust_escalated"
        })
        .expect("the N=2 adoption surfaces as a diagnostic");
    assert_eq!(escalation.payload["from_trust_class"], "heuristic");
    assert_eq!(escalation.payload["to_trust_class"], "versioned");
}

/// The invariant-#5 half of ADR 0007 Amendment P1: feeding the engine's
/// own emitted events back into the log must be a no-op on the derived
/// graph, and must not raise any dependency's trust class.
///
/// While `probe.checked.trust_class` carried the ledger's *adopted*
/// class, the second emission said `versioned` where the store had
/// recorded `heuristic`. Replaying that log therefore disagreed with
/// itself: `DerivedGraph` keeps the first observation and records an
/// `EdgeConflict`, so the engine's own output poisoned the graph it was
/// derived from — an artifact whose dependency provably did not change
/// mid-computation reports as though it had. The fold was reading its
/// own output back, which is exactly the non-idempotence invariant #5
/// forbids.
#[test]
fn replaying_the_engines_own_events_is_a_no_op_on_the_graph() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let build = || {
        Fixture::new("replay-escalate").with_probe_edge(
            "https",
            "acme.com/pricing",
            &fp,
            TrustClass::Heuristic,
            None,
        )
    };
    let probes = || {
        let mut registry = ProbeRegistry::new();
        registry
            .register(Arc::new(ScriptedProbe::new("https").with_fallback(
                ProbeResult::Match {
                    observed_fp: fp.parse().expect("fingerprint"),
                    observed_trust_class: TrustClass::Versioned,
                },
            )))
            .expect("register");
        registry
    };

    let engine = build().engine(probes());
    let first = engine.check_ok();
    // Two checks, so the ledger has passed N=2 and adopted `versioned`.
    let second = engine.check_ok();
    let mut log: Vec<freshdag_core::ir::IrEvent> = first.events.clone();
    log.extend(second.events.clone());
    assert!(
        log.iter().any(|e| e.kind == EventKind::ProbeChecked),
        "the engine emitted no probe.checked, so this test proves nothing"
    );

    let replayed = build().with_extra_events(log).engine(probes());
    let outcome = replayed.check_ok();

    for dep in &outcome.certificate.depends_on {
        assert_eq!(
            dep.trust_class,
            TrustClass::Heuristic,
            "replaying the engine's own events raised `{}` to {:?}",
            dep.key,
            dep.trust_class
        );
    }
    assert!(
        !outcome
            .events
            .iter()
            .any(|e| e.payload.get("message") == Some(&"graph.edge_conflict".into())),
        "the engine's own events disagree with the store's record of the \
         same dependency, so replaying them fabricates an edge conflict"
    );
    assert_ne!(
        outcome.certificate.status.value,
        ValidityStatus::Valid,
        "a heuristic edge can never reach valid, however the log is replayed"
    );
}

/// Reason ordering is contractual: edge-scoped reasons in `depends_on[]`
/// order, artifact-scoped reasons after them.
#[test]
fn reasons_are_ordered_by_depends_on_position_then_artifact_scoped() {
    let first = format!("blake3:{}", blake3_of("a"));
    let second = format!("blake3:{}", blake3_of("b"));
    let fixture = Fixture::new("ordering")
        .with_file_edge("/repo/a.md", &first)
        .with_file_edge("/repo/b.md", &second)
        .with_bash_invocation();
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(ScriptedProbe::new("file").with_fallback(
            ProbeResult::Unknown {
                reason: "unreadable".to_string(),
                retryable: true,
            },
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    let reasons = &outcome.certificate.status.reasons;
    assert_eq!(reasons[0].dependency_key, "file:///repo/a.md");
    assert_eq!(reasons[1].dependency_key, "file:///repo/b.md");
    assert!(
        reasons[2].reason.is_artifact_scoped(),
        "artifact-scoped reasons sort after every edge-scoped one"
    );
    assert_eq!(reasons[2].dependency_key, "");
}

/// `cert_id` is content-addressed, so identical inputs must produce
/// identical bytes. `detail` sits inside the preimage; if anything in
/// this crate put a timestamp or a counter there, this test fails.
#[test]
fn identical_inputs_produce_an_identical_cert_id() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let build = || {
        let fixture = Fixture::new("repro").with_file_edge("/repo/notes.md", &fp);
        let mut registry = ProbeRegistry::new();
        registry
            .register(Arc::new(ScriptedProbe::new("file").with_fallback(
                ProbeResult::Unknown {
                    reason: "http-status=500".to_string(),
                    retryable: true,
                },
            )))
            .expect("register");
        fixture.engine(registry).check_ok().certificate
    };
    assert_eq!(build().cert_id, build().cert_id);
}

#[test]
fn an_unknown_artifact_is_an_error_not_an_empty_certificate() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("unknown-artifact").with_file_edge("/repo/notes.md", &fp);
    let engine = fixture.engine(ProbeRegistry::new());
    let err = engine
        .engine
        .check(&freshdag_core::artifact::ArtifactId(
            "blake3:nope".to_string(),
        ))
        .expect_err("unknown artifact");
    assert!(matches!(err, EngineError::UnknownArtifact { .. }));
}

/// A real `Probe` implementation, arbitrated through the registry and
/// dispatched by the engine. The engine depends on no probe crate; this
/// test is the only place `freshdag-probes` appears, as a dev-dependency.
#[test]
fn a_real_file_probe_matches_through_the_registry() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let hash = freshdag_probes::FileProbe::hash_file(&path).expect("hash Cargo.toml");
    let fingerprint = hash.to_string();

    let fixture = Fixture::new("real-file").with_file_edge(&path.to_string_lossy(), &fingerprint);
    let mut registry = ProbeRegistry::new();
    registry
        .register(Arc::new(freshdag_probes::FileProbe::new(
            path.parent().expect("parent"),
        )))
        .expect("register");
    let outcome = fixture.engine(registry).check_ok();

    assert_eq!(outcome.certificate.status.value, ValidityStatus::Valid);
    assert_eq!(
        outcome.certificate.depends_on[0].trust_class,
        TrustClass::Exact
    );
}

/// Sanity: the identity used for anti-thrash keying is the arbitration
/// triple, so it is stable across checks.
#[test]
fn probe_identity_is_derived_from_the_arbitration_triple() {
    assert_eq!(ProbeIdentity::derive("file", None, 0).as_str(), "file#0");
    assert_eq!(
        ProbeIdentity::derive("https", Some("*.github.com"), 10).as_str(),
        "https@*.github.com#10"
    );
}

#[test]
fn the_engine_declares_itself_in_observation_coverage() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("self-coverage").with_file_edge("/repo/notes.md", &fp);
    let outcome = fixture.engine(ProbeRegistry::new()).check_ok();
    let entry = outcome
        .certificate
        .observation_coverage
        .iter()
        .find(|c| c.producer == crate::ENGINE_PRODUCER)
        .expect("engine declares its own coverage");
    assert_eq!(
        entry.role,
        ProducerRole::Probe,
        "a probe-role producer can never discharge a bash/task obligation"
    );
}

/// Guard the fingerprint helper the tests rely on.
#[test]
fn fingerprints_round_trip_through_the_wire_form() {
    let fp: Fingerprint = "version:42".parse().expect("parse");
    assert_eq!(fp.kind, FingerprintKind::Version);
    assert_eq!(fp.to_string(), "version:42");
}

/// The clock is injected, so `status.checked` is whatever the test says.
#[test]
fn status_checked_comes_from_the_injected_clock() {
    let fp = format!("blake3:{}", blake3_of("notes"));
    let fixture = Fixture::new("clock").with_file_edge("/repo/notes.md", &fp);
    let engine = fixture.engine(ProbeRegistry::new());
    let expected = engine.clock.now();
    let outcome = engine.check_ok();
    assert_eq!(outcome.certificate.status.checked, expected);
}

fn _assert_engine_is_send_sync() {
    fn require<T: Send + Sync>() {}
    require::<Engine>();
    require::<FixedClock>();
    let _: fn() = || {
        let _ = <FixedClock as Clock>::now;
    };
}
