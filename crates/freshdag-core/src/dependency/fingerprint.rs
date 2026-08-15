//! Fingerprints — the observed state of a dependency.
//!
//! A `Fingerprint` is a trust-class-tagged string in the wire form
//! `<kind>:<payload>` (e.g., `blake3:abc...`, `etag:"abc123"`,
//! `version:42`, `mtime:1690000000`). See
//! `docs/contracts/certificate-contract.md §Field Rules`.
//!
//! Absence is represented in the containing `Option<Fingerprint>`,
//! never by a sentinel value inside `Fingerprint`. Invariant #7.

use std::str::FromStr;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// The kind of fingerprint (drives interpretation of the payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FingerprintKind {
    /// A content hash (`blake3:...`, `sha256:...`). Combined with the
    /// hash algorithm encoded in the payload.
    ContentHash,
    /// A source-provided version token (`version:42`).
    Version,
    /// An HTTP ETag (`etag:"..."`).
    Etag,
    /// A last-modified timestamp (`mtime:<rfc3339>` or `mtime:<unix>`).
    Mtime,
    /// A custom scheme-specific fingerprint. Payload interpretation is
    /// up to the probe that produced it.
    Custom,
}

impl FingerprintKind {
    /// Wire-format prefix (before the `:`).
    #[must_use]
    pub const fn as_wire_prefix(self) -> &'static str {
        match self {
            Self::ContentHash => "blake3", // default; probes may use sha256
            Self::Version => "version",
            Self::Etag => "etag",
            Self::Mtime => "mtime",
            Self::Custom => "custom",
        }
    }
}

/// The observed state of a dependency, in wire form `<kind>:<payload>`.
///
/// Fingerprint equality is byte-equality of `to_string()`. Two
/// fingerprints of different kinds are never equal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint {
    /// The kind of fingerprint (used for interpretation, not comparison).
    pub kind: FingerprintKind,
    /// The wire payload after the prefix (e.g., `abc123`, `"etag123"`).
    pub payload: String,
}

impl Fingerprint {
    /// Construct a fingerprint. Payload MUST be non-empty — an empty
    /// payload would be indistinguishable from "unknown," which
    /// invariant #7 rules out. Panics on empty payload in ALL build
    /// modes (not `debug_assert!`); an empty payload here would be a
    /// silent invariant violation.
    #[must_use]
    pub fn new(kind: FingerprintKind, payload: impl Into<String>) -> Self {
        let payload = payload.into();
        assert!(
            !payload.is_empty(),
            "empty Fingerprint payload violates invariant #7"
        );
        Self { kind, payload }
    }
}

impl core::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Special-case content-hash: the payload is already `algo:digest`.
        match self.kind {
            FingerprintKind::ContentHash => f.write_str(&self.payload),
            _ => write!(f, "{}:{}", self.kind.as_wire_prefix(), self.payload),
        }
    }
}

impl FromStr for Fingerprint {
    type Err = FingerprintParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(FingerprintParseError::Empty);
        }
        let (prefix, rest) = s
            .split_once(':')
            .ok_or_else(|| FingerprintParseError::Malformed(s.to_string()))?;
        // Empty prefix (":something") is malformed — not a Custom
        // fingerprint. Same for empty payload.
        if prefix.is_empty() {
            return Err(FingerprintParseError::Malformed(s.to_string()));
        }
        if rest.is_empty() {
            return Err(FingerprintParseError::Empty);
        }
        // "unknown" (any casing) MUST NOT parse — invariant #7. Compare
        // case-insensitively so `UNKNOWN:x`, `Unknown:x`, etc. are
        // rejected too.
        if prefix.eq_ignore_ascii_case("unknown") {
            return Err(FingerprintParseError::UnknownIsNotFingerprint);
        }
        let (kind, payload) = match prefix {
            "blake3" | "sha256" => (FingerprintKind::ContentHash, s.to_string()),
            "version" => (FingerprintKind::Version, rest.to_string()),
            "etag" => (FingerprintKind::Etag, rest.to_string()),
            "mtime" => (FingerprintKind::Mtime, rest.to_string()),
            // "custom" and any unknown-but-parseable prefix fall through
            // to the Custom variant so probes may define new schemes
            // without a code change here.
            _ => (FingerprintKind::Custom, rest.to_string()),
        };
        Ok(Self { kind, payload })
    }
}

impl Serialize for Fingerprint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

/// Errors from parsing a `Fingerprint`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FingerprintParseError {
    /// Wire form was empty.
    #[error("empty fingerprint (absence must be Option::None, not empty payload; invariant #7)")]
    Empty,
    /// Wire form was missing the `kind:` prefix.
    #[error("malformed fingerprint (expected `<kind>:<payload>`): {0}")]
    Malformed(String),
    /// Reserved: the token `unknown` MUST NOT parse as a fingerprint —
    /// per invariant #7, unknown is `Option::None`, not a Fingerprint value.
    #[error("`unknown` is not a valid fingerprint; use Option::None to represent unknown state")]
    UnknownIsNotFingerprint,
}
