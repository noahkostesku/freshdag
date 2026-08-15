//! Production [`HttpTransport`] backed by `reqwest::blocking`.
//!
//! `reqwest::blocking` runs its own internal runtime on a dedicated
//! thread. That is the point: the
//! [`Probe`](freshdag_core::probe::Probe) trait is synchronous and
//! making it async is an explicit, escalated decision — not something
//! to smuggle in under a transport.

use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::redirect::Policy;

use super::headers::Headers;
use super::transport::{HttpMethod, HttpRequest, HttpResponse, HttpTransport, TransportError};

/// Default per-request deadline.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A blocking `reqwest` transport.
///
/// Configured to be inert with respect to external state, as
/// probe-contract §Rate Limiting / Cost requires ("Probes MUST NOT
/// rewrite external state"):
///
/// - redirects are **not** followed by the client (the probe owns
///   redirect policy, so it can be audited),
/// - no cookie store (the `cookies` feature is not enabled at all, so
///   there is no code path that could persist one),
/// - no credentials are held, read, or forwarded.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: Client,
}

impl ReqwestTransport {
    /// Build a transport with the default timeout.
    ///
    /// # Errors
    ///
    /// [`TransportError::InvalidRequest`] if the TLS backend cannot be
    /// initialized.
    pub fn new() -> Result<Self, TransportError> {
        Self::with_timeout(DEFAULT_TIMEOUT)
    }

    /// Build a transport with an explicit per-request timeout.
    ///
    /// # Errors
    ///
    /// [`TransportError::InvalidRequest`] if the client cannot be built.
    pub fn with_timeout(timeout: Duration) -> Result<Self, TransportError> {
        let client = Client::builder()
            .timeout(timeout)
            .redirect(Policy::none())
            .user_agent(concat!("freshdag-probes/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| TransportError::InvalidRequest)?;
        Ok(Self { client })
    }

    /// Classify a `reqwest` error into our transport taxonomy.
    ///
    /// Deliberately total: there is no fall-through that could produce
    /// anything other than a `TransportError`, and every
    /// `TransportError` maps to `Unknown`. There is no path from a
    /// failed request to `Match`.
    fn classify(err: &reqwest::Error) -> TransportError {
        if err.is_timeout() {
            return TransportError::Timeout;
        }
        if err.is_body() || err.is_decode() {
            return TransportError::Body;
        }
        if err.is_builder() {
            return TransportError::InvalidRequest;
        }
        if err.is_connect() {
            // reqwest folds DNS resolution failures, TLS handshake
            // failures and TCP connect failures into `is_connect`.
            // Distinguish by inspecting the error chain rather than
            // guessing, and fall back to the retryable classification
            // when we cannot tell — `Connect` is retryable, `Tls` is
            // not, and over-reporting a non-retryable failure would
            // trigger spurious trust demotion.
            let chain = error_chain_text(err);
            if chain.contains("dns error")
                || chain.contains("failed to lookup address")
                || chain.contains("name or service not known")
                || chain.contains("nodename nor servname")
            {
                return TransportError::Dns;
            }
            if chain.contains("certificate")
                || chain.contains("tls")
                || chain.contains("handshake")
                || chain.contains("invalid peer")
            {
                return TransportError::Tls;
            }
            return TransportError::Connect;
        }
        if err.is_request() {
            return TransportError::Connect;
        }
        TransportError::Other
    }
}

/// Lowercased concatenation of an error and its `source` chain.
fn error_chain_text(err: &reqwest::Error) -> String {
    let mut text = err.to_string();
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(err);
    while let Some(e) = source {
        text.push_str("; ");
        text.push_str(&e.to_string());
        source = e.source();
    }
    text.to_lowercase()
}

impl HttpTransport for ReqwestTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let method = match request.method {
            HttpMethod::Head => reqwest::Method::HEAD,
            HttpMethod::Get => reqwest::Method::GET,
        };
        let mut builder = self.client.request(method, &request.url);
        for (name, value) in request.headers.iter() {
            builder = builder.header(name, value);
        }
        let response = builder.send().map_err(|e| Self::classify(&e))?;

        let status = response.status().as_u16();
        let mut headers = Headers::new();
        for (name, value) in response.headers() {
            // Non-UTF-8 header values are dropped rather than
            // lossily transcoded: a mangled ETag that still parses
            // would be worse than an absent one.
            if let Ok(v) = value.to_str() {
                headers.push(name.as_str(), v);
            }
        }

        let body = if request.method == HttpMethod::Get && status != 304 {
            Some(Box::new(response) as Box<dyn std::io::Read + Send>)
        } else {
            None
        };

        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}
