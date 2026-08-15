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

/// The closed set of machine-checkable reasons a dependency edge can
/// contribute to a non-`Valid` status.
///
/// Wire form is kebab-case, matching [`ValidityStatus`] and
/// `schemas/certificate/v0.1.json` (`status.reasons[].reason`). The set
/// is deliberately closed: invariant #6 requires every skip/reuse
/// decision be explainable, and invariant #13 requires that
/// explanation be testable rather than free text. Downstream consumers
/// (engine, CLI renderer, scenario harness) key behaviour off these
/// variants, never off prose.
///
/// Codes are either **edge-scoped** (they explain one entry of
/// `depends_on[]`, and `dependency_key` names it) or **artifact-scoped**
/// (they explain the certificate as a whole, and `dependency_key` is the
/// empty string `""` — never `null`, never omitted). See
/// [`ReasonCode::is_artifact_scoped`].
///
/// Three FreshDAG vocabularies share spellings and must not be
/// conflated: reason codes (this enum), probe results
/// ([`ProbeResult`](crate::probe::ProbeResult) /
/// `probe.checked.result`), and validity statuses
/// ([`ValidityStatus`]). `drift` and `unknown` appear in more than one
/// of them and mean different things in each.
///
/// Adding a variant is a contract change — see
/// `.claude/rules/architecture.md` — because it widens the schema enum
/// consumers validate against, even though it is additive to the Rust
/// enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasonCode {
    /// Edge-scoped. A probe observed a fingerprint different from the
    /// recorded one.
    Drift,
    /// Edge-scoped. A probe **ran** and could not decide. Per invariant
    /// #7 this is never treated as fresh.
    ///
    /// Contrast [`ReasonCode::NoProbeAvailable`], which asserts no probe
    /// ran at all. Reporting `ProbeUnknown` when no probe existed is a
    /// false statement to a user reading `freshdag why`.
    ProbeUnknown,
    /// Edge-scoped. No probe could be selected: none registered for the
    /// scheme, registration failed, arbitration tied, or the probe that
    /// recorded the fingerprint was removed (probe-contract
    /// §Anti-thrash Protocol, "Probe removal").
    NoProbeAvailable,
    /// Edge-scoped. The edge matched, but at `Heuristic` trust;
    /// invariant #8 forbids reporting it as `Valid`, so the artifact
    /// status is capped at `LikelyValid`.
    TrustClassHeuristicCapsAtLikelyValid,
    /// Edge-scoped. The edge matched inside its TTL at `Volatile`
    /// trust, so the artifact status is capped at `LikelyValid`.
    TrustClassVolatileCapsAtLikelyValid,
    /// Edge-scoped. A `Volatile` dependency's TTL elapsed without
    /// re-observation.
    TtlExpired,
    /// Edge-scoped. The signal backing the recorded trust class
    /// disappeared; the engine emitted a `probe.trust_demoted`
    /// diagnostic and forced re-observation.
    ProbeTrustDemoted,
    /// Artifact-scoped. The computation exhibited effects no producer
    /// in `observation_coverage` claims to cover (certificate contract
    /// §Coverage-Deficit Rule).
    CoverageDeficit,
    /// Artifact-scoped. An event in the stream names a producer absent
    /// from `observation_coverage`, so its silences cannot be
    /// interpreted.
    ProducerMissingFromCoverage,
    /// Artifact-scoped. The computation produced an artifact with zero
    /// observed dependencies — absence of evidence is not evidence of
    /// freshness.
    NoDependenciesObserved,
}

