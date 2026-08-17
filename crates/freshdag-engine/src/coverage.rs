//! Coverage-deficit computation, engine side.
//!
//! `docs/contracts/certificate-contract.md §Coverage-Deficit Rule`
//! defines two obligations. `freshdag-core`'s
//! [`Certificate::check_coverage_deficit`] implements the second one
//! (the `bash|task` clause) and only when the certificate claims
//! `valid`. This module implements the first, general one:
//!
//! ```text
//! observed_effect_kinds = { fs.*, proc.*, net.* events emitted while
//!                           this computation was active }
//! covered_effect_kinds  = union over observation_coverage of the kinds
//!                         each producer declares it emits
//! deficit               = observed_effect_kinds - covered_effect_kinds
//! ```
//!
//! and it evaluates the `bash|task` clause at **every** status, not
//! only at `valid`. A computation that is already `unknown` because it
//! observed no dependencies still owes the user the reason its
//! dependency set is empty: the subprocess nobody watched.
//!
//! [`Certificate::check_coverage_deficit`]: freshdag_core::certificate::Certificate::check_coverage_deficit

use std::collections::BTreeSet;

use freshdag_core::certificate::CoverageEntry;
use freshdag_core::ir::{EventKind, IrEvent, ProducerRole};

/// The effect kinds the coverage-deficit rule ranges over: `fs.*`,
/// `proc.*`, `net.*`.
///
/// `EventKind` is not iterable, so the set is spelled out. It is
/// exercised by [`tests::effect_kinds_are_exactly_the_fs_proc_net_family`],
/// which fails if a new `fs.*`/`proc.*`/`net.*` kind lands in core
/// without being added here.
pub const EFFECT_KINDS: &[EventKind] = &[
    EventKind::FsRead,
    EventKind::FsWrite,
    EventKind::FsStat,
    EventKind::FsRename,
    EventKind::FsUnlink,
    EventKind::FsDirlist,
    EventKind::ProcSpawn,
    EventKind::ProcExit,
    EventKind::NetConnect,
    EventKind::NetFetch,
];

/// Is this an effect kind the deficit rule ranges over?
#[must_use]
pub fn is_effect_kind(kind: EventKind) -> bool {
    EFFECT_KINDS.contains(&kind)
}

/// `observed_effect_kinds - covered_effect_kinds`, in wire-string order.
///
/// A non-empty result means the computation exhibited a category of
/// effect no producer claims to cover, so its silences in that category
/// are uninterpretable. Invariant #7: that is `unknown`, never `valid`.
#[must_use]
pub fn effect_deficit(events: &[IrEvent], coverage: &[CoverageEntry]) -> BTreeSet<EventKind> {
    let mut deficit = BTreeSet::new();
    for event in events {
        if !is_effect_kind(event.kind) {
            continue;
        }
        if !coverage.iter().any(|c| c.covers(event.kind)) {
            deficit.insert(event.kind);
        }
    }
    deficit
}

/// Deterministic, secret-free `detail` for a
/// [`ReasonCode::CoverageDeficit`](freshdag_core::dependency::ReasonCode::CoverageDeficit)
/// raised by the effect-kind rule.
#[must_use]
pub fn effect_deficit_detail(deficit: &BTreeSet<EventKind>) -> String {
    let kinds: Vec<&str> = deficit.iter().map(|k| k.as_wire_str()).collect();
    format!("uncovered-effect-kinds={}", kinds.join(","))
}

/// Does any producer with `role: observer` declare `fs.*` coverage?
///
/// Only an observer discharges a `bash`/`task` obligation. An adapter
/// declaring `fs.read`/`fs.write` does not, however broad its `emits`
/// list — it synthesizes those from tool inputs it can see and is blind
/// inside subprocesses by construction (ADR 0006).
#[must_use]
pub fn has_fs_covered_observer(coverage: &[CoverageEntry]) -> bool {
    coverage.iter().any(|c| {
        matches!(c.role, ProducerRole::Observer)
            && (c.covers(EventKind::FsRead) || c.covers(EventKind::FsWrite))
    })
}

/// Deterministic `detail` for an undischarged `bash`/`task` obligation.
#[must_use]
pub fn obligation_detail(tool_kinds: &BTreeSet<String>) -> String {
    let kinds: Vec<&str> = tool_kinds.iter().map(String::as_str).collect();
    format!(
        "undischarged-observation-obligation tool_kind={}",
        kinds.join(",")
    )
}

