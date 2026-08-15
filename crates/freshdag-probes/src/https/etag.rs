//! Entity-tag parsing, per RFC 9110 §8.8.3.
//!
//! ```text
//! entity-tag = [ weak ] opaque-tag
//! weak       = %s"W/"                      ; case-sensitive
//! opaque-tag = DQUOTE *etagc DQUOTE
//! etagc      = %x21 / %x23-7E / obs-text   ; VCHAR except DQUOTE, plus obs-text
//! ```
//!
//! The strength distinction is the whole reason this module exists.
//! RFC 9110 §8.8.3.2 defines *weak* equivalence at the entity level:
//! two representations with the same weak validator may differ octet by
//! octet (a gzip variant, a whitespace-normalized document, a
//! transcoded image). FreshDAG's `exact` comparators are defined on
//! octets, so a weak ETag can only ever justify
//! [`TrustClass::Heuristic`](freshdag_core::dependency::TrustClass::Heuristic).
//! Mapping `W/"..."` to `versioned` would be a silent trust promotion —
//! invariant #8.

/// A successfully parsed entity-tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ETag {
    /// `true` for `W/"..."` (weak validator).
    pub weak: bool,
    /// The opaque tag *including* its surrounding double quotes, i.e.
    /// the exact bytes to echo back in `If-None-Match`.
    pub quoted: String,
}

impl ETag {
    /// The full wire form (`"abc"` or `W/"abc"`) — what we send in
    /// `If-None-Match` and what we record as the fingerprint payload.
    #[must_use]
    pub fn wire(&self) -> String {
        if self.weak {
            format!("W/{}", self.quoted)
        } else {
            self.quoted.clone()
        }
    }

    /// Strong comparison (RFC 9110 §8.8.3.2): both tags must be strong
    /// and octet-identical.
    ///
    /// Used for nothing today — the probe compares full wire forms —
    /// but kept explicit so a future caller cannot accidentally reach
    /// for `==` and get weak-equivalence semantics.
    #[must_use]
    pub fn strongly_equals(&self, other: &Self) -> bool {
        !self.weak && !other.weak && self.quoted == other.quoted
    }
}

/// Why an `ETag` header value could not be parsed.
///
/// A malformed ETag is never coerced into a usable validator. It makes
/// the ETag *unavailable*, which downgrades what the probe can conclude
/// (usually to `Unknown`) rather than downgrading what it claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ETagParseError {
    /// Value was empty or whitespace only.
    #[error("empty")]
    Empty,
    /// Not wrapped in double quotes (e.g. bare `abc123`, which Apache
    /// and several CDNs have shipped at one time or another).
    #[error("unquoted")]
    Unquoted,
    /// A `"` appeared inside the opaque tag, or a control character
    /// did.
    #[error("illegal-character")]
    IllegalCharacter,
    /// A weakness prefix that is not `W/` (e.g. `X/"abc"`).
    #[error("bad-weak-prefix")]
    BadWeakPrefix,
}

/// Parse a single `ETag` header field value.
///
/// Deliberately strict:
///
/// - `"abc"` → strong.
/// - `W/"abc"` → weak.
/// - `w/"abc"` → weak. RFC 9110 spells the prefix case-sensitively, but
///   lowercase `w/` occurs in the wild and its *intent* is unambiguous.
///   Accepting it as **weak** is the conservative reading: the failure
///   mode of rejecting it is a lost signal, the failure mode of reading
///   it as strong would be a fabricated `versioned` claim.
/// - `abc` (unquoted) → [`ETagParseError::Unquoted`]. We do not repair
///   it by adding quotes; an unquoted tag came from a server that is
///   not implementing RFC 9110, and guessing its intent is how trust
///   laundering starts.
/// - `"ab"cd"` → [`ETagParseError::IllegalCharacter`].
///
/// # Errors
///
/// [`ETagParseError`] for any value not matching `entity-tag`.
pub fn parse_etag(raw: &str) -> Result<ETag, ETagParseError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(ETagParseError::Empty);
    }

    let (weak, opaque) = if let Some(rest) = value.strip_prefix("W/") {
        (true, rest)
    } else if let Some(rest) = value.strip_prefix("w/") {
        (true, rest)
    } else if let Some((prefix, rest)) = value.split_once('/') {
        // Something like `X/"abc"`. Only distinguish it from a plain
        // unquoted tag when the suffix at least looks like an
        // opaque-tag; otherwise `abc/def` is just unquoted junk.
        if prefix.len() == 1 && rest.starts_with('"') {
            return Err(ETagParseError::BadWeakPrefix);
        }
        (false, value)
    } else {
        (false, value)
    };

    if opaque.len() < 2 || !opaque.starts_with('"') || !opaque.ends_with('"') {
        return Err(ETagParseError::Unquoted);
    }

    let inner = &opaque[1..opaque.len() - 1];
    // etagc = %x21 / %x23-7E / obs-text(%x80-FF). Notably excludes
    // DQUOTE (%x22), SP, HTAB, and all controls.
    if !inner.bytes().all(|b| b == 0x21 || (0x23..=0x7E).contains(&b) || b >= 0x80) {
        return Err(ETagParseError::IllegalCharacter);
    }

    Ok(ETag {
        weak,
        quoted: opaque.to_string(),
    })
}
