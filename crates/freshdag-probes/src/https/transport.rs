//! The transport seam under the `https://` probe.
//!
//! The probe's freshness logic (ETag strength, conditional requests,
//! redirect policy, trust classification) is the part that has to be
//! right. Making it depend directly on `reqwest` would mean every test
//! of that logic needs a socket, and several cases we must prove —
//! DNS failure, TLS failure, an `https://` origin that redirects to
//! `http://` — cannot be produced by a loopback server at all.
//!
//! So the probe is written against [`HttpTransport`]:
//!
//! - [`ReqwestTransport`](super::ReqwestTransport) is the production
//!   implementation (blocking; the [`Probe`](freshdag_core::probe::Probe)
//!   trait is synchronous and stays that way).
//! - [`ScriptedTransport`] is a deterministic in-memory implementation
//!   that replays a fixed sequence of responses. It performs no I/O of
//!   any kind and backs the conformance fixtures.

use std::io::{self, Read};
use std::sync::Mutex;

use super::headers::Headers;

/// The two request methods this probe is permitted to issue.
///
/// Probes are strictly read-only (probe-contract §Rate Limiting / Cost),
/// so no mutating method is representable here — that is a type-level
/// guarantee, not a convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    /// `HEAD` — preferred: O(headers), no body transfer.
    Head,
    /// `GET` — used when the origin rejects `HEAD` (405) or when
    /// content-hash fallback needs a body to stream.
    Get,
}

impl HttpMethod {
    /// Uppercase wire form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Head => "HEAD",
            Self::Get => "GET",
        }
    }
}

impl core::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single outbound request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// Absolute request URL.
    pub url: String,
    /// Method.
    pub method: HttpMethod,
    /// Conditional / negotiation headers to send.
    pub headers: Headers,
}

/// A single inbound response.
///
/// `body` is a stream, not a buffer: content-hash fallback hashes while
/// reading so a large response never has to be resident (memo
/// §Content-hash fallback, "Streaming"). It is `None` for `HEAD` and for
/// `304`.
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// Response header fields.
    pub headers: Headers,
    /// Response body reader, when one exists.
    pub body: Option<Box<dyn Read + Send>>,
}

impl core::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body", &self.body.as_ref().map(|_| "<stream>"))
            .finish()
    }
}

impl HttpResponse {
    /// A body-less response (`HEAD`, `304`, …).
    #[must_use]
    pub fn new(status: u16, headers: Headers) -> Self {
        Self {
            status,
            headers,
            body: None,
        }
    }

    /// Attach a body stream.
    #[must_use]
    pub fn with_body(mut self, body: impl Read + Send + 'static) -> Self {
        self.body = Some(Box::new(body));
        self
    }
}

/// Why a request never produced a response.
///
/// Every variant maps to `ProbeResult::Unknown` — none of them may ever
/// become `Match`. That mapping is invariant #7's load-bearing edge in
/// this crate and is asserted per-variant in the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// Request deadline elapsed.
    #[error("transport-timeout")]
    Timeout,
    /// Name resolution failed.
    #[error("transport-dns-failed")]
    Dns,
    /// TCP connect / connection reset / broken pipe.
    #[error("transport-connect-failed")]
    Connect,
    /// TLS handshake or certificate validation failed.
    #[error("transport-tls-failed")]
    Tls,
    /// The response began but the body could not be read to completion.
    #[error("transport-body-read-failed")]
    Body,
    /// The URL could not be turned into a request at all.
    #[error("transport-invalid-request")]
    InvalidRequest,
    /// Anything else the transport could not classify.
    #[error("transport-error")]
    Other,
}

