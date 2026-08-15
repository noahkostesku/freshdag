//! Trust classes for dependency fingerprints.
//!
//! See ADR 0004 and `docs/contracts/certificate-contract.md §Field Rules`.
//! The rules embodied in code:
//!
//! - `Heuristic` and `Volatile` cannot aggregate to `Valid`; the highest
//!   achievable status is `LikelyValid`. Enforced in
//!   [`crate::dependency::Validity::aggregate`].
//! - Trust class ordering is meaningful: `Exact > Versioned > Heuristic >
//!   Volatile`. Escalation goes up; demotion goes down and requires an
//!   explicit `probe.trust_demoted` diagnostic per the probe contract's
//!   anti-thrash protocol.

use serde::{Deserialize, Serialize};

/// The four trust classes for dependency fingerprints.
///
/// Wire form is lowercase (`"exact"`, `"versioned"`, `"heuristic"`,
/// `"volatile"`) — matches `schemas/certificate/v0.1.json`
/// `depends_on[].trust_class`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustClass {
    /// Content-addressed. Two dependencies with the same fingerprint are
    /// byte-identical.
    Exact,
    /// A trustworthy monotonic identifier is available from the source.
    /// Two dependencies with the same version token are asserted equal
    /// by the source (ETag, Attio record version, etc.).
    Versioned,
    /// A cheap signal that usually implies equality but can be wrong
    /// (mtime, weak ETag, Last-Modified).
    Heuristic,
    /// The source has no trustworthy freshness signal. Freshness is only
    /// asserted for the duration of a declared TTL.
    Volatile,
}

impl TrustClass {
    /// Ordinal for comparison (higher is stronger).
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Exact => 4,
            Self::Versioned => 3,
            Self::Heuristic => 2,
            Self::Volatile => 1,
        }
    }

    /// Would `self` be an escalation of trust from `previous`?
    ///
    /// Escalation is allowed but requires N=2 consecutive observations
    /// per the probe contract's anti-thrash protocol; this predicate is
    /// the "is escalation" test, not the "should adopt" decision.
    #[must_use]
    pub fn is_escalation_over(self, previous: Self) -> bool {
        self.rank() > previous.rank()
    }

    /// Would `self` be a demotion from `previous`?
    ///
    /// Demotion is never silent — see probe contract §Anti-thrash.
    #[must_use]
    pub fn is_demotion_from(self, previous: Self) -> bool {
        self.rank() < previous.rank()
    }
}

impl PartialOrd for TrustClass {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TrustClass {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}
