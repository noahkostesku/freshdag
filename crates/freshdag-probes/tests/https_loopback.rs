//! Loopback tests for the *real* transport.
//!
//! `src/https/tests.rs` proves the probe's freshness logic against a
//! scripted transport; that covers what the probe decides. These tests
//! cover the other half — that [`ReqwestTransport`] actually puts the
//! conditional header on the wire, actually reads the status back, and
//! actually does not follow redirects behind the probe's back. A seam is
//! only worth having if both sides of it are tested.
//!
//! # No outbound network
//!
//! Every server here binds `127.0.0.1:0` explicitly (rather than
//! `httptest`'s default, which prefers IPv6 loopback), and every request
//! targets that address. Nothing resolves a name, nothing leaves the
//! machine, and no test depends on wall-clock timing. The
//! connection-refused test gets its closed port by binding one and
//! dropping the listener, so it does not guess at a port being free.
//!
//! These servers speak cleartext HTTP, so the probe is configured with
//! `allow_plaintext_http: true`. That flag exists exactly for this, and
//! it does NOT weaken the `https:` → `http:` redirect refusal, which is
//! unconditional — asserted in `src/https/tests.rs` and in
//! `fixtures/probe-conformance/https/unknown/cross-scheme-downgrade/`.

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

use freshdag_core::dependency::{Fingerprint, FingerprintKind, TrustClass};
use freshdag_core::probe::ProbeResult;
use freshdag_probes::https::headers::Headers;
use freshdag_probes::https::transport::{HttpMethod, HttpRequest, HttpTransport, TransportError};
use freshdag_probes::https::ReqwestTransport;
use freshdag_probes::{ContentHashFallback, HttpsProbe, HttpsProbeConfig};
use httptest::matchers::{contains, request};
use httptest::responders::status_code;
use httptest::{all_of, Expectation, Server, ServerBuilder};

const RECORDED_ETAG: &str = "\"v1\"";

/// A loopback-only server. `127.0.0.1:0` is explicit: the default
/// builder prefers IPv6 loopback, and pinning IPv4 keeps the addresses
/// in failure output readable and the binding unambiguous.
fn loopback_server() -> Server {
    let addr: SocketAddr = "127.0.0.1:0".parse().expect("loopback addr");
    ServerBuilder::new()
        .bind_addr(addr)
        .run()
        .expect("bind loopback server")
}

fn plaintext_probe() -> HttpsProbe {
    plaintext_probe_with(HttpsProbeConfig {
        allow_plaintext_http: true,
        ..HttpsProbeConfig::default()
    })
}

fn plaintext_probe_with(config: HttpsProbeConfig) -> HttpsProbe {
    let transport = ReqwestTransport::new().expect("build reqwest transport");
    HttpsProbe::with_transport(Arc::new(transport), config)
}

fn etag_fp(wire: &str) -> Fingerprint {
    Fingerprint::new(FingerprintKind::Etag, wire)
}

/// A port nothing is listening on: bind one, learn its number, drop the
/// listener. Guessing a "probably free" port would make this test flaky
/// on a busy machine.
fn closed_loopback_port() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    addr
}

#[test]
fn conditional_request_reaches_the_wire_and_304_is_understood() {
    let server = loopback_server();
    server.expect(
        Expectation::matching(all_of![
            request::method_path("HEAD", "/pricing"),
            // The whole point of the probe: the recorded validator is
            // echoed back so the origin can answer cheaply.
            request::headers(contains(("if-none-match", RECORDED_ETAG))),
        ])
        .respond_with(status_code(304)),
    );

    let outcome = plaintext_probe().check_detailed(
        &server.url_str("/pricing"),
        &etag_fp(RECORDED_ETAG),
        None,
    );

    match outcome.result {
        ProbeResult::Match {
            observed_fp,
            observed_trust_class,
        } => {
            assert_eq!(observed_fp.payload, RECORDED_ETAG);
            assert_eq!(observed_trust_class, TrustClass::Versioned);
        }
        other => panic!("expected Match from a real 304, got {other:?}"),
    }
    assert_eq!(outcome.cost.requests, 1);
}

#[test]
fn freshness_headers_are_parsed_off_a_real_response() {
    let server = loopback_server();
    server.expect(
        Expectation::matching(request::method_path("HEAD", "/pricing")).respond_with(
            status_code(200)
                .append_header("ETag", "\"v2\"")
                .append_header("Last-Modified", "Sun, 06 Nov 1994 08:49:37 GMT")
                .append_header("Cache-Control", "max-age=60")
                .append_header("Content-Type", "text/html; charset=utf-8"),
        ),
    );

    let outcome = plaintext_probe().check_detailed(
        &server.url_str("/pricing"),
        &etag_fp(RECORDED_ETAG),
        None,
    );

    match outcome.result {
        ProbeResult::Drift {
            observed_fp,
            observed_trust_class,
        } => {
            assert_eq!(observed_fp.kind, FingerprintKind::Etag);
            assert_eq!(observed_fp.payload, "\"v2\"");
            assert_eq!(observed_trust_class, TrustClass::Versioned);
        }
        other => panic!("expected Drift, got {other:?}"),
    }
    assert_eq!(
        outcome.content_type.as_deref(),
        Some("text/html; charset=utf-8"),
        "the negotiated representation is recorded as metadata"
    );
}

