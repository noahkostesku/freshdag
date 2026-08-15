//! Computation identity — the "one agent-produced artifact" scope.
//!
//! `ComputationId` is a deterministic function of
//! `(recipe_id_or_hash, canonicalized_declared_inputs,
//! adapter_identity_rule_version)` per `docs/contracts/execution-ir.md
//! §Event Envelope`. Two adapters observing the same runtime with the
//! same identity rule must produce the same `ComputationId`; that is
//! why the derivation lives in `freshdag-core` rather than in the
//! adapter.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ir::{Hash, HashAlgo};

/// Opaque computation identifier. Wire form is
/// `"comp:<blake3-hex>"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ComputationId(pub String);

impl ComputationId {
    /// Derive a `ComputationId` from the three-tuple defined in
    /// execution-ir.md §Event Envelope.
    ///
    /// The derivation is BLAKE3 over a canonical encoding: the three
    /// strings joined by `\0`. Adapters MUST canonicalize
    /// `declared_inputs` before calling (sorted, whitespace-trimmed).
    /// The `adapter_identity_rule_version` is the adapter's own
    /// versioning of its identity rule — bump it when the rule
    /// changes so two versions of the same adapter produce distinct
    /// IDs (see IMPORTANT-1 in the S0 verifier report for why this
    /// matters).
    #[must_use]
    pub fn derive(
        recipe_id_or_hash: &str,
        canonicalized_declared_inputs: &str,
        adapter_identity_rule_version: &str,
    ) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(recipe_id_or_hash.as_bytes());
        hasher.update(b"\0");
        hasher.update(canonicalized_declared_inputs.as_bytes());
        hasher.update(b"\0");
        hasher.update(adapter_identity_rule_version.as_bytes());
        let digest = hasher.finalize();
        let hex = digest.to_hex().to_string();
        Self(format!("comp:{hex}"))
    }

    /// Extract the content hash portion (without the `comp:` prefix).
    ///
    /// # Errors
    ///
    /// Returns `None` if `self.0` does not have the expected
    /// `"comp:<64-hex>"` shape.
    #[must_use]
    pub fn as_hash(&self) -> Option<Hash> {
        let digest = self.0.strip_prefix("comp:")?;
        Hash::new(HashAlgo::Blake3, digest).ok()
    }
}

impl core::fmt::Display for ComputationId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A description of a computation.
///
/// Not serialized as-is on the certificate — the certificate's
/// `produced_by` object is a subset (see `certificate::ProducedBy`).
/// `Computation` is the in-memory representation the engine reasons
/// about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Computation {
    /// Stable ID.
    pub id: ComputationId,
    /// Recipe identifier (e.g., a script path or agent name).
    pub recipe: Option<String>,
    /// Content hash of the recipe body (whatever the adapter chose to
    /// hash). Required on any computation that produces a `Valid`
    /// certificate.
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