#[cfg(test)]
mod tests {
    use freshdag_core::ir::EventKindPattern;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    fn entry(producer: &str, role: ProducerRole, emits: &[&str]) -> CoverageEntry {
        CoverageEntry {
            producer: producer.to_string(),
            version: "0.1.0".to_string(),
            role,
            emits: emits.iter().map(|s| EventKindPattern::new(*s)).collect(),
            partial: std::collections::BTreeMap::new(),
            known_limitations: Vec::new(),
        }
    }

    fn event(producer: &str, kind: EventKind) -> IrEvent {
        IrEvent {
            event_id: Uuid::nil(),
            producer: producer.to_string(),
            producer_version: "0.1.0".to_string(),
            session_id: "s".to_string(),
            computation_id: Some("comp:1".to_string()),
            parent_id: None,
            causal_inputs: None,
            ts: OffsetDateTime::UNIX_EPOCH,
            kind,
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn effect_kinds_are_exactly_the_fs_proc_net_family() {
        // Guards against a new fs./proc./net. kind landing in core
        // without joining EFFECT_KINDS: this is the set the general
        // deficit rule ranges over, and a missing member is a silent
        // hole in invariant #7 enforcement.
        let all = [
            EventKind::SessionStarted,
            EventKind::SessionEnded,
            EventKind::ComputationStarted,
            EventKind::ComputationEnded,
            EventKind::ToolInvoked,
            EventKind::ToolCompleted,
            EventKind::FsRead,
            EventKind::FsWrite,
            EventKind::FsStat,
            EventKind::FsRename,
            EventKind::FsUnlink,
            EventKind::FsDirlist,
            EventKind::ProcSpawn,
            EventKind::ProcExit,
            EventKind::NetConnect,
            EventKind::NetFetch,
            EventKind::ProbeChecked,
            EventKind::ArtifactProduced,
            EventKind::Diagnostic,
        ];
        for kind in all {
            let wire = kind.as_wire_str();
            let is_family =
                wire.starts_with("fs.") || wire.starts_with("proc.") || wire.starts_with("net.");
            assert_eq!(
                is_family,
                is_effect_kind(kind),
                "EFFECT_KINDS disagrees with the fs./proc./net. family for `{wire}`"
            );
        }
    }

    #[test]
    fn uncovered_effect_kinds_become_a_deficit() {
        let coverage = vec![entry("adapter", ProducerRole::Adapter, &["fs.read"])];
        let events = vec![
            event("adapter", EventKind::FsRead),
            event("adapter", EventKind::NetFetch),
            event("adapter", EventKind::ProcSpawn),
        ];
        let deficit = effect_deficit(&events, &coverage);
        assert_eq!(
            deficit,
            [EventKind::ProcSpawn, EventKind::NetFetch]
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(
            effect_deficit_detail(&deficit),
            "uncovered-effect-kinds=proc.spawn,net.fetch"
        );
    }

    #[test]
    fn a_wildcard_declaration_covers_the_family() {
        let coverage = vec![entry(
            "observer",
            ProducerRole::Observer,
            &["fs.*", "proc.*", "net.*"],
        )];
        let events = vec![
            event("observer", EventKind::FsDirlist),
            event("observer", EventKind::ProcExit),
            event("observer", EventKind::NetConnect),
        ];
        assert!(effect_deficit(&events, &coverage).is_empty());
    }

    #[test]
    fn only_an_observer_discharges_the_bash_obligation() {
        let adapter = vec![entry(
            "adapter",
            ProducerRole::Adapter,
            &["fs.read", "fs.write"],
        )];
        assert!(
            !has_fs_covered_observer(&adapter),
            "an adapter's fs.* declaration must not discharge the obligation (ADR 0006)"
        );

        let observer = vec![entry("observer", ProducerRole::Observer, &["fs.*"])];
        assert!(has_fs_covered_observer(&observer));

        let probe = vec![entry("probe", ProducerRole::Probe, &["fs.*"])];
        assert!(
            !has_fs_covered_observer(&probe),
            "a probe does not observe subprocess filesystem effects"
        );
    }

    #[test]
    fn deficit_details_are_deterministic() {
        let coverage = vec![entry("adapter", ProducerRole::Adapter, &[])];
        let events = vec![
            event("adapter", EventKind::NetFetch),
            event("adapter", EventKind::FsRead),
            event("adapter", EventKind::FsRead),
        ];
        let a = effect_deficit_detail(&effect_deficit(&events, &coverage));
        let b = effect_deficit_detail(&effect_deficit(&events, &coverage));
        assert_eq!(a, b);
        assert_eq!(a, "uncovered-effect-kinds=fs.read,net.fetch");
    }
}
