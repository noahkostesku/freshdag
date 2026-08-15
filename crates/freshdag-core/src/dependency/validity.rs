//! Validity — the aggregation of per-dependency freshness signals into a
//! per-artifact status.
//!
//! See `ARCHITECTURE.md §7` for the aggregation rules and
//! `docs/contracts/certificate-contract.md §Field Rules` for the wire
//! form (`status.value`).

use serde::{Deserialize, Serialize};

use super::fingerprint::Fingerprint;
use super::trust::TrustClass;

/// The four possible values for a `Certificate.status.value`.
///
/// Wire form is kebab-case (matches schemas/certificate/v0.1.json).
///
/// Invariant #7 forbids `Valid` from any code path where evidence is
/// `Unknown`. [`Validity::aggregate`] is the load-bearing enforcement
/// point: it never returns `Valid` unless every input edge is
/// unambiguously fresh AND no edge carries a `Heuristic` or `Volatile`
/// trust class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValidityStatus {
    /// All dependencies unambiguously fresh at trust class `Exact` or
    /// `Versioned`. This is the strongest possible status.
    Valid,
    /// All dependencies fresh, but at least one carries a `Heuristic`
    /// or `Volatile` trust class — so we cannot promise byte-identity,
    /// only "very likely still what it was."
    LikelyValid,
    /// At least one dependency drifted.
    Stale,
    /// At least one dependency could not be verified (probe failed,
    /// coverage-deficit, TTL expired without re-observation, ...).
    /// This is the default in the face of missing evidence.
    Unknown,
}

impl ValidityStatus {
    /// Rank for aggregation (lower rank wins ties in "strictly worse"
    /// aggregation). Ordering: `Stale < Unknown < LikelyValid < Valid`.
    #[must_use]
    const fn rank(self) -> u8 {
        match self {
            Self::Stale => 0,
            Self::Unknown => 1,
            Self::LikelyValid => 2,
            Self::Valid => 3,
        }
    }
}

/// A per-edge validity signal produced by validity evaluation.
///
/// The engine calls a probe for each dependency edge; the probe returns
/// one of these three via
/// [`ProbeResult`](crate::probe::ProbeResult) which the engine converts
/// to an `EdgeVerdict` combining probe result and recorded trust class.
///
/// `Match` carries the *observed* fingerprint and trust class alongside
/// the *recorded* trust class so downstream logic (anti-thrash escalation
/// per probe-contract §Anti-thrash Protocol, writing the new fingerprint
/// back into the store) has the data without a follow-up probe call.
/// Widening this shape here avoids a public-API break when Wave 2's
/// engine lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeVerdict {
    /// Recorded fingerprint still matches.
    Match {
        /// The recorded trust class.
        recorded_trust_class: TrustClass,
        /// The observed trust class (may differ if the probe escalates).
        observed_trust_class: TrustClass,
        /// The observed fingerprint from the probe.
        observed_fp: Fingerprint,
    },
    /// Recorded fingerprint no longer matches.
    Drift,
    /// Could not verify (probe failure, TTL expired, coverage deficit).
    Unknown,
}

impl EdgeVerdict {
    /// Construct a `Match` from just a trust class + observed fingerprint,
    /// treating recorded and observed trust as the same.
    #[must_use]
    pub fn matched(trust_class: TrustClass, observed_fp: Fingerprint) -> Self {
        Self::Match {
            recorded_trust_class: trust_class,
            observed_trust_class: trust_class,
            observed_fp,
        }
    }

    /// Convert an edge verdict to its `ValidityStatus` contribution.
    ///
    /// This encodes the trust-class table from `ARCHITECTURE.md §7`:
    /// `Exact|Versioned + Match => Valid`; `Heuristic|Volatile + Match
    /// => LikelyValid`; anything `Drift => Stale`; anything `Unknown =>
    /// Unknown`.
    ///
    /// The verdict's status contribution uses the *recorded* trust
    /// class — escalations require multiple observations before they
    /// change the recorded class (anti-thrash), so a single Match at a
    /// higher observed trust class does not immediately upgrade the
    /// artifact's status.
    #[must_use]
    pub const fn to_status(&self) -> ValidityStatus {
        match self {
            Self::Match {
                recorded_trust_class: TrustClass::Exact | TrustClass::Versioned,
                ..
            } => ValidityStatus::Valid,
            Self::Match {
                recorded_trust_class: TrustClass::Heuristic | TrustClass::Volatile,
                ..
            } => ValidityStatus::LikelyValid,
            Self::Drift => ValidityStatus::Stale,
            Self::Unknown => ValidityStatus::Unknown,
        }
    }
}

