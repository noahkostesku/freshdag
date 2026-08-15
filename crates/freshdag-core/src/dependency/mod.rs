//! Dependency, Fingerprint, TrustClass, Validity — the trust-class-typed
//! primitives that are FreshDAG's load-bearing contribution
//! (see `docs/NOVELTY.md §2`).
//!
//! Contract: `docs/contracts/certificate-contract.md §Field Rules` and
//! `docs/contracts/probe-contract.md §Trust-class Semantics`.

mod fingerprint;
mod trust;
mod validity;

pub use fingerprint::{Fingerprint, FingerprintKind, FingerprintParseError};
pub use trust::TrustClass;
pub use validity::{
    EdgeVerdict, Validity, ValidityAggregationError, ValidityReason, ValidityStatus,
};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ir::Hash;

/// A stable, opaque identifier for a dependency instance. Derived from
/// `(scheme, key)` — the same underlying dependency observed twice yields
/// the same `DependencyId`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DependencyId(pub String);

impl DependencyId {
    /// Derive a `DependencyId` from a scheme + key pair.
    ///
    /// v0 identity is the wire-form string `scheme://key` (or `scheme(key)`
    /// for scheme-less forms like `web.search(...)`). This is what appears
    /// in `depends_on[].key` on certificates — see certificate contract.
    ///
    /// If `key` is already in `scheme://…` (or `scheme:…`) form, it is
    /// returned verbatim. The check is against the full scheme prefix
    /// including the `:` delimiter — not just `starts_with(scheme)` —
    /// so `from_scheme_key("file", "filesystem/x")` correctly returns
    /// `"file://filesystem/x"`, not `"filesystem/x"`.
    #[must_use]
    pub fn from_scheme_key(scheme: &str, key: &str) -> Self {
        let with_slashes = format!("{scheme}://");
        let with_colon = format!("{scheme}:");
        if key.starts_with(&with_slashes) || key.starts_with(&with_colon) {
            Self(key.to_string())
        } else {
            Self(format!("{scheme}://{key}"))
        }
    }
}

impl core::fmt::Display for DependencyId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A dependency observed by a computation: what it points at, what state
/// was recorded, and how confidently we can talk about that state.
///
/// A `Dependency` is what appears in `Certificate::depends_on`.
/// Serialized shape matches `schemas/certificate/v0.1.json`
/// `depends_on[]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// Wire-form dependency identity (matches `depends_on[].key`).
    pub key: String,
    /// Scheme portion (e.g., `file`, `https`, `attio`, `mcp`).
    pub scheme: String,
    /// Trust class of the recorded fingerprint.
    pub trust_class: TrustClass,
    /// Observed state of the dependency.
    pub fingerprint: Fingerprint,
    /// When the fingerprint was observed.
    #[serde(with = "time::serde::rfc3339")]
    pub observed_at: OffsetDateTime,
    /// If this dependency points at another FreshDAG-produced artifact,
    /// its `ArtifactId`. `None` for leaf dependencies (files, external
    /// sources).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<crate::artifact::ArtifactId>,
    /// TTL applicable to `volatile` dependencies (seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
}

impl Dependency {
    /// Machine-check: a dependency whose trust class is `volatile` must
    /// carry a TTL. The certificate contract forbids naked-volatile.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        !(matches!(self.trust_class, TrustClass::Volatile) && self.ttl_seconds.is_none())
    }

    /// Convenience: derive the stable dependency ID.
    #[must_use]
    pub fn id(&self) -> DependencyId {
        DependencyId::from_scheme_key(&self.scheme, &self.key)
    }
}

/// Convenience constructor: an `exact` file dependency with a BLAKE3
/// content hash. Prefer this at call sites so trust-class-preserving
/// invariants are met at construction, not by convention.
#[must_use]
pub fn exact_file(path: &str, content_hash: &Hash, observed_at: OffsetDateTime) -> Dependency {
    Dependency {
        key: format!("file://{path}"),
        scheme: "file".to_string(),
        trust_class: TrustClass::Exact,
        fingerprint: Fingerprint::new(FingerprintKind::ContentHash, content_hash.to_string()),
        observed_at,
        produced_by: None,
        ttl_seconds: None,
    }
}
