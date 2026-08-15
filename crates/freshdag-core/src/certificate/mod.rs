//! Validity certificates.
//!
//! Contract: `docs/contracts/certificate-contract.md`.
//! Schema:   `schemas/certificate/v0.1.json`.
//!
//! The `Certificate` type below matches the schema exactly on the wire.
//! Machine-enforced invariants (heuristic-cap, recipe-hash-required,
//! reasons-required) live in [`Certificate::check_invariants`] as
//! Rust-level assertions redundant with the schema's `allOf if/then`
//! rules. Both must hold; both must agree.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;

use crate::artifact::Artifact;
use crate::computation::ComputationId;
use crate::dependency::{Dependency, ReasonCode, TrustClass, ValidityReason, ValidityStatus};
use crate::ir::{CoverageManifest, EventKind, EventKindPattern, Hash, IrEvent, ProducerRole};

/// Schema identifier expected on every v0.1 certificate.
pub const CERTIFICATE_SCHEMA_V0_1: &str = "freshdag.certificate/v0.1";

/// A validity certificate. Serializes to the shape defined in
/// `schemas/certificate/v0.1.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Certificate {
    /// Content-addressed identifier of the certificate itself. Derived
    /// from the certificate's own bytes; see
    /// [`Certificate::derive_cert_id`].
    pub cert_id: Hash,
    /// Schema version identifier.
    pub schema: String,
    /// The artifact this certificate is for.
    pub artifact: Artifact,
    /// The computation that produced the artifact.
    pub produced_by: ProducedBy,
    /// Every dependency the computation observed.
    pub depends_on: Vec<Dependency>,
    /// Which comparator drives downstream equivalence checks. Optional;
    /// absent means `exact` (matches contract default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparator: Option<Comparator>,
    /// Current validity status + reasons.
    pub status: Status,
    /// Every producer that contributed to this certificate's evidence,
    /// so downstream consumers can interpret silences.
    pub observation_coverage: Vec<CoverageEntry>,
}

/// The `produced_by` subobject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProducedBy {
    /// Stable computation identifier. Typed as [`ComputationId`] rather
    /// than a bare `String` so certificates cannot silently carry an
    /// unparseable id and the deterministic-derivation guarantee from
    /// execution-ir.md §Event Envelope is preserved end-to-end.
    pub computation_id: ComputationId,
    /// Recipe name (human-readable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    /// Content hash of the recipe body. Required whenever
    /// `status.value` ∈ {`Valid`, `LikelyValid`} per certificate
    /// contract §Field Rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe_hash: Option<Hash>,
    /// Adapter identity: `"freshdag-adapter-<name>/<version>"`.
    pub adapter: String,
    /// When observation began.
    #[serde(with = "time::serde::rfc3339")]
    pub started: OffsetDateTime,
    /// When observation ended.
    #[serde(with = "time::serde::rfc3339")]
    pub ended: OffsetDateTime,
}

/// The `comparator` subobject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparator {
    /// Comparator name (`exact`, `json-structural`, `set`, `numeric`,
    /// `judge`, `custom`). No implementations ship in v0 — see
    /// `docs/contracts/comparator-contract.md`.
    pub name: String,
    /// Comparator-specific configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

/// The `status` subobject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Current validity status.
    pub value: ValidityStatus,
    /// When `check` last ran.
    #[serde(with = "time::serde::rfc3339")]
    pub checked: OffsetDateTime,
    /// Reasons — required non-empty when `value != Valid` (invariant #6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<ValidityReason>,
}

/// One row of the `observation_coverage` list.
///
/// The full [`CoverageManifest`] lives elsewhere; on the certificate we
/// keep the fields consumers need for silence-interpretation *and* the
/// `emits` patterns the engine needs to compute coverage deficits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageEntry {
    /// Producer identity (matches `IrEvent::producer`).
    pub producer: String,
    /// Producer semver.
    pub version: String,
    /// What vantage point this producer observes from.
    ///
    /// REQUIRED — no `#[serde(default)]`. This field is what lets
    /// [`Certificate::check_coverage_deficit`] tell an adapter from an
    /// observer; defaulting it would reintroduce the hole it closes.
    pub role: ProducerRole,
    /// Event-kind patterns this producer emits. Populated from the
    /// producer's [`CoverageManifest`]. Required for
    /// [`Certificate::check_coverage_deficit`] to be checkable from
    /// the certificate + event stream alone.
    #[serde(default)]
    pub emits: Vec<EventKindPattern>,
    /// Human-readable known limitations that surface to end users.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_limitations: Vec<String>,
}

