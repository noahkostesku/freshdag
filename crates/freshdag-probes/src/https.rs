//! `https://` probe — conditional-request freshness checks over HTTP.
//!
//! Design source: `docs/research/http-validity-probes.md`. Its decision
//! matrix is authoritative; this module is its implementation.
//!
//! # Trust classes this probe can produce
//!
//! | Observed situation | Trust class |
//! | --- | --- |
//! | Strong `ETag` (`"abc"`) | `versioned` |
//! | Weak `ETag` (`W/"abc"`) | `heuristic` — never `versioned` |
//! | Only `Last-Modified` | `heuristic` |
//! | `Cache-Control: immutable` | `versioned` |
//! | `Cache-Control: no-store` / `must-revalidate` | `volatile` |
//! | Content-hash fallback (opt-in) | `exact` |
//! | Neither validator nor fallback | no verdict — `Unknown` |
//!
//! Weak ETags are the single most consequential line in that table.
//! RFC 9110 defines weak equivalence at the entity level, not the octet
//! level; FreshDAG's `exact` comparators are defined on octets.
//! Promoting `W/"..."` to `versioned` would launder a trust class
//! (invariant #8) and is refused in [`etag::parse_etag`]'s consumers.
//!
//! # Invariant #7
//!
//! There is exactly one construction site for
//! [`ProbeResult::Match`](freshdag_core::probe::ProbeResult::Match) in
//! this module, and it is reachable only from a positively established
//! equality — a `304`, or an observed validator byte-equal to the
//! recorded one. Every network failure, every non-2xx/3xx status, every
//! malformed header, and every lost validator routes to
//! [`ProbeResult::Unknown`](freshdag_core::probe::ProbeResult::Unknown).
//! `tests::` asserts `!matches!(.., Match)` for each of those paths
//! individually.

pub mod etag;
pub mod headers;
pub mod redirect;
pub mod report;
pub mod reqwest_transport;
pub mod transport;

use std::collections::BTreeSet;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use freshdag_core::dependency::{Fingerprint, FingerprintKind, TrustClass};
use freshdag_core::ir::{CoverageManifest, Hash, HashAlgo};
use freshdag_core::probe::{Probe, ProbeResult};
use reqwest::Url;

use self::etag::{parse_etag, ETag};
use self::headers::{is_imf_fixdate, CacheControl, Headers, RetryAfter};
use self::redirect::{resolve_next_hop, same_origin};
use self::report::{
    DiagnosticCode, HttpsCheckOutcome, ProbeCost, ProbeDiagnostic, RetryAfterHint,
};
use self::transport::{HttpMethod, HttpRequest, HttpResponse, HttpTransport, TransportError};

pub use self::reqwest_transport::ReqwestTransport;

/// Default cap on a content-hash-fallback body: 64 MiB.
pub const DEFAULT_MAX_FETCH_BYTES: u64 = 64 * 1024 * 1024;

/// Default redirect hop budget.
pub const DEFAULT_MAX_REDIRECTS: u8 = 5;

/// Hard cap on the length of an `Unknown` reason string.
///
/// The reason becomes a certificate reason's non-normative `detail`,
/// and `detail` sits inside the `cert_id` preimage. Bounded, stable
/// strings only.
const MAX_REASON_BYTES: usize = 512;

/// Whether content-hash fallback (a full body GET + BLAKE3) is
/// permitted, and for which dependencies.
///
/// The memo specifies this as opt-in **per dependency**
/// (`fallback_to_content_hash: true` on the recorded dependency).
/// `Probe::check` receives only `(key, recorded_fp, ttl_hint)` — there
/// is no per-dependency config channel through the trait — so the
/// opt-in is expressed here, keyed by dependency URL. This is an
/// interim shape: if a dependency-level probe-config channel lands in
/// `freshdag-core`, this collapses into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentHashFallback {
    /// Never fetch bodies. The default: fallback flips a check from
    /// O(headers) to O(body).
    Never,
    /// Fetch bodies only for these exact dependency keys.
    ForKeys(BTreeSet<String>),
    /// Fetch bodies for every dependency this probe handles.
    Always,
}

impl Default for ContentHashFallback {
    fn default() -> Self {
        Self::Never
    }
}

impl ContentHashFallback {
    /// Opt in for a set of dependency keys.
    #[must_use]
    pub fn for_keys<S: Into<String>>(keys: impl IntoIterator<Item = S>) -> Self {
        Self::ForKeys(keys.into_iter().map(Into::into).collect())
    }

