//! Tests for the `https://` probe.
//!
//! # What this file is for
//!
//! Invariant #7 says an unverifiable dependency is `Unknown`, never
//! fresh. At the probe boundary that reduces to one sentence: **no
//! error path may produce [`ProbeResult::Match`]**. Every failure mode
//! this probe can encounter therefore gets its own test here, and every
//! one of those tests makes *two* assertions:
//!
//! 1. the result is `Unknown` with the expected reason and
//!    `retryable`, and
//! 2. the result is explicitly `!matches!(.., Match)`.
//!
//! The second assertion is redundant only as long as `ProbeResult` has
//! exactly three variants. It is written out anyway, because the thing
//! being defended is "this never becomes a `Match`", and that is what it
//! should say.
//!
//! # Why there is no network here
//!
//! Every test in this file drives
//! [`ScriptedTransport`](super::transport::ScriptedTransport), which is
//! an in-memory `VecDeque` of pre-decided turns. No socket is opened, no
//! name is resolved, nothing is timed. Several of the cases below — DNS
//! failure, TLS failure, an `https:` origin redirecting to `http:` —
//! cannot be produced by a loopback server at all, which is the reason
//! the transport seam exists. `tests/https_loopback.rs` covers the real
//! `reqwest` wiring separately, also without leaving `127.0.0.1`.

use std::sync::Arc;
use std::time::Duration;

use freshdag_core::dependency::{Fingerprint, FingerprintKind, TrustClass};
use freshdag_core::ir::ProducerRole;
use freshdag_core::probe::{Probe, ProbeResult};

use super::headers::Headers;
use super::report::{
    DiagnosticCode, HttpsCheckOutcome, ProbeCheckedPayload, ProbeResultKind, RetryAfterHint,
};
use super::transport::{
    HttpMethod, HttpResponse, RepeatingBody, ScriptedTransport, ScriptedTurn, TransportError,
};
use super::{ContentHashFallback, HttpsProbe, HttpsProbeConfig};

// --------------------------------------------------------------------
// Fixtures and helpers
// --------------------------------------------------------------------

const URL: &str = "https://example.test/pricing";
const RECORDED_ETAG: &str = "\"v1\"";
const RECORDED_LAST_MODIFIED: &str = "Sun, 06 Nov 1994 08:49:37 GMT";
const NEWER_LAST_MODIFIED: &str = "Mon, 07 Nov 1994 08:49:37 GMT";

fn etag_fp(wire: &str) -> Fingerprint {
    Fingerprint::new(FingerprintKind::Etag, wire)
}

fn mtime_fp(date: &str) -> Fingerprint {
    Fingerprint::new(FingerprintKind::Mtime, date)
}

fn content_hash_fp(payload: &str) -> Fingerprint {
    Fingerprint::new(FingerprintKind::ContentHash, payload)
}

