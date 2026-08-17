//! Process exit codes.
//!
//! `ARCHITECTURE.md §11` makes the exit codes of `freshdag check` a
//! contract: `0 = fresh`, `1 = stale`, `2 = unknown`, `>2 = tool
//! error`. They are consumed from CI and cron, where the question
//! being asked is always the same one: **may I reuse this artifact?**
//!
//! # Why `likely-valid` exits 2 and not 0
//!
//! §11 enumerates three verdicts (`fresh|stale|unknown`) but
//! [`ValidityStatus`] has four. `LikelyValid` has to land somewhere,
//! and the only candidates are `0` and `2` — `1` would assert drift we
//! did not observe, and `3+` is reserved for tool errors, so a fourth
//! verdict code is not available.
//!
//! It exits `2`:
//!
//! - Invariant #8 requires heuristic validity be *distinguishable*
//!   from exact validity. Exit `0` is the same byte the kernel gets
//!   for `valid`; emitting it for `likely-valid` re-collapses at the
//!   exit-code layer precisely the distinction the certificate layer
//!   is built to preserve.
//! - Invariant #15: correctness beats cache hit rate. Exit `2` costs a
//!   recomputation. Exit `0` on a heuristic match costs a wrong
//!   answer shipped to a user.
//! - No information is lost. `likely-valid` is printed verbatim in
//!   both the prose and the `--json` certificate; only the 3-valued
//!   exit channel is coarsened, and it is coarsened downward.
//!
//! This is deliberately *not* the same claim as "likely-valid is
//! unknown". It is the claim that a coarse yes/no channel must answer
//! "no" when the honest answer is "probably". `--accept-likely-valid`
//! lets an operator opt into the other reading explicitly, which is
//! the opposite of a silent equation.
//!
//! [`ValidityStatus`]: freshdag_core::dependency::ValidityStatus

use freshdag_core::dependency::{ReasonCode, ValidityReason, ValidityStatus};

/// A `freshdag` process exit code.
///
/// The three validity codes are contractual. The tool-error codes are
/// all `> 2` and are distinguished from each other only as a
/// convenience for scripts; consumers should treat any `> 2` as "the
/// tool failed, this is not a verdict about the artifact."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Every dependency verified unchanged at `exact`/`versioned`
    /// trust. Safe to reuse.
    Valid,
    /// At least one dependency drifted. Recompute.
    Stale,
    /// Not provably valid: missing evidence, an unverifiable
    /// dependency, or a coverage gap. Also `likely-valid` unless the
    /// operator opted in. Unknown is not fresh.
    Unknown,
    /// The engine refused to emit a certificate. A structural defect
    /// is a *tool* failure, never a validity verdict — reporting a
    /// malformed certificate as `1` or `2` would let an engine bug
    /// masquerade as evidence about the world.
    EngineRefused,
    /// The store could not be opened, read, or written.
    StoreError,
    /// The command line was not understood, or names something that
    /// does not exist in the store.
    Usage,
    /// The subcommand is not implemented yet. Deliberately not `0`: a
    /// stub that exits `0` tells a CI job "all good" about work that
    /// never happened.
    NotImplemented,
}

impl Exit {
    /// The byte handed to the kernel.
    pub const fn code(self) -> i32 {
        match self {
            Self::Valid => 0,
            Self::Stale => 1,
            Self::Unknown => 2,
            Self::EngineRefused => 3,
            Self::StoreError => 4,
            Self::Usage => 5,
            Self::NotImplemented => 6,
        }
    }

    /// Map a certificate status to an exit code.
    ///
    /// `accept_likely_valid` reflects `--accept-likely-valid`: an
    /// explicit operator decision to treat "probably unchanged, but
    /// only heuristically verified" as reusable. It is the only way
    /// `LikelyValid` reaches `0`, and it never affects `Stale` or
    /// `Unknown` — no flag turns absent or contrary evidence into
    /// reuse.
    pub const fn for_status(status: ValidityStatus, accept_likely_valid: bool) -> Self {
        match status {
            ValidityStatus::Valid => Self::Valid,
            ValidityStatus::LikelyValid if accept_likely_valid => Self::Valid,
            ValidityStatus::LikelyValid | ValidityStatus::Unknown => Self::Unknown,
            ValidityStatus::Stale => Self::Stale,
        }
    }