    /// Is fallback enabled for `key`?
    #[must_use]
    pub fn enabled_for(&self, key: &str) -> bool {
        match self {
            Self::Never => false,
            Self::Always => true,
            Self::ForKeys(keys) => keys.contains(key),
        }
    }
}

/// Tunables for [`HttpsProbe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsProbeConfig {
    /// Redirect hop budget. Chains longer than this are refused.
    pub max_redirects: u8,
    /// Cap on bytes read during content-hash fallback.
    pub max_fetch_bytes: u64,
    /// Content-hash fallback opt-in.
    pub content_hash_fallback: ContentHashFallback,
    /// Permit cleartext `http://` dependency keys.
    ///
    /// Off by default. Turning it on does NOT permit an `https:` →
    /// `http:` redirect: that refusal is unconditional (see
    /// [`redirect::resolve_next_hop`]).
    pub allow_plaintext_http: bool,
    /// Try `HEAD` first (O(headers)) and fall back to `GET` on `405`.
    pub prefer_head: bool,
}

impl Default for HttpsProbeConfig {
    fn default() -> Self {
        Self {
            max_redirects: DEFAULT_MAX_REDIRECTS,
            max_fetch_bytes: DEFAULT_MAX_FETCH_BYTES,
            content_hash_fallback: ContentHashFallback::Never,
            allow_plaintext_http: false,
            prefer_head: true,
        }
    }
}

/// Probe for the `https` scheme.
#[derive(Debug, Clone)]
pub struct HttpsProbe {
    transport: Arc<dyn HttpTransport>,
    config: HttpsProbeConfig,
}

impl HttpsProbe {
    /// Build a probe over a real network transport with default config.
    ///
    /// # Errors
    ///
    /// [`TransportError`] if the HTTP client cannot be constructed.
    pub fn new() -> Result<Self, TransportError> {
        Self::with_config(HttpsProbeConfig::default())
    }

    /// Build a probe over a real network transport with explicit config.
    ///
    /// # Errors
    ///
    /// [`TransportError`] if the HTTP client cannot be constructed.
    pub fn with_config(config: HttpsProbeConfig) -> Result<Self, TransportError> {
        let transport = ReqwestTransport::new()?;
        Ok(Self::with_transport(Arc::new(transport), config))
    }

    /// Build a probe over an arbitrary transport (tests, fixtures,
    /// recorded cassettes).
    #[must_use]
    pub fn with_transport(transport: Arc<dyn HttpTransport>, config: HttpsProbeConfig) -> Self {
        Self { transport, config }
    }

    /// This probe's declared coverage.
    #[must_use]
    pub fn coverage_manifest() -> CoverageManifest {
        let mut capabilities = std::collections::BTreeMap::new();
        capabilities.insert("conditional_requests".to_string(), true.into());
        capabilities.insert("content_hash_fallback".to_string(), true.into());
        capabilities.insert("auth".to_string(), false.into());
        CoverageManifest {
            // INTEGRATION NOTE (Phase A `ProducerRole`): once
            // `freshdag_core::ir::CoverageManifest` gains its required
            // `role` field, add `role: ProducerRole::Probe,` here. The
            // type does not exist on this branch's base commit
            // (f2f7571), so it is deliberately not hand-rolled locally.
            producer: "freshdag-probes/https".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            platforms: Vec::new(),
            emits: vec!["probe.checked".into()],
            partial: std::collections::BTreeMap::new(),
            capabilities,
            known_limitations: vec![
                "No credential support: endpoints requiring authentication return \
                 Unknown { retryable: false }."
                    .to_string(),
                "Content-hash fallback hashes raw response octets with no \
                 canonicalization, so identical resources served with different \
                 Content-Encoding hash differently (pessimistic by design; \
                 canonicalization is escalated to the comparator subsystem)."
                    .to_string(),
                "The probe is stateless: cross-check ETag churn is visible to the \
                 engine's anti-thrash policy, not to this probe."
                    .to_string(),
                "ttl_hint is not used to shorten work; a probe cannot prove \
                 freshness from a TTL it did not itself observe the start of."
                    .to_string(),
            ],
        }
    }

    /// The probe's configuration.
    #[must_use]
    pub fn config(&self) -> &HttpsProbeConfig {
        &self.config
    }

