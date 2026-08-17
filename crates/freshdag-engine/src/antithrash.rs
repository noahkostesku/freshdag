//! Anti-thrash trust-class transitions.
//!
//! Implements `docs/contracts/probe-contract.md §Anti-thrash Protocol`.
//! State is per-`(dependency_key, probe_identity)`:
//!
//! - **Escalation requires stability.** A single higher-trust
//!   observation does not adopt the higher class; N=2 *consecutive*
//!   observations at the same higher class do. Any intervening
//!   observation at a different class resets the pending escalation.
//! - **Demotion is explicit, never silent.** A strictly lower observed
//!   class emits a `probe.trust_demoted` diagnostic and forces
//!   re-observation: the edge's verdict this round is `Unknown`, so the
//!   demoted class never quietly appears on a certificate.
//!
//! ## Demotion requires an observed trust class (ADR 0010)
//!
//! An observation at a strictly lower class is the *only* demotion
//! trigger. `ProbeResult::Unknown` carries no trust-class information by
//! construction and MUST NOT demote, at any value of `retryable`.
//!
//! The contract used to name `Unknown { retryable: false }` as a second
//! trigger, and this module used to implement it. It was a category
//! error: `retryable` answers a *scheduling* question ("should the
//! daemon try again?") and demotion answers an *evidentiary* one ("did
//! the source's validator get weaker?"), and the two are independent. A
//! deleted file is unretryable and carries no trust information; an
//! endpoint that stopped serving `ETag` is retryable and carries a lot.
//! The one case where demotion is the right conclusion already arrives
//! through [`TrustLedger::observe`], which folds a probe's
//! `observed_trust_class`.
//!
//! The state is in memory and lives for the lifetime of one
//! [`Engine`](crate::Engine). Persisting it across processes is a
//! recorded follow-up, not Wave 2.
//!
//! ## Deviation from the contract's sketch, deliberately
//!
//! The contract sketches `first_seen: Instant`. This module stores an
//! [`OffsetDateTime`] taken from the injected [`EvalClock`](crate::EvalClock)
//! instead. `Instant` is unmockable, and a test that cannot control
//! time is nondeterministic, which `.claude/rules/testing.md` forbids.
//! The field is otherwise used exactly as sketched.
//!
//! ## What this module deliberately does NOT do
//!
//! Adopting an escalation records it here; it does **not** raise the
//! trust class used to compute a certificate's status. The recorded
//! class on `depends_on[]` comes from the store's observation, and
//! v0 never lets an in-memory ledger promote a `Heuristic` edge to
//! `Valid`. Writing an adopted escalation back into the store is a
//! follow-up with a persistence design; until then the conservative
//! direction is the only safe one.

use std::collections::BTreeMap;

use freshdag_core::dependency::TrustClass;
use time::OffsetDateTime;

use crate::registry::ProbeIdentity;

/// Consecutive higher-trust observations required before adoption.
/// N=2 per probe-contract §Anti-thrash Protocol.
pub const ESCALATION_THRESHOLD: u8 = 2;

/// What one observation did to a dependency's trust class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustTransition {
    /// The observed class equals the adopted class. Nothing moved.
    Unchanged {
        /// The class in force.
        adopted: TrustClass,
    },
    /// A higher class was observed but not yet often enough to adopt.
    /// The adopted class is unchanged — this is the anti-flap case.
    EscalationPending {
        /// The class being proposed.
        proposed: TrustClass,
        /// How many consecutive observations have proposed it.
        observations: u8,
        /// The class still in force.
        adopted: TrustClass,
    },
    /// N consecutive higher-trust observations; the higher class is
    /// adopted for future checks.
    Escalated {
        /// Previous class.
        from: TrustClass,
        /// Newly adopted class.
        to: TrustClass,
    },
    /// A strictly lower class was **observed**. The engine emits
    /// `probe.trust_demoted` and the edge is `Unknown` this round.
    ///
    /// Only [`TrustLedger::observe`] produces this. Per ADR 0010 a probe
    /// failure never does, however unretryable: `Unknown` carries no
    /// trust-class information to demote on.
    Demoted {
        /// Previous class.
        from: TrustClass,
        /// The class now in force.
        to: TrustClass,
    },
}

#[derive(Debug, Clone)]
struct Pending {
    class: TrustClass,
    count: u8,
    #[allow(dead_code)] // recorded per contract; consumed by the persistence follow-up
    first_seen: OffsetDateTime,
}

#[derive(Debug, Clone)]
struct Entry {
    last_adopted_trust_class: TrustClass,
    pending_escalation: Option<Pending>,
}