#[test]
fn if_modified_since_reaches_the_wire_for_a_recorded_last_modified() {
    let date = "Sun, 06 Nov 1994 08:49:37 GMT";
    let server = loopback_server();
    server.expect(
        Expectation::matching(all_of![
            request::method_path("HEAD", "/doc"),
            request::headers(contains(("if-modified-since", date))),
        ])
        .respond_with(status_code(304)),
    );

    // The expectation above is the assertion: if the probe had sent
    // `If-None-Match` instead, or no conditional header at all, the
    // request would not have matched and the server would fail the test
    // on drop.
    let outcome = plaintext_probe().check_detailed(
        &server.url_str("/doc"),
        &Fingerprint::new(FingerprintKind::Mtime, date),
        None,
    );
    assert!(matches!(outcome.result, ProbeResult::Match { .. }));
}

#[test]
fn head_405_falls_back_to_a_real_get() {
    let server = loopback_server();
    server.expect(
        Expectation::matching(request::method_path("HEAD", "/pricing"))
            .respond_with(status_code(405)),
    );
    server.expect(
        Expectation::matching(request::method_path("GET", "/pricing"))
            .respond_with(status_code(304)),
    );

    let outcome = plaintext_probe().check_detailed(
        &server.url_str("/pricing"),
        &etag_fp(RECORDED_ETAG),
        None,
    );
    assert!(matches!(outcome.result, ProbeResult::Match { .. }));
    assert_eq!(outcome.cost.requests, 2);
}

#[test]
fn a_real_body_is_streamed_and_hashed_for_content_hash_fallback() {
    let body = "pricing-page-v1\n";
    let server = loopback_server();
    server.expect(
        Expectation::matching(request::method_path("GET", "/pricing"))
            .respond_with(status_code(200).body(body)),
    );

    let url = server.url_str("/pricing");
    let probe = plaintext_probe_with(HttpsProbeConfig {
        allow_plaintext_http: true,
        content_hash_fallback: ContentHashFallback::for_keys([url.clone()]),
        ..HttpsProbeConfig::default()
    });
    let recorded = format!("blake3:{}", blake3::hash(body.as_bytes()).to_hex());
    let outcome = probe.check_detailed(
        &url,
        &Fingerprint::new(FingerprintKind::ContentHash, &recorded),
        None,
    );

    match outcome.result {
        ProbeResult::Match {
            observed_fp,
            observed_trust_class,
        } => {
            assert_eq!(observed_fp.payload, recorded);
            assert_eq!(observed_trust_class, TrustClass::Exact);
        }
        other => panic!("expected Match on identical bytes, got {other:?}"),
    }
    assert_eq!(outcome.cost.body_bytes, body.len() as u64);
}

#[test]
fn the_transport_does_not_follow_redirects_itself() {
    // Redirect policy — the hop cap, the cross-scheme downgrade refusal,
    // the cross-origin diagnostic — lives in the probe, where it can be
    // audited in one place. If the client followed redirects, none of
    // that would run. Assert the transport hands the 3xx back untouched.
    let server = loopback_server();
    server.expect(
        Expectation::matching(request::method_path("HEAD", "/old"))
            .respond_with(status_code(301).append_header("Location", server.url_str("/new"))),
    );

    let transport = ReqwestTransport::new().expect("build transport");
    let response = transport
        .execute(&HttpRequest {
            url: server.url_str("/old"),
            method: HttpMethod::Head,
            headers: Headers::new(),
        })
        .expect("loopback request succeeds");

    assert_eq!(response.status, 301);
    assert!(response.headers.get("Location").is_some());
    assert!(response.body.is_none(), "HEAD yields no body to stream");
}

#[test]
fn the_transport_reports_status_and_headers_verbatim() {
    let server = loopback_server();
    server.expect(
        Expectation::matching(request::method_path("GET", "/x")).respond_with(
            status_code(203)
                .append_header("ETag", "W/\"weak\"")
                .body("hello"),
        ),
    );

    let transport = ReqwestTransport::new().expect("build transport");
    let mut response = transport
        .execute(&HttpRequest {
            url: server.url_str("/x"),
            method: HttpMethod::Get,
            headers: Headers::new(),
        })
        .expect("loopback request succeeds");

    assert_eq!(response.status, 203);
    assert_eq!(response.headers.get("ETag"), Some("W/\"weak\""));

    let mut body = String::new();
    std::io::Read::read_to_string(response.body.as_mut().expect("GET has a body"), &mut body)
        .expect("read body");
    assert_eq!(body, "hello");
}

#[test]
fn a_refused_connection_is_unknown_and_retryable_never_match() {
    // The real classifier, on a real failed connect. This is the
    // invariant-#7 edge with the actual `reqwest` error taxonomy behind
    // it rather than a scripted one.
    let addr = closed_loopback_port();
    let url = format!("http://{addr}/pricing");
    let outcome = plaintext_probe().check_detailed(&url, &etag_fp(RECORDED_ETAG), None);

    assert!(
        !matches!(outcome.result, ProbeResult::Match { .. }),
        "a refused connection MUST NOT be readable as fresh; got {:?}",
        outcome.result
    );
    match outcome.result {
        ProbeResult::Unknown { reason, retryable } => {
            assert_eq!(reason, TransportError::Connect.reason_token());
            assert!(retryable, "a refused connection is a transient");
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}