    /// Check `key`, returning the verdict plus diagnostics, cost, and
    /// the redirect chain.
    ///
    /// [`Probe::check`] delegates here and keeps only the verdict.
    #[must_use]
    pub fn check_detailed(
        &self,
        key: &str,
        recorded_fp: &Fingerprint,
        _ttl_hint: Option<Duration>,
    ) -> HttpsCheckOutcome {
        let mut ctx = Ctx::new(key);
        match self.run(key, recorded_fp, &mut ctx) {
            Ok(result) => ctx.finish(result),
            Err(unknown) => ctx.finish(unknown.into_result()),
        }
    }
}

// --------------------------------------------------------------------
// Internal machinery
// --------------------------------------------------------------------

/// An early return that is always an `Unknown`.
///
/// Using a dedicated error type (rather than returning `ProbeResult`
/// from helpers) makes it structurally impossible for a `?` in the
/// happy path to yield a `Match`.
#[derive(Debug)]
struct Unresolved {
    reason: String,
    retryable: bool,
}

impl Unresolved {
    fn new(reason: impl Into<String>, retryable: bool) -> Self {
        let mut reason = reason.into();
        if reason.len() > MAX_REASON_BYTES {
            reason.truncate(MAX_REASON_BYTES);
        }
        Self { reason, retryable }
    }

    fn into_result(self) -> ProbeResult {
        ProbeResult::Unknown {
            reason: self.reason,
            retryable: self.retryable,
        }
    }
}

/// Accumulates everything the check learns on its way to a verdict.
#[derive(Debug)]
struct Ctx {
    diagnostics: Vec<ProbeDiagnostic>,
    cost: ProbeCost,
    chain: Vec<String>,
    content_type: Option<String>,
}

impl Ctx {
    fn new(key: &str) -> Self {
        Self {
            diagnostics: Vec::new(),
            cost: ProbeCost::default(),
            chain: vec![key.to_string()],
            content_type: None,
        }
    }

    fn diag(&mut self, code: DiagnosticCode, detail: impl Into<String>) {
        self.diagnostics.push(ProbeDiagnostic::new(code, detail));
    }

    fn finish(self, result: ProbeResult) -> HttpsCheckOutcome {
        HttpsCheckOutcome {
            result,
            diagnostics: self.diagnostics,
            cost: self.cost,
            redirect_chain: self.chain,
            content_type: self.content_type,
        }
    }
}

/// How this check will ask the server "has it changed?".
#[derive(Debug, Clone, PartialEq, Eq)]
enum Plan {
    /// `If-None-Match: <wire>`; recorded validator is an ETag.
    IfNoneMatch { wire: String, weak: bool },
    /// `If-Modified-Since: <http-date>`; recorded validator is a
    /// `Last-Modified`.
    IfModifiedSince(String),
    /// Recorded fingerprint asserts `Cache-Control: immutable`.
    Immutable,
    /// Recorded fingerprint is a content hash; verify by re-hashing.
    ContentHash(Hash),
}

impl Plan {
    /// Trust class implied by the RECORDED fingerprint.
    ///
    /// `Probe::check` is handed a `Fingerprint` but not the trust class
    /// recorded alongside it on the certificate, so the recorded class
    /// has to be re-derived here to detect demotion. That is a real gap
    /// in the trait shape and is reported as such.
    const fn recorded_trust_class(&self) -> TrustClass {
        match self {
            Self::IfNoneMatch { weak: false, .. } | Self::Immutable => TrustClass::Versioned,
            Self::IfNoneMatch { weak: true, .. } | Self::IfModifiedSince(_) => {
                TrustClass::Heuristic
            }
            Self::ContentHash(_) => TrustClass::Exact,
        }
    }

    fn conditional_headers(&self) -> Headers {
        let mut h = Headers::new();
        match self {
            Self::IfNoneMatch { wire, .. } => h.push("If-None-Match", wire.clone()),
            Self::IfModifiedSince(date) => h.push("If-Modified-Since", date.clone()),
            Self::Immutable | Self::ContentHash(_) => {}
        }
        h
    }
}

/// What the final response's headers assert about freshness.
#[derive(Debug)]
struct Classification {
    etag: Option<ETag>,
    last_modified: Option<String>,
    cache_control: CacheControl,
    trust_class: TrustClass,
}