/// Per-`(dependency_key, probe_identity)` trust-class state.
#[derive(Debug, Default)]
pub struct TrustLedger {
    entries: BTreeMap<(String, ProbeIdentity), Entry>,
    /// Which probe identity last answered for a dependency key. Used to
    /// detect probe removal (probe-contract §Anti-thrash, "Probe
    /// removal") without falling through to another probe.
    last_probe: BTreeMap<String, ProbeIdentity>,
}

impl TrustLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The probe identity that last answered for this dependency key.
    #[must_use]
    pub fn last_probe_for(&self, dependency_key: &str) -> Option<&ProbeIdentity> {
        self.last_probe.get(dependency_key)
    }

    /// Record that `identity` is answering for `dependency_key`.
    pub fn note_probe(&mut self, dependency_key: &str, identity: &ProbeIdentity) {
        self.last_probe
            .insert(dependency_key.to_string(), identity.clone());
    }

    /// The trust class currently in force for this pair, if seen before.
    #[must_use]
    pub fn adopted(&self, dependency_key: &str, identity: &ProbeIdentity) -> Option<TrustClass> {
        self.entries
            .get(&(dependency_key.to_string(), identity.clone()))
            .map(|e| e.last_adopted_trust_class)
    }

    /// Fold one probe observation into the ledger.
    ///
    /// `recorded` seeds the adopted class the first time this pair is
    /// seen — the certificate's recorded class is the starting point,
    /// not whatever the probe happened to say first.
    pub fn observe(
        &mut self,
        dependency_key: &str,
        identity: &ProbeIdentity,
        recorded: TrustClass,
        observed: TrustClass,
        at: OffsetDateTime,
    ) -> TrustTransition {
        self.note_probe(dependency_key, identity);
        let entry = self
            .entries
            .entry((dependency_key.to_string(), identity.clone()))
            .or_insert(Entry {
                last_adopted_trust_class: recorded,
                pending_escalation: None,
            });
        let adopted = entry.last_adopted_trust_class;

        if observed == adopted {
            // Resetting on a non-consecutive observation is the whole
            // point of the protocol: a flapping source never
            // accumulates two in a row.
            entry.pending_escalation = None;
            return TrustTransition::Unchanged { adopted };
        }

        if observed.is_demotion_from(adopted) {
            entry.pending_escalation = None;
            entry.last_adopted_trust_class = observed;
            return TrustTransition::Demoted {
                from: adopted,
                to: observed,
            };
        }

        // Escalation.
        let count = match &entry.pending_escalation {
            Some(p) if p.class == observed => p.count.saturating_add(1),
            _ => 1,
        };
        if count >= ESCALATION_THRESHOLD {
            entry.pending_escalation = None;
            entry.last_adopted_trust_class = observed;
            TrustTransition::Escalated {
                from: adopted,
                to: observed,
            }
        } else {
            let first_seen = match &entry.pending_escalation {
                Some(p) if p.class == observed => p.first_seen,
                _ => at,
            };
            entry.pending_escalation = Some(Pending {
                class: observed,
                count,
                first_seen,
            });
            TrustTransition::EscalationPending {
                proposed: observed,
                observations: count,
                adopted,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> ProbeIdentity {
        ProbeIdentity::derive("https", None, 0)
    }

    fn at(secs: i64) -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(secs)
    }

    #[test]
    fn a_single_higher_observation_does_not_escalate() {
        let mut ledger = TrustLedger::new();
        let t = ledger.observe(
            "https://acme.com/x",
            &id(),
            TrustClass::Heuristic,
            TrustClass::Versioned,
            at(0),
        );
        assert_eq!(
            t,
            TrustTransition::EscalationPending {
                proposed: TrustClass::Versioned,
                observations: 1,
                adopted: TrustClass::Heuristic,
            }
        );
        assert_eq!(
            ledger.adopted("https://acme.com/x", &id()),
            Some(TrustClass::Heuristic)
        );
    }

    #[test]
    fn two_consecutive_higher_observations_escalate() {
        let mut ledger = TrustLedger::new();
        let key = "https://acme.com/x";
        ledger.observe(
            key,
            &id(),
            TrustClass::Heuristic,
            TrustClass::Versioned,
            at(0),
        );
        let t = ledger.observe(
            key,
            &id(),
            TrustClass::Heuristic,
            TrustClass::Versioned,
            at(1),
        );
        assert_eq!(
            t,
            TrustTransition::Escalated {
                from: TrustClass::Heuristic,
                to: TrustClass::Versioned
            }
        );
        assert_eq!(ledger.adopted(key, &id()), Some(TrustClass::Versioned));
    }

    /// The adversarial input the probe contract exists for: a source
    /// that alternates between two trust classes forever.
    ///
    /// Sequence: `versioned, heuristic, versioned, heuristic, versioned,
    /// heuristic` against a `heuristic`-recorded dependency. Every
    /// `versioned` is an escalation proposal; every `heuristic` matches
    /// the adopted class and resets it. The adopted class MUST stay
    /// `Heuristic` throughout — no oscillation, and in particular no
    /// silent promotion of a heuristic edge to a versioned one, which
    /// would let the artifact report `valid` instead of `likely-valid`
    /// (invariant #8).
    #[test]
    fn flapping_trust_classes_never_oscillate_the_adopted_class() {
        let mut ledger = TrustLedger::new();
        let key = "https://flappy.example/x";
        let flap = [
            TrustClass::Versioned,
            TrustClass::Heuristic,
            TrustClass::Versioned,
            TrustClass::Heuristic,
            TrustClass::Versioned,
            TrustClass::Heuristic,
        ];
        let mut adopted_history = Vec::new();
        for (i, observed) in flap.iter().enumerate() {
            let t = ledger.observe(
                key,
                &id(),
                TrustClass::Heuristic,
                *observed,
                at(i64::try_from(i).expect("small index")),
            );
            match t {
                TrustTransition::Unchanged { adopted }
                | TrustTransition::EscalationPending { adopted, .. } => {
                    adopted_history.push(adopted);
                }
                other => panic!("flap produced a class change at step {i}: {other:?}"),
            }
            adopted_history.push(ledger.adopted(key, &id()).expect("entry"));
        }
        assert!(
            adopted_history.iter().all(|c| *c == TrustClass::Heuristic),
            "adopted class oscillated: {adopted_history:?}"
        );
    }

    #[test]
    fn a_reset_forces_the_escalation_count_to_restart() {
        let mut ledger = TrustLedger::new();
        let key = "https://acme.com/x";
        // exact, then versioned, then exact: two exact observations but
        // not consecutive, so nothing is adopted.
        ledger.observe(key, &id(), TrustClass::Heuristic, TrustClass::Exact, at(0));
        ledger.observe(
            key,
            &id(),
            TrustClass::Heuristic,
            TrustClass::Versioned,
            at(1),
        );
        let t = ledger.observe(key, &id(), TrustClass::Heuristic, TrustClass::Exact, at(2));
        assert_eq!(
            t,
            TrustTransition::EscalationPending {
                proposed: TrustClass::Exact,
                observations: 1,
                adopted: TrustClass::Heuristic,
            },
            "an intervening different class resets the pending escalation"
        );
    }

    #[test]
    fn demotion_is_reported_explicitly() {
        let mut ledger = TrustLedger::new();
        let key = "https://acme.com/x";
        let t = ledger.observe(
            key,
            &id(),
            TrustClass::Versioned,
            TrustClass::Heuristic,
            at(0),
        );
        assert_eq!(
            t,
            TrustTransition::Demoted {
                from: TrustClass::Versioned,
                to: TrustClass::Heuristic
            }
        );
    }

    /// ADR 0010: the ledger has exactly one demotion trigger, and it is
    /// an observation. `Unknown` never reaches this module, so a probe
    /// failure — retryable or not — leaves the adopted class alone.
    ///
    /// The deleted `note_unretryable_failure` also announced a demotion
    /// it never performed: it returned `Demoted { to: Volatile }`
    /// without writing that class into the entry, so the "forces
    /// re-observation" half of the contract clause never happened, and
    /// it reported a transition even when `from` was already `Volatile`.
    #[test]
    fn a_probe_failure_leaves_the_adopted_class_alone() {
        let mut ledger = TrustLedger::new();
        let key = "file:///x";
        ledger.observe(key, &id(), TrustClass::Exact, TrustClass::Exact, at(0));
        assert_eq!(ledger.adopted(key, &id()), Some(TrustClass::Exact));

        // The engine's Unknown arm records only that this probe
        // answered. Nothing about the trust class moves.
        ledger.note_probe(key, &id());
        assert_eq!(
            ledger.adopted(key, &id()),
            Some(TrustClass::Exact),
            "a probe failure carries no trust-class information to demote on"
        );
    }

    #[test]
    fn probe_identity_is_tracked_per_dependency() {
        let mut ledger = TrustLedger::new();
        assert!(ledger.last_probe_for("file:///x").is_none());
        ledger.note_probe("file:///x", &id());
        assert_eq!(ledger.last_probe_for("file:///x"), Some(&id()));
    }
}