    /// [`Self::for_status`], with the floor `--accept-likely-valid`
    /// cannot go under.
    ///
    /// The flag means "I accept a result that was **checked** but only
    /// heuristically". It does not mean "I accept a result no probe
    /// backs", and two codes are that second thing:
    /// [`ReasonCode::VolatileWithinTtlUnprobed`] (nothing was consulted)
    /// and [`ReasonCode::ProbeUnknown`] (something was consulted and
    /// returned nothing). Either way the only evidence is that whoever
    /// recorded the dependency declared a lifetime which has not yet
    /// elapsed.
    ///
    /// Letting the flag lift that to `0` would make a producer's own
    /// declaration a substitute for observation — invariant #7 with
    /// extra steps, and reachable by any producer willing to write a
    /// long `ttl_seconds`. So the floor holds even under the flag, and
    /// the renderer says why.
    ///
    /// This does not weaken the flag anywhere else: a `likely-valid`
    /// from a heuristic match, or from a volatile edge a probe actually
    /// checked and agreed with, still reaches `0`. The distinction is
    /// exactly the one ADR 0009's two reason codes exist to draw, which
    /// is why the floor could not be written before they existed.
    #[must_use]
    pub fn for_status_with_reasons(
        status: ValidityStatus,
        reasons: &[ValidityReason],
        accept_likely_valid: bool,
    ) -> Self {
        // Both codes mean "no probe evidence backs this edge". They
        // differ in whether a probe was consulted, which is a fact worth
        // telling a user and irrelevant to whether the flag applies: a
        // probe that ran and could not decide contributed nothing, so
        // the only thing holding the edge up is still the declared TTL.
        //
        // `probe-unknown` normally accompanies an `Unknown` artifact,
        // where the flag never applied anyway. It reaches `LikelyValid`
        // only on a volatile edge inside a validated TTL — precisely the
        // case this floor is about.
        let unverified = reasons.iter().any(|r| {
            matches!(
                r.reason,
                ReasonCode::VolatileWithinTtlUnprobed | ReasonCode::ProbeUnknown
            )
        });
        if accept_likely_valid && unverified && matches!(status, ValidityStatus::LikelyValid) {
            return Self::Unknown;
        }
        Self::for_status(status, accept_likely_valid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validity_codes_match_the_architecture_contract() {
        assert_eq!(Exit::Valid.code(), 0);
        assert_eq!(Exit::Stale.code(), 1);
        assert_eq!(Exit::Unknown.code(), 2);
    }

    #[test]
    fn every_tool_error_is_above_two() {
        for exit in [
            Exit::EngineRefused,
            Exit::StoreError,
            Exit::Usage,
            Exit::NotImplemented,
        ] {
            assert!(
                exit.code() > 2,
                "{exit:?} exits {}; ARCHITECTURE.md §11 reserves 0..=2 for verdicts",
                exit.code()
            );
        }
    }

    #[test]
    fn likely_valid_is_not_reusable_by_default() {
        assert_eq!(
            Exit::for_status(ValidityStatus::LikelyValid, false),
            Exit::Unknown,
            "invariant #8: likely-valid must stay distinguishable from valid"
        );
    }

    fn reason(code: ReasonCode) -> ValidityReason {
        ValidityReason {
            dependency_key: "web.search(\"x\")".to_string(),
            reason: code,
            detail: None,
        }
    }

    /// **The hole W9.1 closed.**
    ///
    /// `--accept-likely-valid` used to lift ANY `likely-valid` to exit
    /// 0, including one resting on a volatile edge no probe ever
    /// checked. The flag means "I accept a checked-but-heuristic
    /// result"; that case was never checked at all, so the flag was
    /// letting a producer's own `ttl_seconds` stand in for observation.
    #[test]
    fn accept_likely_valid_does_not_lift_a_never_probed_volatile_edge() {
        let unprobed = [reason(ReasonCode::VolatileWithinTtlUnprobed)];
        assert_eq!(
            Exit::for_status_with_reasons(ValidityStatus::LikelyValid, &unprobed, true),
            Exit::Unknown,
            "a declared lifetime is a promise, not an observation"
        );
        // And without the flag it was already 2; the floor changes nothing there.
        assert_eq!(
            Exit::for_status_with_reasons(ValidityStatus::LikelyValid, &unprobed, false),
            Exit::Unknown
        );
    }

    /// A probe that ran and could not decide is no more "checked" than
    /// no probe at all.
    ///
    /// `probe-unknown` reaches `LikelyValid` only on a volatile edge
    /// inside a validated TTL — everywhere else an undecided probe makes
    /// the edge `Unknown`, where the flag never applied. So the floor
    /// must cover it: an operator accepting `likely-valid` is accepting
    /// an edge nothing verified, whichever of the two ways it went
    /// unverified.
    #[test]
    fn accept_likely_valid_does_not_lift_an_edge_whose_probe_could_not_decide() {
        let undecided = [reason(ReasonCode::ProbeUnknown)];
        assert_eq!(
            Exit::for_status_with_reasons(ValidityStatus::LikelyValid, &undecided, true),
            Exit::Unknown,
            "a probe that supplied no evidence leaves only the declared TTL"
        );
    }

    /// The floor is narrow on purpose: it must not quietly disable the
    /// flag for every `likely-valid`.
    #[test]
    fn accept_likely_valid_still_works_for_checked_results() {
        for code in [
            ReasonCode::TrustClassHeuristicCapsAtLikelyValid,
            ReasonCode::TrustClassVolatileCapsAtLikelyValid,
        ] {
            assert_eq!(
                Exit::for_status_with_reasons(ValidityStatus::LikelyValid, &[reason(code)], true),
                Exit::Valid,
                "{code}: a probe ran and agreed, so the flag still applies"
            );
        }
    }

    /// One unprobed edge poisons the artifact even beside checked ones.
    ///
    /// The status is a single aggregate over every edge; if any edge
    /// went unchecked, the operator accepting `likely-valid` is
    /// accepting that gap whether or not other edges were verified.
    #[test]
    fn one_unprobed_edge_is_enough_to_hold_the_floor() {
        let mixed = [
            reason(ReasonCode::TrustClassHeuristicCapsAtLikelyValid),
            reason(ReasonCode::VolatileWithinTtlUnprobed),
        ];
        assert_eq!(
            Exit::for_status_with_reasons(ValidityStatus::LikelyValid, &mixed, true),
            Exit::Unknown
        );
    }

    /// The floor never *upgrades* anything, and never fires outside
    /// `likely-valid`.
    #[test]
    fn the_floor_only_ever_lowers_and_only_at_likely_valid() {
        let unprobed = [reason(ReasonCode::VolatileWithinTtlUnprobed)];
        for (status, expected) in [
            (ValidityStatus::Valid, Exit::Valid),
            (ValidityStatus::Stale, Exit::Stale),
            (ValidityStatus::Unknown, Exit::Unknown),
        ] {
            for flag in [true, false] {
                assert_eq!(
                    Exit::for_status_with_reasons(status, &unprobed, flag),
                    expected,
                    "{status:?} with flag={flag} must be unaffected by the floor"
                );
            }
        }
    }

    /// With no reasons at all, the floor is a pure pass-through.
    #[test]
    fn the_floor_agrees_with_for_status_when_no_reason_triggers_it() {
        for status in [
            ValidityStatus::Valid,
            ValidityStatus::LikelyValid,
            ValidityStatus::Stale,
            ValidityStatus::Unknown,
        ] {
            for flag in [true, false] {
                assert_eq!(
                    Exit::for_status_with_reasons(status, &[], flag),
                    Exit::for_status(status, flag),
                    "{status:?} flag={flag}"
                );
            }
        }
    }

    #[test]
    fn accepting_likely_valid_never_promotes_anything_else() {
        assert_eq!(
            Exit::for_status(ValidityStatus::LikelyValid, true),
            Exit::Valid
        );
        assert_eq!(
            Exit::for_status(ValidityStatus::Unknown, true),
            Exit::Unknown,
            "invariant #7: no flag turns unknown into fresh"
        );
        assert_eq!(
            Exit::for_status(ValidityStatus::Stale, true),
            Exit::Stale,
            "no flag turns observed drift into fresh"
        );
    }

    #[test]
    fn valid_is_the_only_status_reaching_zero_unaided() {
        for status in [
            ValidityStatus::Valid,
            ValidityStatus::LikelyValid,
            ValidityStatus::Stale,
            ValidityStatus::Unknown,
        ] {
            let zero = Exit::for_status(status, false).code() == 0;
            assert_eq!(
                zero,
                matches!(status, ValidityStatus::Valid),
                "{status:?} reached exit 0 without an explicit operator opt-in"
            );
        }
    }
}