impl Classification {
    /// Does the response carry any usable freshness validator?
    const fn has_validator(&self) -> bool {
        self.etag.is_some() || self.last_modified.is_some() || self.cache_control.immutable
    }
}

impl HttpsProbe {
    /// The whole check. `Ok` is a decided verdict; `Err` is an
    /// `Unknown`.
    fn run(
        &self,
        key: &str,
        recorded_fp: &Fingerprint,
        ctx: &mut Ctx,
    ) -> Result<ProbeResult, Unresolved> {
        let original = self.parse_and_gate_url(key, ctx)?;
        let plan = self.plan(key, recorded_fp)?;

        let final_response = self.fetch(&original, &plan, ctx)?;
        let classification = self.classify(&final_response.headers, ctx);

        if let Some(ct) = final_response.headers.get("Content-Type") {
            ctx.content_type = Some(ct.to_string());
            ctx.diag(DiagnosticCode::RepresentationRecorded, ct.to_string());
        }

        // Demotion is never silent: announce it before deciding.
        if classification.has_validator()
            && classification
                .trust_class
                .is_demotion_from(plan.recorded_trust_class())
        {
            ctx.diag(
                DiagnosticCode::TrustDemoted,
                format!(
                    "recorded {:?}, observed {:?}",
                    plan.recorded_trust_class(),
                    classification.trust_class
                ),
            );
        }

        if final_response.status == 304 {
            return Ok(self.decide_not_modified(recorded_fp, &plan, &classification));
        }

        self.decide_ok(&original, &plan, &classification, final_response, ctx)
    }

    fn parse_and_gate_url(&self, key: &str, ctx: &mut Ctx) -> Result<Url, Unresolved> {
        // The key is NOT echoed into the reason: dependency URLs can
        // carry query-string tokens, and reasons end up on shareable
        // certificates.
        let url = Url::parse(key).map_err(|_| Unresolved::new("malformed-dependency-key", false))?;
        match url.scheme() {
            "https" => Ok(url),
            "http" if self.config.allow_plaintext_http => {
                ctx.diag(
                    DiagnosticCode::PlaintextTransport,
                    "dependency key uses cleartext http://".to_string(),
                );
                Ok(url)
            }
            "http" => Err(Unresolved::new("plaintext-http-refused", false)),
            other => {
                let mut scheme = other.to_string();
                scheme.truncate(32);
                Err(Unresolved::new(
                    format!("unsupported-url-scheme={scheme}"),
                    false,
                ))
            }
        }
    }

    fn plan(&self, key: &str, recorded_fp: &Fingerprint) -> Result<Plan, Unresolved> {
        match recorded_fp.kind {
            FingerprintKind::Etag => match parse_etag(&recorded_fp.payload) {
                Ok(e) => Ok(Plan::IfNoneMatch {
                    wire: e.wire(),
                    weak: e.weak,
                }),
                Err(err) => Err(Unresolved::new(
                    format!("recorded-fingerprint-malformed etag={err}"),
                    false,
                )),
            },
            FingerprintKind::Mtime => {
                if is_imf_fixdate(&recorded_fp.payload) {
                    Ok(Plan::IfModifiedSince(recorded_fp.payload.clone()))
                } else {
                    Err(Unresolved::new(
                        "recorded-fingerprint-malformed mtime=not-imf-fixdate",
                        false,
                    ))
                }
            }
            FingerprintKind::ContentHash => {
                if !self.config.content_hash_fallback.enabled_for(key) {
                    // probe-contract §Trust-class Semantics: an `exact`
                    // dependency the probe cannot content-hash returns
                    // Unknown { retryable: true }, never a bare Drift.
                    return Err(Unresolved::new("content-hash-fallback-required", true));
                }
                recorded_fp
                    .payload
                    .parse::<Hash>()
                    .map(Plan::ContentHash)
                    .map_err(|_| {
                        Unresolved::new("recorded-fingerprint-malformed content-hash", false)
                    })
            }
            FingerprintKind::Custom if recorded_fp.payload == "immutable" => Ok(Plan::Immutable),
            FingerprintKind::Custom | FingerprintKind::Version => Err(Unresolved::new(
                format!(
                    "unsupported-fingerprint-kind={}",
                    recorded_fp.kind.as_wire_prefix()
                ),
                false,
            )),
        }
    }

