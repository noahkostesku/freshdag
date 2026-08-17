//! Adapter configuration.
//!
//! `docs/contracts/adapter-contract.md §Configuration` requires an
//! adapter to accept a sink location and an optional coverage override
//! that suppresses noisy event kinds. Suppression is *never* silent: a
//! [`crate::diagnostic::DiagnosticCode::CoverageOverrideSuppressed`]
//! diagnostic records what was withheld, and `diagnostic` itself can
//! never be suppressed.

use freshdag_core::ir::EventKindPattern;

/// The producer identity this adapter stamps on every event. Must match
/// the `producer` field of the coverage manifest, otherwise
/// `Certificate::check_coverage_deficit` rejects the stream with
/// `ProducerMissingFromCoverage`.
pub const PRODUCER: &str = "freshdag-adapter-claude";

/// The `agent_kind` reported on `session.started`.
pub const AGENT_KIND: &str = "claude-code";

/// Sentinel `session_id` used only when a hook payload was too broken to
/// yield one (invalid JSON, missing `session_id`).
///
/// Events carrying it also carry `computation_id: null` — the adapter
/// refuses to attribute an event to a computation it cannot identify.
pub const UNKNOWN_SESSION_ID: &str = "claude-code:unknown-session";

/// Everything the compile path needs beyond the payload, the clock and
/// the id generator.
///
/// The compile function is a pure function of
/// `(payload, clock, idgen, AdapterConfig)`.
#[derive(Debug, Clone)]
pub struct AdapterConfig {
    /// Semver stamped into `IrEvent::producer_version`.
    pub producer_version: String,
    /// Identity-rule version used to derive `computation_id`. See
    /// [`crate::identity`].
    pub identity_rule_version: String,
    /// User-supplied coverage override: event kinds to withhold.
    /// `diagnostic` is never suppressible.
    pub suppressed_kinds: Vec<EventKindPattern>,
    /// Largest file this adapter will hash inline to fingerprint an
    /// `fs.read`. Above it the read is emitted without a fingerprint
    /// rather than stalling the tool call.
    pub max_hash_bytes: u64,
}

impl AdapterConfig {
    /// Default configuration: this crate's version, the v1 identity
    /// rule, no suppression.
    #[must_use]
    pub fn new() -> Self {
        Self {
            producer_version: env!("CARGO_PKG_VERSION").to_string(),
            identity_rule_version: crate::identity::SESSION_AS_COMPUTATION_V1.to_string(),
            suppressed_kinds: Vec::new(),
            max_hash_bytes: crate::content::DEFAULT_MAX_HASH_BYTES,
        }
    }

    /// Replace the suppression list.
    #[must_use]
    pub fn with_suppressed_kinds(mut self, kinds: Vec<EventKindPattern>) -> Self {
        self.suppressed_kinds = kinds;
        self
    }
}

impl Default for AdapterConfig {
    fn default() -> Self {
        Self::new()
    }
}