impl ReasonCode {
    /// The canonical wire string for this reason code.
    ///
    /// The `match` below is exhaustive by construction, so adding a
    /// variant fails to COMPILE until this table is updated. The
    /// `reason_code_serde_and_as_wire_str_agree` and
    /// `schema_reason_enums_match_rust` tests in
    /// `crates/freshdag-core/src/tests.rs` then keep this table, serde,
    /// and both JSON schemas from drifting apart.
    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Drift => "drift",
            Self::ProbeUnknown => "probe-unknown",
            Self::NoProbeAvailable => "no-probe-available",
            Self::TrustClassHeuristicCapsAtLikelyValid => {
                "trust-class-heuristic-caps-at-likely-valid"
            }
            Self::TrustClassVolatileCapsAtLikelyValid => {
                "trust-class-volatile-caps-at-likely-valid"
            }
            Self::TtlExpired => "ttl-expired",
            Self::ProbeTrustDemoted => "probe-trust-demoted",
            Self::CoverageDeficit => "coverage-deficit",
            Self::ProducerMissingFromCoverage => "producer-missing-from-coverage",
            Self::NoDependenciesObserved => "no-dependencies-observed",
        }
    }

    /// Does this code explain the certificate as a whole rather than
    /// one `depends_on[]` edge?
    ///
    /// Artifact-scoped reasons carry `dependency_key: ""` and sort
    /// after every edge-scoped reason in `status.reasons[]`. That
    /// ordering is contractual — `cert_id` hashes it and fixtures pin
    /// `reasons[0]` (certificate contract §Reason Codes, "Ordering").
    #[must_use]
    pub const fn is_artifact_scoped(self) -> bool {
        match self {
            Self::CoverageDeficit
            | Self::ProducerMissingFromCoverage
            | Self::NoDependenciesObserved => true,
            Self::Drift
            | Self::ProbeUnknown
            | Self::NoProbeAvailable
            | Self::TrustClassHeuristicCapsAtLikelyValid
            | Self::TrustClassVolatileCapsAtLikelyValid
            | Self::TtlExpired
            | Self::ProbeTrustDemoted => false,
        }
    }
}

impl core::fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_wire_str())
    }
}

/// A reason for a non-`Valid` status, for `status.reasons[]` in the
/// certificate (see certificate-contract.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidityReason {
    /// Dependency key this reason refers to.
    ///
    /// For an artifact-scoped reason (see
    /// [`ReasonCode::is_artifact_scoped`]) this is the empty string
    /// `""` — the sentinel, never `null` and never omitted, so the
    /// schema's `required` list stays unchanged.
    pub dependency_key: String,
    /// Machine-parseable reason code. This — and only this — is what
    /// downstream decisions key off.
    pub reason: ReasonCode,
    /// Human context for the reason (probe failure text, HTTP status,
    /// rate-limit description, ...).
    ///
    /// `detail` is NEVER load-bearing for a decision. No consumer —
    /// engine, CLI, UI, or integration — may branch on its contents,
    /// parse it, or treat its absence as meaningful; it exists purely
    /// so a human reading `freshdag why` learns *why* the probe said
    /// what it said. Decisions key off [`ValidityReason::reason`].
    ///
    /// Two further rules from certificate-contract §The `detail` field:
    /// `detail` MUST be deterministic (it is inside the `cert_id`
    /// preimage, so no elapsed times, timestamps, PIDs, ports, or retry
    /// counters), and it MUST NOT carry secrets (no credentials, no
    /// `Authorization` headers, no response bodies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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
    /// **Ordering is contractual.** Reasons are emitted in
    /// `dependency_keys` order — which the caller supplies in
    /// `depends_on[]` order — and artifact-scoped reasons (see
    /// [`ReasonCode::is_artifact_scoped`]) sort after every edge-scoped
    /// reason. `cert_id` hashes `status.reasons[]` and fixtures pin
    /// `reasons[0]`, so reordering is a wire-visible change.
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
                    reason: ReasonCode::NoDependenciesObserved,
                    detail: None,
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
                    reason: ReasonCode::Drift,
                    detail: None,
                }),
                EdgeVerdict::Unknown => reasons.push(ValidityReason {
                    dependency_key: key.clone(),
                    reason: ReasonCode::ProbeUnknown,
                    detail: None,
                }),
                EdgeVerdict::Match {
                    recorded_trust_class: TrustClass::Heuristic,
                    ..
                } => reasons.push(ValidityReason {
                    dependency_key: key.clone(),
                    reason: ReasonCode::TrustClassHeuristicCapsAtLikelyValid,
                    detail: None,
                }),
                EdgeVerdict::Match {
                    recorded_trust_class: TrustClass::Volatile,
                    ..
                } => reasons.push(ValidityReason {
                    dependency_key: key.clone(),
                    reason: ReasonCode::TrustClassVolatileCapsAtLikelyValid,
                    detail: None,
                }),
                EdgeVerdict::Match {
                    recorded_trust_class: TrustClass::Exact | TrustClass::Versioned,
                    ..
                } => {}
            }
        }
        // Contractual ordering: edge-scoped reasons in depends_on[]
        // order, artifact-scoped reasons last. `sort_by_key` is stable,
        // so the edge order established above is preserved.
        reasons.sort_by_key(|r| r.reason.is_artifact_scoped());

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