    /// Issue requests until a non-redirect response, applying redirect
    /// policy and the `HEAD` → `GET` fallback.
    fn fetch(
        &self,
        original: &Url,
        plan: &Plan,
        ctx: &mut Ctx,
    ) -> Result<HttpResponse, Unresolved> {
        let mut current = original.clone();
        let mut hops: u8 = 0;
        let mut head_retried = false;
        let mut method = if matches!(plan, Plan::ContentHash(_)) || !self.config.prefer_head {
            // Content-hash fallback needs a body; HEAD gives none.
            HttpMethod::Get
        } else {
            HttpMethod::Head
        };

        loop {
            let request = HttpRequest {
                url: current.to_string(),
                method,
                headers: plan.conditional_headers(),
            };
            ctx.cost.requests += 1;
            let response = self
                .transport
                .execute(&request)
                .map_err(|e| Unresolved::new(e.reason_token(), e.retryable()))?;

            match response.status {
                304 => return Ok(response),
                405 if method == HttpMethod::Head && !head_retried => {
                    head_retried = true;
                    method = HttpMethod::Get;
                    ctx.diag(
                        DiagnosticCode::HeadNotAllowed,
                        "origin rejected HEAD; retrying with GET",
                    );
                }
                200..=299 => return Ok(response),
                300..=399 => {
                    if hops >= self.config.max_redirects {
                        return Err(Unresolved::new(
                            format!(
                                "redirect-chain-too-deep max-redirects={}",
                                self.config.max_redirects
                            ),
                            false,
                        ));
                    }
                    let next = resolve_next_hop(&current, response.headers.get("Location"))
                        .map_err(|r| Unresolved::new(r.reason_token(), false))?;
                    hops += 1;
                    ctx.cost.redirect_hops = u32::from(hops);
                    if !same_origin(original, &next) {
                        ctx.diag(
                            DiagnosticCode::CrossOriginRedirect,
                            format!(
                                "{} -> {}",
                                origin_of(original),
                                origin_of(&next)
                            ),
                        );
                    }
                    ctx.chain.push(next.to_string());
                    current = next;
                }
                401 | 403 => {
                    return Err(Unresolved::new(
                        format!("auth-required http-status={}", response.status),
                        false,
                    ))
                }
                429 => {
                    ctx.cost.retry_after = RetryAfter::parse(&response.headers).map(|ra| match ra {
                        RetryAfter::Seconds(s) => RetryAfterHint::Seconds(s),
                        RetryAfter::HttpDate(d) => RetryAfterHint::HttpDate(d),
                    });
                    ctx.diag(
                        DiagnosticCode::RateLimited,
                        format!("retry-after={:?}", ctx.cost.retry_after),
                    );
                    return Err(Unresolved::new("rate-limited http-status=429", true));
                }
                400..=499 => {
                    return Err(Unresolved::new(
                        format!("http-status={}", response.status),
                        false,
                    ))
                }
                500..=599 => {
                    return Err(Unresolved::new(
                        format!("http-status={}", response.status),
                        true,
                    ))
                }
                other => {
                    return Err(Unresolved::new(format!("unexpected-http-status={other}"), false))
                }
            }
        }
    }

    /// Derive the observed validator and trust class from the final
    /// response's headers.
    fn classify(&self, headers: &Headers, ctx: &mut Ctx) -> Classification {
        let raw_etags = headers.get_all("ETag");
        if raw_etags.len() > 1 {
            ctx.diag(
                DiagnosticCode::DuplicateEtag,
                format!("{} ETag header fields", raw_etags.len()),
            );
        }
        let etag = match raw_etags.first().map(|raw| parse_etag(raw)) {
            Some(Ok(e)) => Some(e),
            Some(Err(err)) => {
                // A malformed ETag makes the validator UNAVAILABLE. It
                // is never repaired into a usable one: that is how a
                // fabricated `versioned` claim would get made.
                ctx.diag(DiagnosticCode::MalformedEtag, err.to_string());
                None
            }
            None => None,
        };

        let last_modified = headers
            .get("Last-Modified")
            .filter(|v| is_imf_fixdate(v))
            .map(ToString::to_string);

        let cache_control = CacheControl::parse(headers);

        let trust_class = if cache_control.forbids_caching() {
            if cache_control.immutable {
                ctx.diag(
                    DiagnosticCode::ContradictoryCacheControl,
                    "immutable together with no-store/must-revalidate; \
                     resolved as volatile",
                );
            }
            // The server explicitly forbids caching assumptions. Take
            // it at its word even if a validator is present: invariant
            // #15, correctness beats hit rate.
            TrustClass::Volatile
        } else if cache_control.immutable {
            TrustClass::Versioned
        } else if let Some(e) = &etag {
            if e.weak {
                TrustClass::Heuristic
            } else {
                TrustClass::Versioned
            }
        } else if last_modified.is_some() {
            TrustClass::Heuristic
        } else {
            TrustClass::Volatile
        };

        Classification {
            etag,
            last_modified,
            cache_control,
            trust_class,
        }
    }