/// A reason for a non-`Valid` status, for `status.reasons[]` in the
/// certificate (see certificate-contract.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidityReason {
    /// Dependency key this reason refers to.
    pub dependency_key: String,
    /// Machine-parseable reason code (e.g., `"drift"`, `"probe_unknown"`,
    /// `"ttl_expired"`, `"coverage_deficit"`).
    pub reason: String,
}

/// A validity determination for an artifact — status plus per-edge
/// reasons (populated whenever status ≠ Valid, per certificate contract).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validity {
    /// The aggregated status.
    pub value: ValidityStatus,
    /// Reasons when `value != Valid`; MUST be non-empty in that case
    /// (certificate contract). Empty and absent are equivalent for
    /// `Valid`.
    #[serde(default)]
    pub reasons: Vec<ValidityReason>,
}

impl Validity {
    /// Aggregate per-edge verdicts into an artifact-level validity.
    ///
    /// Rules (from ARCHITECTURE §7):
    /// - Any `Drift` → `Stale`.
    /// - Else any `Unknown` → `Unknown`.
    /// - Else any `LikelyValid` contribution → `LikelyValid`.
    /// - Else all `Valid` contributions → `Valid`.
    ///
    /// Empty verdict slice → `Unknown` (no evidence, cannot be Valid).
    ///
    /// Reasons are attached to non-`Valid` statuses only. Each
    /// `Drift`/`Unknown` edge contributes a reason keyed by the
    /// `dependency_key` the caller supplied.
    ///
    /// # Errors
    ///
    /// Returns [`ValidityAggregationError::MismatchedLengths`] if the
    /// verdict and dependency-key slices differ in length.
    pub fn aggregate(
        verdicts: &[EdgeVerdict],
        dependency_keys: &[String],
    ) -> Result<Self, ValidityAggregationError> {
        if verdicts.len() != dependency_keys.len() {
            return Err(ValidityAggregationError::MismatchedLengths {
                verdicts: verdicts.len(),
                keys: dependency_keys.len(),
            });
        }
        if verdicts.is_empty() {
            return Ok(Self {
                value: ValidityStatus::Unknown,
                reasons: vec![ValidityReason {
                    dependency_key: String::new(),
                    reason: "no_dependencies_observed".to_string(),
                }],
            });
        }

        // The invariant #7 keystone: fold with the strict-minimum rank.
        let mut status = ValidityStatus::Valid;
        let mut reasons: Vec<ValidityReason> = Vec::new();
        for (v, key) in verdicts.iter().zip(dependency_keys.iter()) {
            let contribution = v.to_status();
            if contribution.rank() < status.rank() {
                status = contribution;
            }
            match v {
                EdgeVerdict::Drift => reasons.push(ValidityReason {
                    dependency_key: key.clone(),
                    reason: "drift".to_string(),
                }),
                EdgeVerdict::Unknown => reasons.push(ValidityReason {
                    dependency_key: key.clone(),
                    reason: "probe_unknown".to_string(),
                }),
                EdgeVerdict::Match {
                    recorded_trust_class: TrustClass::Heuristic,
                    ..
                } => reasons.push(ValidityReason {
                    dependency_key: key.clone(),
                    reason: "trust_class_heuristic_caps_at_likely_valid".to_string(),
                }),
                EdgeVerdict::Match {
                    recorded_trust_class: TrustClass::Volatile,
                    ..
                } => reasons.push(ValidityReason {
                    dependency_key: key.clone(),
                    reason: "trust_class_volatile_caps_at_likely_valid".to_string(),
                }),
                EdgeVerdict::Match {
                    recorded_trust_class: TrustClass::Exact | TrustClass::Versioned,
                    ..
                } => {}
            }
        }
        // Only surface reasons for non-Valid statuses.
        if matches!(status, ValidityStatus::Valid) {
            reasons.clear();
        }
        Ok(Self {
            value: status,
            reasons,
        })
    }
}

/// Errors from [`Validity::aggregate`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidityAggregationError {
    /// Caller passed verdict and dependency-key slices of different lengths.
    #[error("verdict/key slice length mismatch: {verdicts} verdicts vs {keys} keys")]
    MismatchedLengths {
        /// Number of verdicts supplied.
        verdicts: usize,
        /// Number of dependency keys supplied.
        keys: usize,
    },
}
