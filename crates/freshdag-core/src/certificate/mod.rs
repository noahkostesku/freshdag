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
use crate::dependency::{Dependency, TrustClass, ValidityReason, ValidityStatus};
use crate::ir::{CoverageManifest, Hash};

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
    /// Stable computation identifier.
    pub computation_id: String,
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
/// keep only the fields consumers need for silence-interpretation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageEntry {
    /// Producer identity (matches `IrEvent::producer`).
    pub producer: String,
    /// Producer semver.
    pub version: String,
    /// Human-readable known limitations that surface to end users.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_limitations: Vec<String>,
}

impl From<&CoverageManifest> for CoverageEntry {
    fn from(m: &CoverageManifest) -> Self {
        Self {
            producer: m.producer.clone(),
            version: m.version.clone(),
            known_limitations: m.known_limitations.clone(),
        }
    }
}

/// Errors from [`Certificate::check_invariants`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InvariantError {
    /// Schema field didn't match the expected string for this version.
    #[error("schema field is `{0}`; expected `{CERTIFICATE_SCHEMA_V0_1}`")]
    SchemaMismatch(String),
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
