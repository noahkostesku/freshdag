//! Probe interface — the trait every probe implementation implements.
//!
//! Traits live in `freshdag-core` (not `freshdag-probes`) because the
//! engine consumes probe results and the engine depends only on core.
//! Implementations live in `freshdag-probes/` (or scheme-specific
//! crates layered on top).
//!
//! Contract: `docs/contracts/probe-contract.md`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::dependency::{Fingerprint, TrustClass};

/// The result of a single probe check.
///
/// Invariant #7 is enforced at the type level: there is no variant
/// that expresses "unknown but treat as fresh." A probe that cannot
/// verify must return [`ProbeResult::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeResult {
    /// Recorded fingerprint still matches.
    Match {
        /// The observed fingerprint (usually equal to the recorded one,
        /// but returned explicitly so trust escalation is representable).
        observed_fp: Fingerprint,
        /// The trust class of the observation. May escalate over the
        /// recorded class per probe-contract §Anti-thrash Protocol.
        observed_trust_class: TrustClass,
    },
    /// Recorded fingerprint no longer matches.
    Drift {
        /// The newly observed fingerprint.
        observed_fp: Fingerprint,
        /// The trust class of the observation.
        observed_trust_class: TrustClass,
    },
    /// Could not verify — probe MUST return this on any failure that
    /// might otherwise be mis-classified as `Match`.
    Unknown {
        /// Human-readable reason (goes on the certificate's `reasons`).
        reason: String,
        /// Whether the caller may retry (network failures) or must not
        /// (endpoint misconfiguration).
        retryable: bool,
    },
}

/// The interface every probe implements.
///
/// Probes are strictly read-only: they may not mutate external state.
/// Blocking is acceptable — probes are called from a dedicated worker
/// pool by the engine.
pub trait Probe: Send + Sync {
    /// The scheme this probe handles (`file`, `https`, `attio`, `mcp`,
    /// `postgres`, ...).
    fn scheme(&self) -> &str;

    /// Optional host pattern for arbitration among multiple probes
    /// handling the same scheme (see probe-contract §Probe Arbitration).
    fn host_pattern(&self) -> Option<&str> {
        None
    }

    /// Priority within `(scheme, host_pattern)`. Higher wins. Ties are
    /// contract violations that the engine surfaces as `diagnostic`.
    fn priority(&self) -> u32 {
        0
    }

    /// Check whether the dependency is still at the recorded
    /// fingerprint. Blocking; the engine calls this from a worker pool.
    ///
    /// # Arguments
    ///
    /// - `key` is the scheme-specific opaque key (e.g., `/abs/path.md`
    ///   for `file`, `https://acme.com/pricing` for `https`).
    /// - `recorded_fp` is the fingerprint on the certificate; the probe
    ///   compares against this.
    /// - `ttl_hint` is passed for `volatile` dependencies; probes may
    ///   consult it to short-circuit within a TTL window.
    ///
    /// # Errors
    ///
    /// A probe never returns `Err`: every failure mode maps to
    /// [`ProbeResult::Unknown`] with a reason. This is a contract-level
    /// rule enforced by returning `ProbeResult` (not `Result<...>`) —
    /// see probe-contract §Failure Modes.
    fn check(
        &self,
        key: &str,
        recorded_fp: &Fingerprint,
        ttl_hint: Option<Duration>,
    ) -> ProbeResult;
}