impl From<&CoverageManifest> for CoverageEntry {
    fn from(m: &CoverageManifest) -> Self {
        Self {
            producer: m.producer.clone(),
            version: m.version.clone(),
            role: m.role,
            emits: m.emits.clone(),
            known_limitations: m.known_limitations.clone(),
        }
    }
}

impl CoverageEntry {
    /// Does this coverage entry declare it emits the given event kind?
    #[must_use]
    pub fn covers(&self, kind: EventKind) -> bool {
        self.emits.iter().any(|p| p.matches(kind))
    }
}

/// Errors from [`Certificate::check_invariants`] and
/// [`Certificate::check_coverage_deficit`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InvariantError {
    /// Schema field didn't match the expected string for this version.
    #[error("schema field is `{0}`; expected `{CERTIFICATE_SCHEMA_V0_1}`")]
    SchemaMismatch(String),
    /// A computation invoked `bash` or `task` but no observer producer
    /// in `observation_coverage` declares fs.* coverage. Per
    /// certificate-contract §Coverage-Deficit, the certificate MUST
    /// NOT be `valid` under this condition.
    #[error(
        "coverage deficit: tool.invoked tool_kind={tool_kind} with no producer \
         in observation_coverage declaring fs.* coverage"
    )]
    CoverageDeficit {
        /// The tool kind that triggered the rule (`bash` or `task`).
        tool_kind: String,
    },
    /// An event in the IR stream names a producer that is not present
    /// in `observation_coverage`. Anti-pattern from certificate contract.
    #[error("producer `{producer}` emitted events but is not in observation_coverage")]
    ProducerMissingFromCoverage {
        /// The missing producer's identity.
        producer: String,
    },
    /// Invariant #7 keystone: `Valid` requires every dependency at
    /// `Exact` or `Versioned` trust class.
    #[error(
        "status is Valid but dependency `{dependency_key}` has trust class \
         `{trust_class:?}` (Heuristic/Volatile cap the status at LikelyValid)"
    )]
    ValidWithLowerTrust {
        /// Which dependency violated the rule.
        dependency_key: String,
        /// Its trust class.
        trust_class: TrustClass,
    },
    /// Invariant #9: `Valid` or `LikelyValid` require `recipe_hash`.
    #[error("status is {status:?} but produced_by.recipe_hash is absent (invariant #9)")]
    MissingRecipeHash {
        /// The status that requires the hash.
        status: ValidityStatus,
    },
    /// Invariant #6: non-`Valid` status must carry reasons.
    #[error("status is {status:?} but status.reasons is empty (invariant #6)")]
    MissingReasons {
        /// The status that requires reasons.
        status: ValidityStatus,
    },
    /// A `Volatile` dependency was recorded without a TTL.
    #[error("dependency `{0}` is Volatile but ttl_seconds is absent")]
    NakedVolatile(String),
    /// The certificate names no producers in `observation_coverage`
    /// (anti-pattern from certificate contract).
    #[error("observation_coverage is empty")]
    EmptyObservationCoverage,
}

/// How an [`InvariantError`] translates to certificate emission.
///
/// This is deliberately NOT `Option<ReasonCode>`. The idiomatic call
/// site for an `Option` — `if let Some(rc) = err.reason_code() {
/// downgrade(rc) }` — silently ignores every structural defect,
/// precisely the errors that must be loudest, and it sits directly on
/// the invariant-#7 path. A two-case enum forces the caller to name
/// what it does in both worlds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[must_use]
pub enum ReasonMapping {
    /// An evaluation outcome: downgrade `status.value` and attach this
    /// reason. The certificate is well-formed; the world is not clean.
    Downgrade(ReasonCode),
    /// The certificate is malformed. Emit nothing. This is a producer
    /// defect, not a fact about the world; downgrading would hide the
    /// bug behind a legitimate-looking `unknown`.
    StructuralDefect,
}