impl TransportError {
    /// Is retrying this failure plausibly useful?
    ///
    /// Transient network conditions are retryable. TLS failures and
    /// malformed requests are not: they signal a misconfiguration whose
    /// resolution requires human or upstream action, and (per
    /// probe-contract §Anti-thrash Protocol) `retryable: false` is what
    /// tells the engine to force re-observation rather than spin.
    #[must_use]
    pub const fn retryable(self) -> bool {
        match self {
            Self::Timeout | Self::Dns | Self::Connect | Self::Body | Self::Other => true,
            Self::Tls | Self::InvalidRequest => false,
        }
    }

    /// Stable, secret-free, time-free token for the `Unknown` reason
    /// detail. See `docs/contracts/certificate-contract.md
    /// §The detail field`: this string participates in the `cert_id`
    /// preimage, so it must be byte-identical across runs. It therefore
    /// never contains elapsed times, ports, or URLs.
    #[must_use]
    pub const fn reason_token(self) -> &'static str {
        match self {
            Self::Timeout => "transport-timeout",
            Self::Dns => "transport-dns-failed",
            Self::Connect => "transport-connect-failed",
            Self::Tls => "transport-tls-failed",
            Self::Body => "transport-body-read-failed",
            Self::InvalidRequest => "transport-invalid-request",
            Self::Other => "transport-error",
        }
    }
}

/// A synchronous HTTP transport.
///
/// Implementations MUST NOT follow redirects: redirect policy
/// (hop cap, cross-scheme downgrade refusal, cross-origin diagnostics)
/// belongs to the probe, which is where it can be audited.
pub trait HttpTransport: Send + Sync + core::fmt::Debug {
    /// Issue one request and return one response, without following
    /// redirects.
    ///
    /// # Errors
    ///
    /// [`TransportError`] when no response was obtained.
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// One scripted turn for [`ScriptedTransport`].
#[derive(Debug)]
pub enum ScriptedTurn {
    /// Reply with this response.
    Respond(HttpResponse),
    /// Fail with this transport error.
    Fail(TransportError),
}

/// A deterministic, zero-I/O [`HttpTransport`] that replays a fixed
/// script.
///
/// Requests are recorded so tests can assert on the method and
/// conditional headers actually sent. Running past the end of the
/// script is a test bug, and surfaces as
/// [`TransportError::InvalidRequest`] rather than a panic in a
/// background thread.
#[derive(Debug)]
pub struct ScriptedTransport {
    turns: Mutex<std::collections::VecDeque<ScriptedTurn>>,
    seen: Mutex<Vec<HttpRequest>>,
}

impl ScriptedTransport {
    /// Build a transport that replays `turns` in order.
    #[must_use]
    pub fn new(turns: impl IntoIterator<Item = ScriptedTurn>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// The requests issued so far, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().expect("scripted transport poisoned").clone()
    }

    /// How many requests were issued.
    #[must_use]
    pub fn request_count(&self) -> usize {
        self.seen.lock().expect("scripted transport poisoned").len()
    }
}

impl HttpTransport for ScriptedTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.seen
            .lock()
            .expect("scripted transport poisoned")
            .push(request.clone());
        let turn = self
            .turns
            .lock()
            .expect("scripted transport poisoned")
            .pop_front();
        match turn {
            Some(ScriptedTurn::Respond(r)) => Ok(r),
            Some(ScriptedTurn::Fail(e)) => Err(e),
            None => Err(TransportError::InvalidRequest),
        }
    }
}

/// A `Read` that yields `byte` exactly `len` times.
///
/// Used to build oversized bodies for `max_fetch_bytes` tests without
/// allocating them.
#[derive(Debug)]
pub struct RepeatingBody {
    byte: u8,
    remaining: u64,
}

impl RepeatingBody {
    /// Construct a body of `len` copies of `byte`.
    #[must_use]
    pub const fn new(byte: u8, len: u64) -> Self {
        Self {
            byte,
            remaining: len,
        }
    }
}

impl Read for RepeatingBody {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        let n = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        buf[..n].fill(self.byte);
        self.remaining -= n as u64;
        Ok(n)
    }
}