fn blake3_of(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn headers(pairs: &[(&str, &str)]) -> Headers {
    pairs.iter().copied().collect()
}

fn respond(status: u16, pairs: &[(&str, &str)]) -> ScriptedTurn {
    ScriptedTurn::Respond(HttpResponse::new(status, headers(pairs)))
}

fn redirect_to(location: &str) -> ScriptedTurn {
    respond(302, &[("Location", location)])
}

/// Build a probe over a scripted transport, returning the transport too
/// so tests can assert on the requests actually issued.
fn scripted(
    config: HttpsProbeConfig,
    turns: Vec<ScriptedTurn>,
) -> (HttpsProbe, Arc<ScriptedTransport>) {
    let transport = Arc::new(ScriptedTransport::new(turns));
    let probe = HttpsProbe::with_transport(transport.clone(), config);
    (probe, transport)
}

/// Default-configured probe over a scripted transport.
fn probe(turns: Vec<ScriptedTurn>) -> HttpsProbe {
    scripted(HttpsProbeConfig::default(), turns).0
}

/// Assert a failure path produced `Unknown` and, separately, that it did
/// not produce `Match`. Returns the reason so callers can assert on its
/// exact bytes.
#[track_caller]
fn expect_unknown(
    outcome: &HttpsCheckOutcome,
    expect_retryable: bool,
    reason_needle: &str,
) -> String {
    assert!(
        !matches!(outcome.result, ProbeResult::Match { .. }),
        "invariant #7: this path MUST NOT yield Match; got {:?}",
        outcome.result
    );
    match &outcome.result {
        ProbeResult::Unknown { reason, retryable } => {
            assert!(
                reason.contains(reason_needle),
                "reason should contain {reason_needle:?}; got {reason:?}"
            );
            assert_eq!(
                *retryable, expect_retryable,
                "retryable mismatch for reason {reason:?}"
            );
            reason.clone()
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[track_caller]
fn expect_drift(outcome: &HttpsCheckOutcome, expect_class: TrustClass) -> Fingerprint {
    match &outcome.result {
        ProbeResult::Drift {
            observed_fp,
            observed_trust_class,
        } => {
            assert_eq!(*observed_trust_class, expect_class);
            observed_fp.clone()
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[track_caller]
fn expect_match(outcome: &HttpsCheckOutcome, expect_class: TrustClass) -> Fingerprint {
    match &outcome.result {
        ProbeResult::Match {
            observed_fp,
            observed_trust_class,
        } => {
            assert_eq!(*observed_trust_class, expect_class);
            observed_fp.clone()
        }
        other => panic!("expected Match, got {other:?}"),
    }
}

// ====================================================================
// FAILURE-MODE MATRIX
//
// One test per mode. Each asserts Unknown, asserts !Match, and pins
// the `retryable` classification (which the engine's anti-thrash
// policy reads: `retryable: false` forces re-observation, `true`
// schedules a retry, so getting it backwards is not cosmetic).
// ====================================================================

#[test]
fn http_5xx_returns_unknown_retryable() {
    let outcome = probe(vec![respond(503, &[])]).check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    let reason = expect_unknown(&outcome, true, "http-status=503");
    assert_eq!(reason, "http-status=503");
}

#[test]
fn http_4xx_other_than_429_returns_unknown_not_retryable() {
    // A 404 means the dependency is gone, not that it is unchanged.
    // Retrying will not fix it; the engine must force re-observation.
    let outcome = probe(vec![respond(404, &[])]).check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    let reason = expect_unknown(&outcome, false, "http-status=404");
    assert_eq!(reason, "http-status=404");
}

#[test]
fn http_429_with_retry_after_returns_unknown_retryable() {
    let outcome = probe(vec![respond(429, &[("Retry-After", "120")])]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );
    expect_unknown(&outcome, true, "rate-limited");
    assert!(outcome.has_diagnostic(DiagnosticCode::RateLimited));
    assert_eq!(
        outcome.cost.retry_after,
        Some(RetryAfterHint::Seconds(120)),
        "the server-provided backoff must survive onto the cost report"
    );
}

#[test]
fn http_429_with_http_date_retry_after_is_surfaced_verbatim() {
    let outcome = probe(vec![respond(
        429,
        &[("Retry-After", RECORDED_LAST_MODIFIED)],
    )])
    .check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    expect_unknown(&outcome, true, "rate-limited");
    assert_eq!(
        outcome.cost.retry_after,
        Some(RetryAfterHint::HttpDate(RECORDED_LAST_MODIFIED.to_string())),
        "an HTTP-date Retry-After is not converted; that needs a clock the probe does not own"
    );
}

#[test]
fn transport_timeout_returns_unknown_retryable() {
    let outcome = probe(vec![ScriptedTurn::Fail(TransportError::Timeout)]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );
    let reason = expect_unknown(&outcome, true, "transport-timeout");
    assert_eq!(reason, "transport-timeout");
}

#[test]
fn transport_dns_failure_returns_unknown_retryable() {
    let outcome = probe(vec![ScriptedTurn::Fail(TransportError::Dns)]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );
    let reason = expect_unknown(&outcome, true, "transport-dns-failed");
    assert_eq!(reason, "transport-dns-failed");
}

#[test]
fn transport_connect_failure_returns_unknown_retryable() {
    let outcome = probe(vec![ScriptedTurn::Fail(TransportError::Connect)]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );
    let reason = expect_unknown(&outcome, true, "transport-connect-failed");
    assert_eq!(reason, "transport-connect-failed");
}

#[test]
fn transport_tls_failure_returns_unknown_not_retryable() {
    // A TLS failure is a misconfiguration (expired cert, wrong SAN,
    // untrusted chain), not a transient. Retrying spins; the engine
    // needs to be told to stop trusting this edge.
    let outcome = probe(vec![ScriptedTurn::Fail(TransportError::Tls)]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );
    let reason = expect_unknown(&outcome, false, "transport-tls-failed");
    assert_eq!(reason, "transport-tls-failed");
}

#[test]
fn http_401_auth_required_returns_unknown_not_retryable() {
    let outcome = probe(vec![respond(401, &[("WWW-Authenticate", "Bearer")])]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );
    let reason = expect_unknown(&outcome, false, "auth-required");
    assert_eq!(reason, "auth-required http-status=401");
}

#[test]
fn http_403_auth_required_returns_unknown_not_retryable() {
    let outcome = probe(vec![respond(403, &[])]).check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    let reason = expect_unknown(&outcome, false, "auth-required");
    assert_eq!(reason, "auth-required http-status=403");
}

#[test]
fn redirect_chain_deeper_than_the_hop_budget_returns_unknown() {
    // Budget is 5, so the sixth 3xx is refused. A chain this deep is
    // indistinguishable from a redirect loop.
    let turns: Vec<ScriptedTurn> = (0..6)
        .map(|i| redirect_to(&format!("https://example.test/hop{i}")))
        .collect();
    let (probe, transport) = scripted(HttpsProbeConfig::default(), turns);
    let outcome = probe.check_detailed(URL, &etag_fp(RECORDED_ETAG), None);

    expect_unknown(&outcome, false, "redirect-chain-too-deep");
    assert_eq!(
        transport.request_count(),
        6,
        "the probe stops at the budget rather than following forever"
    );
    assert_eq!(outcome.cost.redirect_hops, 5);
}

#[test]
fn https_to_http_redirect_downgrade_is_refused_under_every_config() {
    // The refusal is unconditional. `allow_plaintext_http` exists so a
    // loopback fixture can be addressed directly; it must NOT license a
    // remote origin to walk the probe down off TLS, because a freshness
    // signal read over a downgraded connection is evidence of nothing.
    for allow_plaintext_http in [false, true] {
        let config = HttpsProbeConfig {
            allow_plaintext_http,
            ..HttpsProbeConfig::default()
        };
        let (probe, transport) = scripted(
            config,
            vec![
                redirect_to("http://example.test/pricing"),
                // Would be served if the downgrade were followed.
                respond(200, &[("ETag", RECORDED_ETAG)]),
            ],
        );
        let outcome = probe.check_detailed(URL, &etag_fp(RECORDED_ETAG), None);

        expect_unknown(&outcome, false, "cross-scheme-downgrade-refused");
        assert_eq!(
            transport.request_count(),
            1,
            "allow_plaintext_http={allow_plaintext_http}: the downgraded hop must never be issued"
        );
    }
}

#[test]
fn body_exceeding_max_fetch_bytes_returns_unknown() {
    let config = HttpsProbeConfig {
        max_fetch_bytes: 1024,
        content_hash_fallback: ContentHashFallback::for_keys([URL]),
        ..HttpsProbeConfig::default()
    };
    let oversized = ScriptedTurn::Respond(
        HttpResponse::new(200, Headers::new()).with_body(RepeatingBody::new(b'x', 8192)),
    );
    let (probe, _) = scripted(config, vec![oversized]);
    let outcome = probe.check_detailed(URL, &content_hash_fp(&blake3_of(b"whatever")), None);

    let reason = expect_unknown(&outcome, false, "body-exceeds-max-fetch-bytes");
    assert_eq!(reason, "body-exceeds-max-fetch-bytes max-fetch-bytes=1024");
    assert!(
        outcome.cost.body_bytes > 1024,
        "the cost report must show what the aborted fetch actually cost"
    );
}

#[test]
fn version_signal_lost_returns_unknown_not_match() {
    // Recorded a strong ETag; the endpoint now serves the resource with
    // no validator at all. The memo (§Version-lost transitions) forbids
    // silently falling back to a weaker signal.
    let outcome = probe(vec![respond(200, &[("Content-Type", "text/html")])]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );

    expect_unknown(&outcome, false, "version-signal-lost");
    assert!(
        outcome.has_diagnostic(DiagnosticCode::TrustDemoted),
        "demotion is explicit, never silent (probe-contract §Anti-thrash Protocol)"
    );
}

#[test]
fn version_signal_lost_for_last_modified_returns_unknown() {
    let outcome =
        probe(vec![respond(200, &[])]).check_detailed(URL, &mtime_fp(RECORDED_LAST_MODIFIED), None);
    expect_unknown(&outcome, false, "version-signal-lost");
    assert!(outcome.has_diagnostic(DiagnosticCode::TrustDemoted));
}

#[test]
fn version_signal_lost_for_immutable_returns_unknown() {
    let outcome = probe(vec![respond(200, &[("Cache-Control", "max-age=60")])]).check_detailed(
        URL,
        &Fingerprint::new(FingerprintKind::Custom, "immutable"),
        None,
    );
    expect_unknown(&outcome, false, "version-signal-lost");
    assert!(outcome.has_diagnostic(DiagnosticCode::TrustDemoted));
}

#[test]
fn malformed_recorded_etag_fingerprint_returns_unknown_before_any_request() {
    // An unquoted recorded tag cannot be echoed in `If-None-Match`
    // without inventing quotes, and inventing them is how a fabricated
    // `versioned` claim gets made.
    let (probe, transport) = scripted(
        HttpsProbeConfig::default(),
        vec![respond(304, &[])], // never reached
    );
    let outcome = probe.check_detailed(URL, &etag_fp("v1"), None);

    let reason = expect_unknown(&outcome, false, "recorded-fingerprint-malformed");
    assert_eq!(reason, "recorded-fingerprint-malformed etag=unquoted");
    assert_eq!(
        transport.request_count(),
        0,
        "a malformed recorded fingerprint is refused without touching the network"
    );
}

#[test]
fn malformed_recorded_mtime_fingerprint_returns_unknown() {
    let outcome = probe(vec![]).check_detailed(URL, &mtime_fp("1994-11-06T08:49:37Z"), None);
    let reason = expect_unknown(&outcome, false, "recorded-fingerprint-malformed");
    assert_eq!(
        reason,
        "recorded-fingerprint-malformed mtime=not-imf-fixdate"
    );
}

#[test]
fn malformed_recorded_content_hash_fingerprint_returns_unknown() {
    let config = HttpsProbeConfig {
        content_hash_fallback: ContentHashFallback::Always,
        ..HttpsProbeConfig::default()
    };
    let (probe, _) = scripted(config, vec![]);
    let outcome = probe.check_detailed(URL, &content_hash_fp("blake3:not-hex"), None);
    let reason = expect_unknown(&outcome, false, "recorded-fingerprint-malformed");
    assert_eq!(reason, "recorded-fingerprint-malformed content-hash");
}

#[test]
fn unsupported_recorded_fingerprint_kind_returns_unknown() {
    let fp = Fingerprint::new(FingerprintKind::Version, "42");
    let outcome = probe(vec![]).check_detailed(URL, &fp, None);
    expect_unknown(&outcome, false, "unsupported-fingerprint-kind=version");
}

#[test]
fn malformed_wire_etag_is_ignored_and_yields_unknown_not_match() {
    // Apache and several CDNs have shipped bare, unquoted ETags. The
    // probe does not repair them: a repaired tag would be compared as if
    // the server had spoken RFC 9110, which it did not.
    let outcome = probe(vec![respond(200, &[("ETag", "v1")])]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );

    expect_unknown(&outcome, false, "version-signal-lost");
    assert!(outcome.has_diagnostic(DiagnosticCode::MalformedEtag));
    assert!(outcome.has_diagnostic(DiagnosticCode::TrustDemoted));
}

#[test]
fn conflicting_duplicate_wire_etags_return_unknown_not_match() {
    // RFC 9110 §8.8.3 allows one `ETag` field. Two that disagree means
    // the response has no single validator. Note the FIRST field here is
    // byte-equal to the recorded one: a probe that took `first()` would
    // report `Match` out of an ambiguity. That is the exact shape of an
    // invariant-#7 hole, so it gets its own test.
    let outcome = probe(vec![respond(
        200,
        &[("ETag", RECORDED_ETAG), ("ETag", "\"v2\"")],
    )])
    .check_detailed(URL, &etag_fp(RECORDED_ETAG), None);

    expect_unknown(&outcome, false, "ambiguous-etag");
    assert!(outcome.has_diagnostic(DiagnosticCode::DuplicateEtag));
}

#[test]
fn agreeing_duplicate_wire_etags_are_diagnosed_but_not_fatal() {
    // Sloppy, not ambiguous: both fields say the same thing, so there is
    // still exactly one validator. Refusing here would be pessimism
    // without a correctness argument.
    let outcome = probe(vec![respond(
        200,
        &[("ETag", "\"v2\""), ("ETag", "\"v2\"")],
    )])
    .check_detailed(URL, &etag_fp(RECORDED_ETAG), None);

    let observed = expect_drift(&outcome, TrustClass::Versioned);
    assert_eq!(observed.payload, "\"v2\"");
    assert!(outcome.has_diagnostic(DiagnosticCode::DuplicateEtag));
}

#[test]
fn redirect_without_location_returns_unknown() {
    let outcome = probe(vec![respond(302, &[])]).check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    expect_unknown(&outcome, false, "redirect-without-location");
}

#[test]
fn redirect_to_unsupported_scheme_returns_unknown() {
    let outcome = probe(vec![redirect_to("ftp://example.test/pricing")]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );
    expect_unknown(&outcome, false, "redirect-unsupported-scheme");
}

#[test]
fn plaintext_http_dependency_key_is_refused_by_default() {
    let (probe, transport) = scripted(HttpsProbeConfig::default(), vec![respond(304, &[])]);
    let outcome =
        probe.check_detailed("http://example.test/pricing", &etag_fp(RECORDED_ETAG), None);
    expect_unknown(&outcome, false, "plaintext-http-refused");
    assert_eq!(transport.request_count(), 0);
}

#[test]
fn non_http_dependency_key_returns_unknown() {
    let outcome =
        probe(vec![]).check_detailed("ftp://example.test/x", &etag_fp(RECORDED_ETAG), None);
    expect_unknown(&outcome, false, "unsupported-url-scheme=ftp");
}

#[test]
fn unparseable_dependency_key_returns_unknown() {
    let outcome = probe(vec![]).check_detailed("not a url", &etag_fp(RECORDED_ETAG), None);
    expect_unknown(&outcome, false, "malformed-dependency-key");
}

#[test]
fn exact_dependency_without_content_hash_fallback_returns_unknown_never_drift() {
    // probe-contract §Trust-class Semantics: "If the endpoint does not
    // support cheap content hashing, the probe returns
    // Unknown { retryable: true }, not a bare Drift."
    let (probe, transport) = scripted(HttpsProbeConfig::default(), vec![respond(200, &[])]);
    let outcome = probe.check_detailed(URL, &content_hash_fp(&blake3_of(b"body")), None);

    assert!(
        !matches!(outcome.result, ProbeResult::Drift { .. }),
        "an un-hashable `exact` dependency must not be reported as drifted; got {:?}",
        outcome.result
    );
    expect_unknown(&outcome, true, "content-hash-fallback-required");
    assert_eq!(transport.request_count(), 0);
}

#[test]
fn content_hash_fallback_with_no_body_returns_unknown() {
    let config = HttpsProbeConfig {
        content_hash_fallback: ContentHashFallback::Always,
        ..HttpsProbeConfig::default()
    };
    let (probe, _) = scripted(config, vec![respond(200, &[])]);
    let outcome = probe.check_detailed(URL, &content_hash_fp(&blake3_of(b"body")), None);
    expect_unknown(&outcome, false, "content-hash-fallback-no-body");
}

#[test]
fn running_past_the_end_of_a_response_sequence_returns_unknown() {
    // Defensive: a transport that yields nothing must not be readable as
    // "nothing changed".
    let outcome = probe(vec![]).check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    expect_unknown(&outcome, false, "transport-invalid-request");
}

/// The matrix, restated as one table.
///
/// Individual tests above pin reasons and `retryable` flags; this one
/// exists so that adding a new failure path without a `Match` guard is
/// caught by a test whose name says exactly what broke.
#[test]
fn no_failure_path_in_the_matrix_ever_yields_match() {
    type Case = (&'static str, Vec<ScriptedTurn>, Fingerprint);

    let cases: Vec<Case> = vec![
        ("5xx", vec![respond(500, &[])], etag_fp(RECORDED_ETAG)),
        ("4xx", vec![respond(404, &[])], etag_fp(RECORDED_ETAG)),
        (
            "429",
            vec![respond(429, &[("Retry-After", "1")])],
            etag_fp(RECORDED_ETAG),
        ),
        (
            "timeout",
            vec![ScriptedTurn::Fail(TransportError::Timeout)],
            etag_fp(RECORDED_ETAG),
        ),
        (
            "dns",
            vec![ScriptedTurn::Fail(TransportError::Dns)],
            etag_fp(RECORDED_ETAG),
        ),
        (
            "connect",
            vec![ScriptedTurn::Fail(TransportError::Connect)],
            etag_fp(RECORDED_ETAG),
        ),
        (
            "tls",
            vec![ScriptedTurn::Fail(TransportError::Tls)],
            etag_fp(RECORDED_ETAG),
        ),
        (
            "body-read",
            vec![ScriptedTurn::Fail(TransportError::Body)],
            etag_fp(RECORDED_ETAG),
        ),
        ("401", vec![respond(401, &[])], etag_fp(RECORDED_ETAG)),
        ("403", vec![respond(403, &[])], etag_fp(RECORDED_ETAG)),
        (
            "downgrade",
            vec![redirect_to("http://example.test/pricing")],
            etag_fp(RECORDED_ETAG),
        ),
        (
            "no-location",
            vec![respond(302, &[])],
            etag_fp(RECORDED_ETAG),
        ),
        (
            "version-signal-lost",
            vec![respond(200, &[])],
            etag_fp(RECORDED_ETAG),
        ),
        (
            "malformed-wire-etag",
            vec![respond(200, &[("ETag", "unquoted")])],
            etag_fp(RECORDED_ETAG),
        ),
        (
            "ambiguous-etag",
            vec![respond(
                200,
                &[("ETag", RECORDED_ETAG), ("ETag", "\"other\"")],
            )],
            etag_fp(RECORDED_ETAG),
        ),
        ("malformed-recorded-fp", vec![], etag_fp("bare")),
        ("empty-script", vec![], etag_fp(RECORDED_ETAG)),
        (
            "1xx-informational",
            vec![respond(199, &[])],
            etag_fp(RECORDED_ETAG),
        ),
    ];

    for (name, turns, fp) in cases {
        let outcome = probe(turns).check_detailed(URL, &fp, None);
        assert!(
            !matches!(outcome.result, ProbeResult::Match { .. }),
            "invariant #7 violated by case `{name}`: {:?}",
            outcome.result
        );
        assert!(
            matches!(outcome.result, ProbeResult::Unknown { .. }),
            "case `{name}` should be Unknown; got {:?}",
            outcome.result
        );
    }
}

// ====================================================================
// DETERMINISM
//
// `Unknown::reason` becomes the certificate reason's `detail`, and
// `detail` sits inside the `cert_id` preimage. A reason that varies
// between runs makes certificates unreproducible without anything
// looking wrong.
// ====================================================================

#[test]
fn timeout_reason_is_byte_identical_across_runs() {
    let run = || {
        let outcome = probe(vec![ScriptedTurn::Fail(TransportError::Timeout)]).check_detailed(
            URL,
            &etag_fp(RECORDED_ETAG),
            None,
        );
        match outcome.result {
            ProbeResult::Unknown { reason, .. } => reason,
            other => panic!("expected Unknown, got {other:?}"),
        }
    };
    let first = run();
    let second = run();
    assert_eq!(
        first.as_bytes(),
        second.as_bytes(),
        "reason must be byte-identical across runs; it is in the cert_id preimage"
    );
    assert_eq!(first, "transport-timeout");
}

#[test]
fn rate_limited_reason_is_byte_identical_across_runs() {
    let run = || {
        let outcome = probe(vec![respond(429, &[("Retry-After", "30")])]).check_detailed(
            URL,
            &etag_fp(RECORDED_ETAG),
            None,
        );
        match outcome.result {
            ProbeResult::Unknown { reason, .. } => reason,
            other => panic!("expected Unknown, got {other:?}"),
        }
    };
    let first = run();
    let second = run();
    assert_eq!(first.as_bytes(), second.as_bytes());
    assert_eq!(first, "rate-limited http-status=429");
    assert!(
        !first.contains("Retry-After"),
        "the backoff hint is scheduling state, not certificate detail"
    );
}

#[test]
fn reason_never_leaks_the_dependency_url_or_its_query_string() {
    // Dependency URLs routinely carry API tokens in the query string,
    // and reasons land on shareable certificates.
    let key = "https://example.test/pricing?api_key=SUPERSECRET";
    let outcome = probe(vec![respond(500, &[])]).check_detailed(key, &etag_fp(RECORDED_ETAG), None);
    let reason = expect_unknown(&outcome, true, "http-status=500");
    assert!(
        !reason.contains("SUPERSECRET"),
        "reason leaked a token: {reason}"
    );
    assert!(
        !reason.contains("example.test"),
        "reason leaked a host: {reason}"
    );
}

// ====================================================================
// `probe.checked` PAYLOAD WELL-FORMEDNESS
//
// `retryable` is required when the result is `unknown` and forbidden
// otherwise. The engine reconstructs its retry / anti-thrash state from
// the append-only log (invariant #5), so a `retryable` that goes missing
// there is state the engine cannot recover.
// ====================================================================

#[test]
fn probe_checked_retryable_is_present_iff_result_is_unknown() {
    let results = vec![
        // Unknown
        probe(vec![respond(503, &[])])
            .check_detailed(URL, &etag_fp(RECORDED_ETAG), None)
            .result,
        probe(vec![respond(404, &[])])
            .check_detailed(URL, &etag_fp(RECORDED_ETAG), None)
            .result,
        // Match
        probe(vec![respond(304, &[])])
            .check_detailed(URL, &etag_fp(RECORDED_ETAG), None)
            .result,
        // Drift
        probe(vec![respond(200, &[("ETag", "\"v2\"")])])
            .check_detailed(URL, &etag_fp(RECORDED_ETAG), None)
            .result,
    ];

    let mut saw_unknown = false;
    let mut saw_decided = false;
    for result in &results {
        let payload = ProbeCheckedPayload::from_result("https", URL, result);
        assert!(
            payload.retryable_field_is_wellformed(),
            "retryable presence is wrong for {result:?}"
        );
        match payload.result {
            ProbeResultKind::Unknown => {
                saw_unknown = true;
                assert!(payload.retryable.is_some());
                assert!(payload.observed_fingerprint.is_none());
                assert!(payload.trust_class.is_none());
            }
            ProbeResultKind::Match | ProbeResultKind::Drift => {
                saw_decided = true;
                assert!(payload.retryable.is_none());
                assert!(payload.observed_fingerprint.is_some());
                assert!(payload.trust_class.is_some());
            }
        }
        // And the same thing again through the wire form, since that is
        // what actually lands in the log.
        let json = payload.to_json_value().expect("payload serializes");
        let has_retryable = json.get("retryable").is_some();
        assert_eq!(
            has_retryable,
            payload.result == ProbeResultKind::Unknown,
            "wire form disagrees with the in-memory payload for {result:?}"
        );
    }
    assert!(
        saw_unknown && saw_decided,
        "the fixture set must cover both"
    );
}

#[test]
fn probe_checked_key_is_the_original_url_not_the_redirect_target() {
    let (probe, _) = scripted(
        HttpsProbeConfig::default(),
        vec![
            redirect_to("https://cdn.example.test/pricing"),
            respond(200, &[("ETag", "\"v2\"")]),
        ],
    );
    let outcome = probe.check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    let payload = ProbeCheckedPayload::from_result("https", URL, &outcome.result);

    assert_eq!(payload.key, URL);
    assert_eq!(
        outcome.redirect_chain.first().map(String::as_str),
        Some(URL)
    );
    assert_eq!(outcome.redirect_chain.len(), 2);
    assert!(outcome.has_diagnostic(DiagnosticCode::CrossOriginRedirect));
}

// ====================================================================
// TRUST-CLASS MAPPING
//
// The decision matrix in `docs/research/http-validity-probes.md`,
// asserted line by line. Escalation is permitted; demotion must be
// announced; weak equivalence must never be laundered into `versioned`.
// ====================================================================

#[test]
fn strong_etag_maps_to_versioned() {
    let outcome = probe(vec![respond(200, &[("ETag", "\"v2\"")])]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );
    let observed = expect_drift(&outcome, TrustClass::Versioned);
    assert_eq!(observed.kind, FingerprintKind::Etag);
    assert_eq!(observed.payload, "\"v2\"");
}

#[test]
fn weak_etag_maps_to_heuristic_and_never_versioned() {
    // RFC 9110 §8.8.3.2: weak equivalence is entity-level, not
    // octet-level. FreshDAG's `exact` comparators are octet-level, so
    // `W/"..."` cannot justify `versioned` no matter how convenient.
    let outcome = probe(vec![respond(200, &[("ETag", "W/\"v2\"")])]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );
    let observed = expect_drift(&outcome, TrustClass::Heuristic);
    assert_eq!(observed.payload, "W/\"v2\"");

    match outcome.result {
        ProbeResult::Drift {
            observed_trust_class,
            ..
        } => assert_ne!(
            observed_trust_class,
            TrustClass::Versioned,
            "a weak ETag must never be laundered into `versioned` (invariant #8)"
        ),
        other => panic!("expected Drift, got {other:?}"),
    }
    assert!(
        outcome.has_diagnostic(DiagnosticCode::TrustDemoted),
        "versioned -> heuristic is a demotion and must be announced"
    );
}

#[test]
fn weak_etag_match_stays_heuristic() {
    let outcome = probe(vec![respond(304, &[])]).check_detailed(URL, &etag_fp("W/\"v1\""), None);
    let observed = expect_match(&outcome, TrustClass::Heuristic);
    assert_eq!(observed.payload, "W/\"v1\"");
}

#[test]
fn last_modified_only_maps_to_heuristic() {
    let outcome = probe(vec![respond(
        200,
        &[("Last-Modified", NEWER_LAST_MODIFIED)],
    )])
    .check_detailed(URL, &mtime_fp(RECORDED_LAST_MODIFIED), None);
    let observed = expect_drift(&outcome, TrustClass::Heuristic);
    assert_eq!(observed.kind, FingerprintKind::Mtime);
    assert_eq!(observed.payload, NEWER_LAST_MODIFIED);
}

#[test]
fn cache_control_immutable_maps_to_versioned() {
    let outcome = probe(vec![respond(
        200,
        &[("Cache-Control", "public, max-age=31536000, immutable")],
    )])
    .check_detailed(
        URL,
        &Fingerprint::new(FingerprintKind::Custom, "immutable"),
        None,
    );
    let observed = expect_match(&outcome, TrustClass::Versioned);
    assert_eq!(observed.payload, "immutable");
}

#[test]
fn cache_control_no_store_maps_to_volatile() {
    let outcome = probe(vec![respond(
        200,
        &[("ETag", "\"v2\""), ("Cache-Control", "no-store")],
    )])
    .check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    expect_drift(&outcome, TrustClass::Volatile);
    assert!(outcome.has_diagnostic(DiagnosticCode::TrustDemoted));
}

#[test]
fn cache_control_must_revalidate_maps_to_volatile() {
    let outcome = probe(vec![respond(
        200,
        &[("ETag", "\"v2\""), ("Cache-Control", "must-revalidate")],
    )])
    .check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    expect_drift(&outcome, TrustClass::Volatile);
}

#[test]
fn no_store_beats_immutable_pessimistically() {
    // Contradictory directives. `immutable` is an optimization claim;
    // `no-store` is a prohibition. Resolving toward the prohibition can
    // only cost a recheck, while resolving the other way can report a
    // changed resource as fresh.
    let outcome = probe(vec![respond(
        200,
        &[("Cache-Control", "immutable, no-store")],
    )])
    .check_detailed(
        URL,
        &Fingerprint::new(FingerprintKind::Custom, "immutable"),
        None,
    );

    expect_unknown(&outcome, false, "version-signal-lost immutable");
    assert!(outcome.has_diagnostic(DiagnosticCode::ContradictoryCacheControl));
}

#[test]
fn escalation_from_last_modified_to_etag_is_drift_not_match() {
    // The endpoint gained a strong ETag. That is a trust escalation the
    // engine may adopt after N=2 stable observations, but this check
    // cannot prove equality across validator kinds, and a `200` to a
    // conditional request is the server saying "changed."
    let outcome = probe(vec![respond(200, &[("ETag", "\"v9\"")])]).check_detailed(
        URL,
        &mtime_fp(RECORDED_LAST_MODIFIED),
        None,
    );
    let observed = expect_drift(&outcome, TrustClass::Versioned);
    assert_eq!(observed.kind, FingerprintKind::Etag);
}

// ====================================================================
// DECIDED PATHS — the narrow set of ways a `Match` may be produced
// ====================================================================

#[test]
fn not_modified_304_returns_match_preserving_the_recorded_fingerprint() {
    let (probe, transport) = scripted(HttpsProbeConfig::default(), vec![respond(304, &[])]);
    let outcome = probe.check_detailed(URL, &etag_fp(RECORDED_ETAG), None);

    let observed = expect_match(&outcome, TrustClass::Versioned);
    assert_eq!(observed.payload, RECORDED_ETAG);

    let sent = transport.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, HttpMethod::Head, "HEAD is O(headers)");
    assert_eq!(sent[0].headers.get("If-None-Match"), Some(RECORDED_ETAG));
}

#[test]
fn if_modified_since_is_sent_for_a_recorded_last_modified() {
    let (probe, transport) = scripted(HttpsProbeConfig::default(), vec![respond(304, &[])]);
    let outcome = probe.check_detailed(URL, &mtime_fp(RECORDED_LAST_MODIFIED), None);

    expect_match(&outcome, TrustClass::Heuristic);
    let sent = transport.requests();
    assert_eq!(
        sent[0].headers.get("If-Modified-Since"),
        Some(RECORDED_LAST_MODIFIED)
    );
    assert!(sent[0].headers.get("If-None-Match").is_none());
}

#[test]
fn head_405_falls_back_to_get_once() {
    let (probe, transport) = scripted(
        HttpsProbeConfig::default(),
        vec![respond(405, &[]), respond(304, &[])],
    );
    let outcome = probe.check_detailed(URL, &etag_fp(RECORDED_ETAG), None);

    expect_match(&outcome, TrustClass::Versioned);
    assert!(outcome.has_diagnostic(DiagnosticCode::HeadNotAllowed));
    let sent = transport.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].method, HttpMethod::Head);
    assert_eq!(sent[1].method, HttpMethod::Get);
    assert_eq!(outcome.cost.requests, 2);
}

#[test]
fn repeated_405_does_not_loop() {
    // The GET retry happens once. A server that 405s both must not spin.
    let (probe, transport) = scripted(
        HttpsProbeConfig::default(),
        vec![respond(405, &[]), respond(405, &[])],
    );
    let outcome = probe.check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    expect_unknown(&outcome, false, "http-status=405");
    assert_eq!(transport.request_count(), 2);
}

#[test]
fn etag_echoed_on_200_is_match_with_an_instability_diagnostic() {
    // We sent `If-None-Match: "v1"` and got `200` with `ETag: "v1"`.
    // The tag is unchanged, so this is a `Match` — but the endpoint is
    // ignoring conditional requests, which is worth saying out loud.
    let outcome = probe(vec![respond(200, &[("ETag", RECORDED_ETAG)])]).check_detailed(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );

    let observed = expect_match(&outcome, TrustClass::Versioned);
    assert_eq!(observed.payload, RECORDED_ETAG);
    assert!(outcome.has_diagnostic(DiagnosticCode::EtagInstability));
}

#[test]
fn content_hash_fallback_matches_identical_bytes() {
    let config = HttpsProbeConfig {
        content_hash_fallback: ContentHashFallback::for_keys([URL]),
        ..HttpsProbeConfig::default()
    };
    let body = b"the quick brown fox";
    let turn = ScriptedTurn::Respond(
        HttpResponse::new(200, headers(&[("Content-Type", "text/plain")])).with_body(&body[..]),
    );
    let (probe, transport) = scripted(config, vec![turn]);
    let outcome = probe.check_detailed(URL, &content_hash_fp(&blake3_of(body)), None);

    let observed = expect_match(&outcome, TrustClass::Exact);
    assert_eq!(observed.payload, blake3_of(body));
    assert_eq!(outcome.cost.body_bytes, body.len() as u64);
    assert!(outcome.has_diagnostic(DiagnosticCode::ContentHashFallback));
    assert_eq!(
        transport.requests()[0].method,
        HttpMethod::Get,
        "hashing needs a body, so HEAD is not an option"
    );
}

#[test]
fn content_hash_fallback_drifts_on_different_bytes() {
    let config = HttpsProbeConfig {
        content_hash_fallback: ContentHashFallback::Always,
        ..HttpsProbeConfig::default()
    };
    let turn = ScriptedTurn::Respond(HttpResponse::new(200, Headers::new()).with_body(&b"new"[..]));
    let (probe, _) = scripted(config, vec![turn]);
    let outcome = probe.check_detailed(URL, &content_hash_fp(&blake3_of(b"old")), None);

    let observed = expect_drift(&outcome, TrustClass::Exact);
    assert_eq!(observed.payload, blake3_of(b"new"));
}

#[test]
fn content_hash_fallback_is_off_by_default_and_opt_in_per_key() {
    assert!(!ContentHashFallback::default().enabled_for(URL));
    assert!(ContentHashFallback::for_keys([URL]).enabled_for(URL));
    assert!(!ContentHashFallback::for_keys([URL]).enabled_for("https://other.test/x"));
    assert!(ContentHashFallback::Always.enabled_for("https://anything.test/x"));
}

#[test]
fn loopback_plaintext_is_permitted_when_explicitly_enabled() {
    let config = HttpsProbeConfig {
        allow_plaintext_http: true,
        ..HttpsProbeConfig::default()
    };
    let (probe, _) = scripted(config, vec![respond(304, &[])]);
    let outcome = probe.check_detailed("http://127.0.0.1:9/x", &etag_fp(RECORDED_ETAG), None);

    expect_match(&outcome, TrustClass::Versioned);
    assert!(outcome.has_diagnostic(DiagnosticCode::PlaintextTransport));
}

// ====================================================================
// REGISTRATION, MANIFEST, AND TRAIT PLUMBING
// ====================================================================

#[test]
fn probe_declares_https_scheme() {
    let (probe, _) = scripted(HttpsProbeConfig::default(), vec![]);
    assert_eq!(probe.scheme(), "https");
    assert!(probe.host_pattern().is_none());
    assert_eq!(probe.priority(), 0);
}

#[test]
fn coverage_manifest_declares_the_probe_role_and_capabilities() {
    let manifest = HttpsProbe::coverage_manifest();
    assert_eq!(manifest.role, ProducerRole::Probe);
    assert_eq!(manifest.producer, "freshdag-probes/https");
    assert!(manifest
        .emits
        .iter()
        .any(|p| p.matches(freshdag_core::ir::EventKind::ProbeChecked)));
    assert_eq!(
        manifest.capabilities.get("conditional_requests"),
        Some(&serde_json::Value::Bool(true))
    );
    assert_eq!(
        manifest.capabilities.get("auth"),
        Some(&serde_json::Value::Bool(false)),
        "the probe holds no credentials and must not claim otherwise"
    );
    assert!(
        !manifest.known_limitations.is_empty(),
        "an honest manifest names its gaps"
    );
}

#[test]
fn trait_check_returns_the_same_verdict_as_check_detailed() {
    // `Probe::check` drops the diagnostics; it must not drop or soften
    // the verdict on the way past.
    let detailed = probe(vec![ScriptedTurn::Fail(TransportError::Tls)])
        .check_detailed(URL, &etag_fp(RECORDED_ETAG), None)
        .result;
    let via_trait = probe(vec![ScriptedTurn::Fail(TransportError::Tls)]).check(
        URL,
        &etag_fp(RECORDED_ETAG),
        None,
    );
    assert_eq!(detailed, via_trait);
    assert!(matches!(via_trait, ProbeResult::Unknown { .. }));
}

#[test]
fn ttl_hint_is_accepted_but_does_not_change_the_verdict() {
    // A probe cannot prove freshness from a TTL whose start it did not
    // observe. Accepting the argument and ignoring it is the honest
    // behavior; short-circuiting on it would be invariant #7 with extra
    // steps.
    let without = probe(vec![respond(503, &[])])
        .check_detailed(URL, &etag_fp(RECORDED_ETAG), None)
        .result;
    let with = probe(vec![respond(503, &[])])
        .check_detailed(
            URL,
            &etag_fp(RECORDED_ETAG),
            Some(Duration::from_secs(86_400)),
        )
        .result;
    assert_eq!(without, with);
}

#[test]
fn probe_issues_no_state_changing_method() {
    // Probes are strictly read-only. `HttpMethod` has exactly two
    // variants and neither mutates, but assert the issued traffic too,
    // so a future transport change cannot quietly widen this.
    let (probe, transport) = scripted(
        HttpsProbeConfig::default(),
        vec![respond(405, &[]), respond(200, &[("ETag", "\"v2\"")])],
    );
    let _ = probe.check_detailed(URL, &etag_fp(RECORDED_ETAG), None);
    for request in transport.requests() {
        assert!(
            matches!(request.method, HttpMethod::Head | HttpMethod::Get),
            "probes are read-only; saw {}",
            request.method
        );
    }
}