impl InvariantError {
    /// How this error should be translated when a caller is deciding
    /// whether — and with what status — to emit a certificate.
    ///
    /// [`InvariantError::CoverageDeficit`] and
    /// [`InvariantError::ProducerMissingFromCoverage`] describe a real,
    /// reportable gap in evidence, so they map to
    /// [`ReasonMapping::Downgrade`]. Every other variant describes a
    /// defect in the certificate's own construction and maps to
    /// [`ReasonMapping::StructuralDefect`].
    ///
    /// # `ProducerMissingFromCoverage` is dual-use
    ///
    /// - On the **emission** path the engine assembles
    ///   `observation_coverage` itself from the producers it ran, so a
    ///   missing producer is an engine bug. W4 MUST treat it as fatal
    ///   there and refuse to emit, regardless of this mapping.
    /// - On the **re-check** path (a certificate produced on machine A
    ///   and checked on machine B — certificate-contract §Portability)
    ///   the coverage list came from the document, so a missing
    ///   producer is real evidence that the certificate's silences
    ///   cannot be interpreted. `Downgrade` to `Unknown` is correct
    ///   there.
    pub const fn reason_mapping(&self) -> ReasonMapping {
        match self {
            Self::CoverageDeficit { .. } => ReasonMapping::Downgrade(ReasonCode::CoverageDeficit),
            Self::ProducerMissingFromCoverage { .. } => {
                ReasonMapping::Downgrade(ReasonCode::ProducerMissingFromCoverage)
            }
            Self::SchemaMismatch(_)
            | Self::ValidWithLowerTrust { .. }
            | Self::MissingRecipeHash { .. }
            | Self::MissingReasons { .. }
            | Self::NakedVolatile(_)
            | Self::EmptyObservationCoverage => ReasonMapping::StructuralDefect,
        }
    }
}

impl Certificate {
    /// Machine-check the invariants that ADR 0004 + certificate contract
    /// require. Returns the first violation encountered.
    ///
    /// This is redundant with the JSON Schema `allOf if/then` rules but
    /// exists so Rust code paths (engine, adapters constructing
    /// certificates in-memory) cannot silently violate the rules.
    ///
    /// # Errors
    ///
    /// Returns `Ok(())` iff every rule holds. Otherwise the first
    /// [`InvariantError`] encountered.
    pub fn check_invariants(&self) -> Result<(), InvariantError> {
        if self.schema != CERTIFICATE_SCHEMA_V0_1 {
            return Err(InvariantError::SchemaMismatch(self.schema.clone()));
        }
        if self.observation_coverage.is_empty() {
            return Err(InvariantError::EmptyObservationCoverage);
        }

        // Invariant #7: Valid requires every edge Exact|Versioned.
        if matches!(self.status.value, ValidityStatus::Valid) {
            for dep in &self.depends_on {
                if matches!(
                    dep.trust_class,
                    TrustClass::Heuristic | TrustClass::Volatile
                ) {
                    return Err(InvariantError::ValidWithLowerTrust {
                        dependency_key: dep.key.clone(),
                        trust_class: dep.trust_class,
                    });
                }
            }
        }

        // Invariant #9: Valid|LikelyValid require recipe_hash.
        if matches!(
            self.status.value,
            ValidityStatus::Valid | ValidityStatus::LikelyValid
        ) && self.produced_by.recipe_hash.is_none()
        {
            return Err(InvariantError::MissingRecipeHash {
                status: self.status.value,
            });
        }

        // Invariant #6: non-Valid statuses carry reasons.
        if !matches!(self.status.value, ValidityStatus::Valid) && self.status.reasons.is_empty() {
            return Err(InvariantError::MissingReasons {
                status: self.status.value,
            });
        }

        // Naked-volatile check.
        for dep in &self.depends_on {
            if matches!(dep.trust_class, TrustClass::Volatile) && dep.ttl_seconds.is_none() {
                return Err(InvariantError::NakedVolatile(dep.key.clone()));
            }
        }

        Ok(())
    }