    /// `304 Not Modified` — the server explicitly asserts unchanged.
    fn decide_not_modified(
        &self,
        recorded_fp: &Fingerprint,
        plan: &Plan,
        classification: &Classification,
    ) -> ProbeResult {
        // A `304` is allowed to omit almost every header. Absence of a
        // validator here is the server confirming ours, NOT a lost
        // version signal, so we keep the recorded class unless the
        // `304` itself said something stronger or weaker.
        let observed_trust_class = if classification.has_validator()
            || classification.cache_control.forbids_caching()
        {
            classification.trust_class
        } else {
            plan.recorded_trust_class()
        };
        ProbeResult::Match {
            observed_fp: recorded_fp.clone(),
            observed_trust_class,
        }
    }

    /// A `2xx` response — compare validators, or hash the body.
    fn decide_ok(
        &self,
        original: &Url,
        plan: &Plan,
        classification: &Classification,
        response: HttpResponse,
        ctx: &mut Ctx,
    ) -> Result<ProbeResult, Unresolved> {
        match plan {
            Plan::ContentHash(recorded) => {
                self.decide_content_hash(original, recorded, classification, response, ctx)
            }
            Plan::IfNoneMatch { wire, .. } => {
                let Some(observed) = &classification.etag else {
                    // The endpoint used to carry a version signal and
                    // no longer does. probe-contract §Anti-thrash: emit
                    // the demotion diagnostic and refuse to decide.
                    ctx.diag(
                        DiagnosticCode::TrustDemoted,
                        "recorded fingerprint was an ETag; response carries none",
                    );
                    return Err(Unresolved::new("version-signal-lost", false));
                };
                if &observed.wire() == wire {
                    // We sent If-None-Match with exactly this tag and
                    // got 200 anyway. The endpoint's own change signal
                    // disagrees with itself. Match (the tag is
                    // unchanged) but say so loudly.
                    ctx.diag(
                        DiagnosticCode::EtagInstability,
                        "200 OK echoing the ETag we sent in If-None-Match; \
                         endpoint ignores conditional requests",
                    );
                    Ok(ProbeResult::Match {
                        observed_fp: Fingerprint::new(FingerprintKind::Etag, observed.wire()),
                        observed_trust_class: classification.trust_class,
                    })
                } else {
                    Ok(ProbeResult::Drift {
                        observed_fp: Fingerprint::new(FingerprintKind::Etag, observed.wire()),
                        observed_trust_class: classification.trust_class,
                    })
                }
            }
            Plan::IfModifiedSince(recorded_date) => {
                if let Some(observed) = &classification.last_modified {
                    if observed == recorded_date {
                        ctx.diag(
                            DiagnosticCode::EtagInstability,
                            "200 OK with an unchanged Last-Modified after \
                             If-Modified-Since; endpoint ignores conditional requests",
                        );
                        return Ok(ProbeResult::Match {
                            observed_fp: Fingerprint::new(
                                FingerprintKind::Mtime,
                                observed.clone(),
                            ),
                            observed_trust_class: classification.trust_class,
                        });
                    }
                    return Ok(ProbeResult::Drift {
                        observed_fp: Fingerprint::new(FingerprintKind::Mtime, observed.clone()),
                        observed_trust_class: classification.trust_class,
                    });
                }
                if let Some(observed) = &classification.etag {
                    // Escalation: the endpoint gained an ETag. We
                    // cannot prove equality across validator kinds, and
                    // a 200 to a conditional request means "changed" as
                    // far as we can tell, so Drift — never Match.
                    return Ok(ProbeResult::Drift {
                        observed_fp: Fingerprint::new(FingerprintKind::Etag, observed.wire()),
                        observed_trust_class: classification.trust_class,
                    });
                }
                ctx.diag(
                    DiagnosticCode::TrustDemoted,
                    "recorded fingerprint was a Last-Modified; response carries none",
                );
                Err(Unresolved::new("version-signal-lost last-modified", false))
            }
            Plan::Immutable => {
                if classification.cache_control.immutable
                    && !classification.cache_control.forbids_caching()
                {
                    return Ok(ProbeResult::Match {
                        observed_fp: Fingerprint::new(FingerprintKind::Custom, "immutable"),
                        observed_trust_class: TrustClass::Versioned,
                    });
                }
                ctx.diag(
                    DiagnosticCode::TrustDemoted,
                    "recorded fingerprint asserted Cache-Control: immutable; \
                     response no longer does",
                );
                Err(Unresolved::new("version-signal-lost immutable", false))
            }
        }
    }

