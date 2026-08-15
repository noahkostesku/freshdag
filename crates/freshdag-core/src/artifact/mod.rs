//! Artifact identity — the content-addressed handle for what a
//! computation produced.
//!
//! Contract: `docs/contracts/certificate-contract.md §Shape` — the
//! `artifact.*` fields on the certificate.

use serde::{Deserialize, Serialize};

use crate::ir::Hash;

/// Content-addressed identifier for an artifact. Wire form is a
/// `Hash` string (e.g., `blake3:...`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtifactId(pub String);

impl ArtifactId {
    /// Derive an `ArtifactId` from a content hash. Two artifacts with
    /// identical bytes share an ID.
    #[must_use]
    pub fn from_hash(h: &Hash) -> Self {
        Self(h.to_string())
    }
}

impl core::fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An artifact produced by a FreshDAG computation.
///
/// Serialized shape matches certificate-contract's `artifact` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Content-addressed ID (usually the same as `content_hash` in
    /// string form).
    pub id: ArtifactId,
    /// Filesystem path if the artifact lives on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Media type or scheme-specific kind (e.g., `text/markdown`).
    pub kind: String,
    /// Content hash of the artifact bytes (usually BLAKE3).
    pub content_hash: Hash,
    /// Artifact size in bytes.
    pub size: u64,
}