    /// Enforce the certificate contract's §Coverage-Deficit rule
    /// against an IR event stream. This is the rule that turns
    /// invariant #7 into a machine-checked property at the boundary
    /// between adapter+observer producers and the certificate.
    ///
    /// Rules (per contract):
    ///
    /// - Every producer that emitted an event MUST appear in
    ///   `observation_coverage` (anti-pattern from certificate
    ///   contract). Enforced regardless of status.
    /// - If `status.value == Valid` and any `tool.invoked` of kind
    ///   `bash|task` occurred, at least one producer in
    ///   `observation_coverage` with `role == ProducerRole::Observer`
    ///   MUST declare fs.* coverage (via its `emits` list). An
    ///   `Adapter`-role producer does NOT discharge the obligation
    ///   however broad its `emits` list — it is blind inside
    ///   subprocesses by construction. Otherwise the certificate is a
    ///   coverage
    ///   deficit and MUST NOT be `valid`. This is why v0 on macOS
    ///   (StubObserver, zero fs coverage) reports `unknown` — not
    ///   `valid` — on any computation that invoked Bash.
    ///
    /// # Errors
    ///
    /// - [`InvariantError::CoverageDeficit`] on a Valid cert with an
    ///   uncovered bash/task invocation.
    /// - [`InvariantError::ProducerMissingFromCoverage`] on any event
    ///   whose producer is absent from `observation_coverage`.
    pub fn check_coverage_deficit(&self, events: &[IrEvent]) -> Result<(), InvariantError> {
        // Producer-membership check regardless of status.
        let declared: std::collections::HashSet<&str> = self
            .observation_coverage
            .iter()
            .map(|c| c.producer.as_str())
            .collect();
        for ev in events {
            if !declared.contains(ev.producer.as_str()) {
                return Err(InvariantError::ProducerMissingFromCoverage {
                    producer: ev.producer.clone(),
                });
            }
        }

        // Coverage-deficit rule only bites when the cert claims Valid.
        if !matches!(self.status.value, ValidityStatus::Valid) {
            return Ok(());
        }

        // ONLY an observer-role producer discharges the obligation. An
        // adapter that declares fs.read/fs.write does NOT — adapters
        // synthesize filesystem events from tool inputs they can see and
        // are blind inside subprocesses by construction. Accepting any
        // producer here let a `valid` certificate stand behind zero
        // subprocess observation, which is exactly what this rule exists
        // to prevent.
        let has_fs_covered_observer = self.observation_coverage.iter().any(|c| {
            matches!(c.role, ProducerRole::Observer)
                && (c.covers(EventKind::FsRead) || c.covers(EventKind::FsWrite))
        });

        for ev in events {
            if !matches!(ev.kind, EventKind::ToolInvoked) {
                continue;
            }
            let Some(tool_kind) = ev.payload.get("tool_kind").and_then(|v| v.as_str()) else {
                continue;
            };
            if matches!(tool_kind, "bash" | "task") && !has_fs_covered_observer {
                return Err(InvariantError::CoverageDeficit {
                    tool_kind: tool_kind.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Derive a `cert_id` from the certificate's canonical JSON bytes.
    ///
    /// The derivation zeroes out the `cert_id` field (as
    /// `"blake3:0000...0000"`) before hashing so the resulting id is
    /// idempotent (rehashing the emitted certificate produces the same
    /// value).
    ///
    /// # Errors
    ///
    /// Returns a `serde_json::Error` if canonical serialization fails
    /// (which would indicate a bug — the type is `Serialize` for every
    /// field).
    pub fn derive_cert_id(&self) -> Result<Hash, serde_json::Error> {
        let mut clone = self.clone();
        clone.cert_id = zero_hash();
        let bytes = serde_json::to_vec(&clone)?;
        let digest = blake3::hash(&bytes);
        Ok(Hash {
            algo: crate::ir::HashAlgo::Blake3,
            digest_hex: digest.to_hex().to_string(),
        })
    }
}

fn zero_hash() -> Hash {
    Hash {
        algo: crate::ir::HashAlgo::Blake3,
        digest_hex: "0".repeat(64),
    }
}