    /// Stream the body through BLAKE3 and compare with the recorded
    /// hash.
    fn decide_content_hash(
        &self,
        original: &Url,
        recorded: &Hash,
        classification: &Classification,
        response: HttpResponse,
        ctx: &mut Ctx,
    ) -> Result<ProbeResult, Unresolved> {
        ctx.diag(
            DiagnosticCode::ContentHashFallback,
            format!("streaming GET of {}", origin_of(original)),
        );

        let Some(mut body) = response.body else {
            return Err(Unresolved::new("content-hash-fallback-no-body", false));
        };

        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut total: u64 = 0;
        loop {
            let n = body
                .read(&mut buf)
                .map_err(|_| Unresolved::new(TransportError::Body.reason_token(), true))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > self.config.max_fetch_bytes {
                ctx.cost.body_bytes = total;
                return Err(Unresolved::new(
                    format!(
                        "body-exceeds-max-fetch-bytes max-fetch-bytes={}",
                        self.config.max_fetch_bytes
                    ),
                    false,
                ));
            }
            hasher.update(&buf[..n]);
        }
        ctx.cost.body_bytes = total;

        let observed = Hash {
            algo: HashAlgo::Blake3,
            digest_hex: hasher.finalize().to_hex().to_string(),
        };
        let observed_fp = Fingerprint::new(FingerprintKind::ContentHash, observed.to_string());

        // The octets are in hand, so this observation is `exact`
        // regardless of what the headers claim — except when the server
        // forbids caching assumptions, where we defer to it.
        let observed_trust_class = if classification.cache_control.forbids_caching() {
            TrustClass::Volatile
        } else {
            TrustClass::Exact
        };

        if observed == *recorded {
            Ok(ProbeResult::Match {
                observed_fp,
                observed_trust_class,
            })
        } else {
            Ok(ProbeResult::Drift {
                observed_fp,
                observed_trust_class,
            })
        }
    }
}

/// `scheme://host[:port]` — safe to put in a diagnostic (never in a
/// certificate reason), and free of any query string.
fn origin_of(url: &Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(h), Some(p)) => format!("{}://{h}:{p}", url.scheme()),
        (Some(h), None) => format!("{}://{h}", url.scheme()),
        (None, _) => url.scheme().to_string(),
    }
}

impl Probe for HttpsProbe {
    fn scheme(&self) -> &'static str {
        "https"
    }

    fn check(
        &self,
        key: &str,
        recorded_fp: &Fingerprint,
        ttl_hint: Option<Duration>,
    ) -> ProbeResult {
        let outcome = self.check_detailed(key, recorded_fp, ttl_hint);
        // Diagnostics do not fit through `ProbeResult`; surface them
        // here so nothing is lost when the probe is driven through the
        // trait. `probe.trust_demoted` in particular is contractually
        // required to be non-silent.
        for diagnostic in &outcome.diagnostics {
            if diagnostic.code == DiagnosticCode::TrustDemoted {
                tracing::warn!(
                    diagnostic = diagnostic.code.as_wire_str(),
                    detail = %diagnostic.detail,
                    "probe trust demotion"
                );
            } else {
                tracing::debug!(
                    diagnostic = diagnostic.code.as_wire_str(),
                    detail = %diagnostic.detail,
                    "probe diagnostic"
                );
            }
        }
        tracing::debug!(
            requests = outcome.cost.requests,
            redirect_hops = outcome.cost.redirect_hops,
            body_bytes = outcome.cost.body_bytes,
            "probe.cost"
        );
        outcome.result
    }
}
