//! What a probe check reports *besides* its verdict.
//!
//! [`ProbeResult`](freshdag_core::probe::ProbeResult) is deliberately
//! narrow: three variants, no side channel. But two of this probe's
//! contractual obligations do not fit inside it:
//!
//! - probe-contract §Anti-thrash Protocol requires a
//!   `probe.trust_demoted` diagnostic when a version signal disappears
//!   ("demotion is explicit, never silent").
//! - probe-contract §Rate Limiting / Cost requires a `probe.cost`
//!   metric.
//!
//! Rather than widen a core type, the probe exposes
//! [`HttpsProbe::check_detailed`](super::HttpsProbe::check_detailed),
//! which returns the verdict *plus* this report.
//! [`Probe::check`](freshdag_core::probe::Probe::check) delegates to it
//! and logs the diagnostics through `tracing`, so nothing is lost when
//! the probe is driven through the trait.

use freshdag_core::dependency::{Fingerprint, TrustClass};
use freshdag_core::probe::ProbeResult;
use serde::{Deserialize, Serialize};

/// A machine-readable diagnostic code.
///
/// These are *diagnostics*, not `ReasonCode`s. Probes do not choose
/// certificate reason codes (that is the engine's closed enum); these
/// annotate the observation itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticCode {
    /// A dependency previously carrying a version signal no longer has
    /// one. probe-contract §Anti-thrash Protocol: the engine forces
    /// re-observation of every artifact edge before the trust class
    /// changes on any certificate.
    #[serde(rename = "probe.trust_demoted")]
    TrustDemoted,
    /// The endpoint changed its ETag while (as far as we can tell)
    /// serving the same representation, or returned `200` echoing the
    /// very validator we sent. Either way the version token and the
    /// content disagree.
    EtagInstability,
    /// The `ETag` header was present but unparseable; it was ignored
    /// rather than repaired.
    MalformedEtag,
    /// More than one `ETag` header field was present.
    DuplicateEtag,
    /// The chain left the original origin. The caller's dependency URL
    /// has been rewritten upstream.
    CrossOriginRedirect,
    /// The origin rejected `HEAD` (405); the probe retried with `GET`.
    HeadNotAllowed,
    /// `429`; `Retry-After`, if parseable, is on
    /// [`ProbeCost::retry_after`].
    RateLimited,
    /// The request was issued over cleartext `http://`.
    PlaintextTransport,
    /// Content-hash fallback ran: this check cost O(body), not
    /// O(headers).
    ContentHashFallback,
    /// The observed representation's media type, recorded as metadata.
    /// `docs/research/http-validity-probes.md` §Open questions #2
    /// (whether the fingerprint should be scoped to the negotiated
    /// representation) is unresolved; v0 records and does not scope.
    RepresentationRecorded,
    /// `Cache-Control` asserted both `immutable` and a
    /// caching-forbidden directive. Contradictory; resolved
    /// pessimistically.
    ContradictoryCacheControl,
}

impl DiagnosticCode {
    /// Stable wire string.
    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::TrustDemoted => "probe.trust_demoted",
            Self::EtagInstability => "etag-instability",
            Self::MalformedEtag => "malformed-etag",
            Self::DuplicateEtag => "duplicate-etag",
            Self::CrossOriginRedirect => "cross-origin-redirect",
            Self::HeadNotAllowed => "head-not-allowed",
            Self::RateLimited => "rate-limited",
            Self::PlaintextTransport => "plaintext-transport",
            Self::ContentHashFallback => "content-hash-fallback",
            Self::RepresentationRecorded => "representation-recorded",
            Self::ContradictoryCacheControl => "contradictory-cache-control",
        }
    }
}

/// A diagnostic with human-readable context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeDiagnostic {
    /// The code.
    pub code: DiagnosticCode,
    /// Free-text context. Unlike `ProbeResult::Unknown::reason`, this
    /// does NOT enter the certificate or the `cert_id` preimage, so it
    /// may contain URLs.
    pub detail: String,
}

impl ProbeDiagnostic {
    /// Construct a diagnostic.
    #[must_use]
    pub fn new(code: DiagnosticCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// What the check cost. probe-contract §Rate Limiting / Cost.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeCost {
    /// HTTP requests issued (including redirect hops and the HEAD→GET
    /// retry).
    pub requests: u32,
    /// Redirect hops followed.
    pub redirect_hops: u32,
    /// Response body bytes read (non-zero only under content-hash
    /// fallback).
    pub body_bytes: u64,
    /// Server-provided backoff hint from a `429`.
    pub retry_after: Option<RetryAfterHint>,
}

/// Serializable form of a `Retry-After` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryAfterHint {
    /// Delta-seconds.
    Seconds(u64),
    /// An HTTP-date, verbatim. Not converted: that needs a trusted
    /// clock the probe does not own.
    HttpDate(String),
}

