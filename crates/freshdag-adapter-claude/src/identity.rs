//! The adapter's identity rule.
//!
//! `docs/contracts/execution-ir.md §Event Envelope` requires that
//! `computation_id` be *"a deterministic function of `(recipe_id_or_hash,
//! canonicalized_declared_inputs, adapter_identity_rule_version)`
//! computed in `freshdag-core`, not minted opaquely by the adapter"* —
//! and that *"adapters that cannot supply a `recipe_id_or_hash` MUST
//! synthesize one from a session-scoped stable identifier and record the
//! rule used."*
//!
//! Claude Code exposes no recipe. A hook payload carries a `session_id`
//! and nothing else that is stable across a computation. So this module
//! implements the fallback clause:
//!
//! - `recipe_id_or_hash` = `claude-code-session:<session_id>`
//! - `canonicalized_declared_inputs` = `""` (the adapter declares none;
//!   it observes rather than declares)
//! - `adapter_identity_rule_version` = [`SESSION_AS_COMPUTATION_V1`]
//!
//! The hash itself is computed by
//! [`freshdag_core::computation::ComputationId::derive`] so two adapters
//! observing the same runtime under the same rule cannot fork the graph.
//! The rule string is recorded in the coverage manifest's `capabilities`
//! and on every `computation.started` payload, so a consumer can always
//! tell which rule produced a given id.

use freshdag_core::computation::ComputationId;

/// Identity-rule version string: one Claude Code session is one
/// computation.
///
/// Bump this (do not mutate it) if the slicing rule changes, so that
/// ids minted under the old rule remain distinguishable from ids minted
/// under the new one.
pub const SESSION_AS_COMPUTATION_V1: &str = "claude/session-as-computation/v1";

/// Prefix applied to a Claude Code `session_id` to form the synthesized
/// `recipe_id_or_hash`.
pub const RECIPE_ID_PREFIX: &str = "claude-code-session:";

/// The synthesized `recipe_id_or_hash` for a session.
#[must_use]
pub fn synthesized_recipe_id(session_id: &str) -> String {
    format!("{RECIPE_ID_PREFIX}{session_id}")
}

/// Derive the `computation_id` for a Claude Code session under the given
/// identity rule version.
///
/// Deterministic: the same `(session_id, rule_version)` always yields the
/// same id, on any host, in any process.
#[must_use]
pub fn computation_id_for_session(session_id: &str, rule_version: &str) -> ComputationId {
    ComputationId::derive(&synthesized_recipe_id(session_id), "", rule_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_is_deterministic() {
        let a = computation_id_for_session("abc-123", SESSION_AS_COMPUTATION_V1);
        let b = computation_id_for_session("abc-123", SESSION_AS_COMPUTATION_V1);
        assert_eq!(a, b);
        assert!(a.0.starts_with("comp:"));
    }

    #[test]
    fn different_sessions_get_different_ids() {
        let a = computation_id_for_session("abc-123", SESSION_AS_COMPUTATION_V1);
        let b = computation_id_for_session("abc-124", SESSION_AS_COMPUTATION_V1);
        assert_ne!(a, b);
    }

    #[test]
    fn rule_version_participates_in_the_hash() {
        // The whole point of versioning the rule: a future slicing rule
        // must not collide with ids minted under v1.
        let a = computation_id_for_session("abc-123", SESSION_AS_COMPUTATION_V1);
        let b = computation_id_for_session("abc-123", "claude/turn-as-computation/v1");
        assert_ne!(a, b);
    }

    #[test]
    fn recipe_id_is_session_scoped() {
        assert_eq!(
            synthesized_recipe_id("s1"),
            "claude-code-session:s1".to_string()
        );
    }
}
