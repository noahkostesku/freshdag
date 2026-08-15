//! Redirect policy for the `https://` probe.
//!
//! Policy (from `docs/research/http-validity-probes.md` §Redirect
//! handling):
//!
//! - Follow at most `max_redirects` hops.
//! - **Never** follow `https:` → `http:`. A redirect that downgrades
//!   the transport is indistinguishable from an active downgrade
//!   attack, and a freshness signal read over a downgraded connection
//!   is not evidence of anything.
//! - Cross-origin redirects are followed but reported: the caller's
//!   dependency URL has effectively been rewritten upstream and they
//!   should know.
//! - The dependency key stays the ORIGINAL URL. The redirect target is
//!   metadata, never identity.

use reqwest::Url;

/// Why a redirect was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RedirectRefusal {
    /// `3xx` with no (or empty) `Location`.
    #[error("redirect-without-location")]
    NoLocation,
    /// `Location` did not resolve to an absolute URL.
    #[error("redirect-location-unparseable")]
    UnparseableLocation,
    /// `https:` → `http:`. Refused unconditionally.
    #[error("cross-scheme-downgrade-refused")]
    SchemeDowngrade,
    /// `Location` pointed at a scheme this probe does not speak.
    #[error("redirect-unsupported-scheme")]
    UnsupportedScheme,
    /// Hop budget exhausted.
    #[error("redirect-chain-too-deep")]
    TooDeep,
}

impl RedirectRefusal {
    /// Stable reason token for the `Unknown` detail string. Contains no
    /// URL, host, port, or timing — see
    /// `docs/contracts/certificate-contract.md §The detail field`.
    #[must_use]
    pub const fn reason_token(&self) -> &'static str {
        match self {
            Self::NoLocation => "redirect-without-location",
            Self::UnparseableLocation => "redirect-location-unparseable",
            Self::SchemeDowngrade => "cross-scheme-downgrade-refused",
            Self::UnsupportedScheme => "redirect-unsupported-scheme",
            Self::TooDeep => "redirect-chain-too-deep",
        }
    }
}

/// Resolve the next hop of a redirect chain.
///
/// `location` may be relative; it is resolved against `current` per
/// RFC 9110 §10.2.2.
///
/// # Errors
///
/// [`RedirectRefusal`] when the hop must not be followed.
pub fn resolve_next_hop(current: &Url, location: Option<&str>) -> Result<Url, RedirectRefusal> {
    let location = location.map(str::trim).filter(|l| !l.is_empty());
    let Some(location) = location else {
        return Err(RedirectRefusal::NoLocation);
    };

    let next = current
        .join(location)
        .map_err(|_| RedirectRefusal::UnparseableLocation)?;

    match next.scheme() {
        "https" => Ok(next),
        "http" => {
            if current.scheme() == "https" {
                // The downgrade rule. Unconditional: it is not gated on
                // any config flag, because a probe configured to allow
                // plaintext for a loopback fixture must still refuse to
                // be walked down off TLS by a remote origin.
                Err(RedirectRefusal::SchemeDowngrade)
            } else {
                Ok(next)
            }
        }
        _ => Err(RedirectRefusal::UnsupportedScheme),
    }
}

/// Do two URLs share an origin (scheme, host, effective port)?
#[must_use]
pub fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}