/// The verdict plus everything the probe learned while reaching it.
#[derive(Debug)]
pub struct HttpsCheckOutcome {
    /// The contractual verdict.
    pub result: ProbeResult,
    /// Diagnostics, in the order they were observed.
    pub diagnostics: Vec<ProbeDiagnostic>,
    /// Cost of the check.
    pub cost: ProbeCost,
    /// Redirect chain actually followed, starting at the original URL.
    /// The dependency key remains `chain[0]`.
    pub redirect_chain: Vec<String>,
    /// `Content-Type` of the final response, when present.
    pub content_type: Option<String>,
}

impl HttpsCheckOutcome {
    /// Does the outcome carry a diagnostic with this code?
    #[must_use]
    pub fn has_diagnostic(&self, code: DiagnosticCode) -> bool {
        self.diagnostics.iter().any(|d| d.code == code)
    }

    /// Every diagnostic code present, deduplicated and sorted — the
    /// stable form goldens compare against.
    #[must_use]
    pub fn diagnostic_codes(&self) -> Vec<DiagnosticCode> {
        let mut codes: Vec<_> = self.diagnostics.iter().map(|d| d.code).collect();
        codes.sort_unstable();
        codes.dedup();
        codes
    }
}

/// Wire form of `probe.checked`'s `result` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeResultKind {
    /// `ProbeResult::Match`.
    Match,
    /// `ProbeResult::Drift`.
    Drift,
    /// `ProbeResult::Unknown`.
    Unknown,
}

/// Payload of the `probe.checked` IR event.
///
/// Shape per `docs/contracts/execution-ir.md`:
/// `{ scheme, key, observed_fingerprint, trust_class, result,
/// retryable? }`.
///
/// `retryable` is REQUIRED when `result == "unknown"` and MUST be
/// absent otherwise. It does not appear on certificates —
/// certificates explain, the log schedules — but it has to be in the
/// canonical log or the engine's retry/anti-thrash state stops being
/// reconstructable from it (invariant #5).
///
/// The probe emits the *payload*, not the envelope: `event_id` and `ts`
/// are the emitting layer's to assign, and manufacturing them here
/// would make every probe result nondeterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeCheckedPayload {
    /// Dependency scheme (`https`).
    pub scheme: String,
    /// Dependency key — the ORIGINAL URL, never the redirect target.
    pub key: String,
    /// Observed fingerprint. Absent for `unknown`: invariant #7 says
    /// absence is `None`, never a sentinel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_fingerprint: Option<Fingerprint>,
    /// Observed trust class. Absent for `unknown`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_class: Option<TrustClass>,
    /// Verdict.
    pub result: ProbeResultKind,
    /// Present iff `result == Unknown`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

impl ProbeCheckedPayload {
    /// Project a [`ProbeResult`] into the event payload.
    ///
    /// `key` must be the original dependency key.
    #[must_use]
    pub fn from_result(scheme: &str, key: &str, result: &ProbeResult) -> Self {
        let (observed_fingerprint, trust_class, kind, retryable) = match result {
            ProbeResult::Match {
                observed_fp,
                observed_trust_class,
            } => (
                Some(observed_fp.clone()),
                Some(*observed_trust_class),
                ProbeResultKind::Match,
                None,
            ),
            ProbeResult::Drift {
                observed_fp,
                observed_trust_class,
            } => (
                Some(observed_fp.clone()),
                Some(*observed_trust_class),
                ProbeResultKind::Drift,
                None,
            ),
            ProbeResult::Unknown { retryable, .. } => {
                (None, None, ProbeResultKind::Unknown, Some(*retryable))
            }
        };
        Self {
            scheme: scheme.to_string(),
            key: key.to_string(),
            observed_fingerprint,
            trust_class,
            result: kind,
            retryable,
        }
    }

    /// Is the `retryable` field present exactly when it must be?
    ///
    /// The engine may assert this on ingest; the probe asserts it in
    /// tests.
    #[must_use]
    pub const fn retryable_field_is_wellformed(&self) -> bool {
        match self.result {
            ProbeResultKind::Unknown => self.retryable.is_some(),
            ProbeResultKind::Match | ProbeResultKind::Drift => self.retryable.is_none(),
        }
    }

    /// Serialize to the JSON value that goes in `IrEvent::payload`.
    ///
    /// # Errors
    ///
    /// `serde_json::Error` if serialization fails (it cannot for this
    /// shape, but the signature stays honest).
    pub fn to_json_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }
}
