//! Engine failures.
//!
//! Every variant here means **no certificate is emitted**. That is the
//! point: `docs/contracts/certificate-contract.md` distinguishes an
//! evaluation outcome (the world is not clean → downgrade the status
//! and say why) from a structural defect (the certificate is malformed
//! → emit nothing). Downgrading a structural defect would let the
//! engine hide its own invariant-#7 violation behind a legitimate
//! looking `unknown`.

use freshdag_core::artifact::ArtifactId;
use freshdag_core::certificate::InvariantError;
use freshdag_core::computation::ComputationId;
use freshdag_core::dependency::{ValidityAggregationError, ValidityStatus};
use thiserror::Error;

/// Why `Engine::check` refused to produce a certificate.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    /// No computation in the derived graph claims this artifact.
    #[error("no computation produced artifact `{artifact}`")]
    UnknownArtifact {
        /// The artifact asked about.
        artifact: ArtifactId,
    },
    /// More than one computation claims the artifact, so `produced_by`
    /// is not well defined (invariant #9).
    #[error("artifact `{artifact}` is claimed by {} computations", computations.len())]
    AmbiguousArtifact {
        /// The artifact asked about.
        artifact: ArtifactId,
        /// The claimants, in `ComputationId` order.
        computations: Vec<ComputationId>,
    },
    /// The derived graph is not reproducible from the log (a producer
    /// emitted the same `event_id` twice with different content). The
    /// store's own documentation says such derived state "must not be
    /// presented as if it were sound", so the engine refuses.
    #[error("derived graph replay is not deterministic; refusing to certify")]
    NondeterministicReplay,
    /// The `artifact.produced` event lacks a field the certificate
    /// requires.
    #[error("artifact.produced event for `{artifact}` is missing `{field}`")]
    MalformedArtifactEvent {
        /// The artifact asked about.
        artifact: ArtifactId,
        /// The absent payload field.
        field: &'static str,
    },
    /// `produced_by.recipe_hash` is required whenever the status is
    /// `valid` or `likely-valid` (certificate contract §Field Rules),
    /// and no `computation.started` event supplied one.
    #[error(
        "status would be {status:?} but no recipe_hash is available; \
         certificates may not claim {status:?} without one"
    )]
    MissingRecipeHash {
        /// The status that would have required the hash.
        status: ValidityStatus,
    },
    /// A `check_invariants` violation that
    /// [`ReasonMapping::StructuralDefect`] classifies as malformed.
    ///
    /// [`ReasonMapping::StructuralDefect`]: freshdag_core::certificate::ReasonMapping::StructuralDefect
    #[error("certificate is structurally defective: {source}")]
    MalformedCertificate {
        /// The violation.
        #[source]
        source: InvariantError,
    },
    /// The engine assembled `observation_coverage` itself and left out a
    /// producer that emitted into the computation. On the emission path
    /// that is an engine bug, not evidence about the world, so it is
    /// fatal rather than a downgrade (certificate contract,
    /// `ProducerMissingFromCoverage` §"dual-use").
    #[error(
        "engine assembled observation_coverage without producer `{producer}`, \
         which emitted into this computation; this is an engine bug"
    )]
    CoverageAssemblyBug {
        /// The producer the engine failed to include.
        producer: String,
    },
    /// `Validity::aggregate` rejected the engine's inputs.
    #[error("validity aggregation failed: {0}")]
    Aggregation(#[from] ValidityAggregationError),
    /// A self-audit inside the engine failed. Always an engine bug.
    #[error("engine self-audit failed: {message}")]
    InternalInconsistency {
        /// What disagreed.
        message: String,
    },
}
