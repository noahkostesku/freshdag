//! Canonical linearization and per-producer ordering.
//!
//! Implements `docs/contracts/execution-ir.md §Ordering`:
//!
//! > For any set of events being materialized into derived state, the
//! > total order is `(ts, producer, event_id)` — lexicographic on the
//! > tuple, ties broken first by producer name then by event UUID.
//!
//! This is the ordering invariant #5 relies on: replay under this rule is
//! deterministic regardless of the physical order events landed on disk.

use std::collections::BTreeMap;

use freshdag_core::ir::IrEvent;
use uuid::Uuid;

/// The canonical sort key for one event.
///
/// `ts` is compared as an **instant** — `(unix_timestamp, nanosecond)` —
/// not as a formatted string and not via `OffsetDateTime`'s own `Ord`.
/// An RFC 3339 timestamp carries a UTC offset, so `2026-01-01T12:00:00Z`
/// and `2026-01-01T13:00:00+01:00` are the same instant with different
/// spellings; comparing text (or anything offset-sensitive) would order
/// them differently on different producers and break determinism.
///
/// `nanosecond` is the count of nanoseconds past the enclosing second
/// boundary, always in `0..1_000_000_000`, counting *forward* — so
/// `(seconds, nanos)` is lexicographically correct for pre-epoch
/// timestamps too.
type OrderKey<'e> = (i64, u32, &'e str, Uuid);

fn order_key(event: &IrEvent) -> OrderKey<'_> {
    (
        event.ts.unix_timestamp(),
        event.ts.nanosecond(),
        event.producer.as_str(),
        event.event_id,
    )
}

/// Sort events into the canonical total order `(ts, producer, event_id)`.
///
/// Deterministic for any well-formed event set: `event_id` is unique per
/// producer (execution-ir.md §Event Envelope), so `(ts, producer,
/// event_id)` is a total order and the output does not depend on the
/// input order.
///
/// **Duplicate `event_id`s are a producer bug.** They are *not* deduped
/// here — dropping an observation to make a sort tidy would violate
/// invariant #4, and the store is not entitled to decide which copy was
/// real. They are kept in input-relative order (the sort is stable) and
/// the output is therefore only order-independent if the duplicates are
/// byte-identical. Use [`linearize_checked`] to find out.
pub fn linearize(events: impl Iterator<Item = IrEvent>) -> Vec<IrEvent> {
    let mut events: Vec<IrEvent> = events.collect();
    events.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    events
}

/// A duplicated `event_id` observed during linearization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateEventId {
    /// The repeated identifier.
    pub event_id: Uuid,
    /// Producer of the first copy in canonical order.
    ///
    /// Not necessarily the only one: detection is global, so a
    /// cross-producer collision reports one identifier whose copies came
    /// from different producers, and this field names only the first.
    pub producer: String,
    /// How many events carried it.
    pub count: usize,
    /// `true` if the copies are not all equal.
    ///
    /// Byte-identical copies are harmless to replay determinism (any
    /// order of them yields the same bytes). Divergent copies mean the
    /// linearization is genuinely ambiguous — the contract's total order
    /// cannot separate them — and downstream derived state must not be
    /// treated as reproducible until the producer is fixed.
    pub divergent: bool,
}

/// Canonically ordered events plus the integrity findings from ordering
/// them.
#[derive(Debug, Clone, Default)]
pub struct Linearization {
    events: Vec<IrEvent>,
    duplicate_event_ids: Vec<DuplicateEventId>,
}

impl Linearization {
    /// The canonically ordered events.
    pub fn events(&self) -> &[IrEvent] {
        &self.events
    }

    /// Take the canonically ordered events.
    pub fn into_events(self) -> Vec<IrEvent> {
        self.events
    }

    /// Duplicated `event_id`s, sorted by identifier.
    pub fn duplicate_event_ids(&self) -> &[DuplicateEventId] {
        &self.duplicate_event_ids
    }

    /// Is this ordering reproducible from any physical input order?
    ///
    /// False when the same `event_id` carried different content twice —
    /// whether one producer emitted it twice, or **two producers each
    /// emitted it once**.
    ///
    /// The check is deliberately global rather than per producer.
    /// `execution-ir.md §Event Envelope` only makes `event_id` unique
    /// *within* a producer, but the canonical order is
    /// `(ts, producer, event_id)` and duplicates are never deduped, so a
    /// collision across producers is equally fatal to reproducibility:
    /// the tuple still separates the two events, but
    /// [`DuplicateEventId`] cannot say which copy belongs to which
    /// stream, and a consumer keying derived state by `event_id` sees
    /// one identifier with two meanings. Producers keep their spaces
    /// disjoint via `freshdag_core::determinism::SeededIdGen::for_producer`.
    pub fn is_deterministic(&self) -> bool {
        !self.duplicate_event_ids.iter().any(|d| d.divergent)
    }
}

/// [`linearize`], plus detection of duplicate `event_id`s.
///
/// Prefer this wherever the result feeds derived state: it is the only
/// way to learn that a replay is *not* reproducible, and per invariant #7
/// an unreproducible replay must not be presented as if it were sound.
pub fn linearize_checked(events: impl Iterator<Item = IrEvent>) -> Linearization {
    let events = linearize(events);

    let mut by_id: BTreeMap<Uuid, Vec<&IrEvent>> = BTreeMap::new();
    for e in &events {
        by_id.entry(e.event_id).or_default().push(e);
    }
    let duplicate_event_ids = by_id
        .into_iter()
        .filter(|(_, group)| group.len() > 1)
        .map(|(event_id, group)| DuplicateEventId {
            event_id,
            producer: group[0].producer.clone(),
            count: group.len(),
            divergent: group.iter().any(|e| *e != group[0]),
        })
        .collect();

    Linearization {
        events,
        duplicate_event_ids,
    }
}

/// A producer emitted events out of `event_id` order.
///
/// Adapters and observers MUST NOT reorder events they emit
/// (execution-ir.md §Ordering), so this is a producer bug. The store
/// reports it rather than quietly correcting it, because an out-of-order
/// producer may also be mis-stamping `ts`, which would corrupt the
/// canonical linearization in a way the store cannot detect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducerOrderViolation {
    /// The offending producer.
    pub producer: String,
    /// The last `event_id` seen from this producer before the violation.
    pub previous: Uuid,
    /// The `event_id` that went backwards.
    pub observed: Uuid,
    /// Index of the offending event in the physical log order.
    pub log_index: usize,
}

/// Per-producer event streams.
///
/// Within a producer the order is total by `event_id`: UUIDv7 places a
/// millisecond timestamp in its most significant bits, and `Uuid`'s
/// ordering is big-endian bytewise, so sorting by `event_id` is sorting
/// by emission time. (A producer that mixes UUID versions still gets a
/// total, deterministic order — just not a temporal one.)
#[derive(Debug, Clone, Default)]
pub struct ProducerStreams {
    streams: BTreeMap<String, Vec<IrEvent>>,
    violations: Vec<ProducerOrderViolation>,
}

impl ProducerStreams {
    /// Group events (given in physical log order) by producer, sorting
    /// each stream by `event_id` and recording any producer that emitted
    /// out of order.
    pub fn from_log_order(events: impl IntoIterator<Item = IrEvent>) -> Self {
        let mut streams: BTreeMap<String, Vec<IrEvent>> = BTreeMap::new();
        let mut violations = Vec::new();

        for (log_index, event) in events.into_iter().enumerate() {
            let stream = streams.entry(event.producer.clone()).or_default();
            if let Some(previous) = stream.last() {
                if event.event_id < previous.event_id {
                    violations.push(ProducerOrderViolation {
                        producer: event.producer.clone(),
                        previous: previous.event_id,
                        observed: event.event_id,
                        log_index,
                    });
                }
            }
            stream.push(event);
        }

        for stream in streams.values_mut() {
            stream.sort_by_key(|e| e.event_id);
        }

        Self {
            streams,
            violations,
        }
    }

    /// Producer identities present, in lexicographic order.
    pub fn producers(&self) -> impl Iterator<Item = &str> {
        self.streams.keys().map(String::as_str)
    }

    /// One producer's stream, in `event_id` order.
    pub fn stream(&self, producer: &str) -> Option<&[IrEvent]> {
        self.streams.get(producer).map(Vec::as_slice)
    }

    /// Producers that emitted events out of `event_id` order.
    pub fn violations(&self) -> &[ProducerOrderViolation] {
        &self.violations
    }

    /// All streams concatenated, producers in lexicographic order.
    ///
    /// This is *not* the canonical replay order — use [`linearize`] for
    /// that. It is the "producer order" view: per producer, totally
    /// ordered by `event_id`.
    pub fn into_events(self) -> Vec<IrEvent> {
        self.streams.into_values().flatten().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{event, event_at, ts, uuid};
    use time::UtcOffset;

    #[test]
    fn sorts_by_timestamp_first() {
        let a = event("p", "s", ts(30), "00000000-0000-7000-8000-00000000000a");
        let b = event("p", "s", ts(10), "00000000-0000-7000-8000-00000000000b");
        let out = linearize(vec![a.clone(), b.clone()].into_iter());
        assert_eq!(out, vec![b, a], "ts dominates event_id");
    }

    #[test]
    fn ties_on_ts_break_by_producer_then_event_id() {
        // Same instant; producer "aaa" must precede "bbb" even though its
        // event_id sorts later.
        let a = event("bbb", "s", ts(10), "00000000-0000-7000-8000-000000000001");
        let b = event("aaa", "s", ts(10), "00000000-0000-7000-8000-000000000009");
        let out = linearize(vec![a.clone(), b.clone()].into_iter());
        assert_eq!(out, vec![b, a]);

        // Same instant, same producer: event_id breaks the tie.
        let c = event("aaa", "s", ts(10), "00000000-0000-7000-8000-000000000009");
        let d = event("aaa", "s", ts(10), "00000000-0000-7000-8000-000000000001");
        let out = linearize(vec![c.clone(), d.clone()].into_iter());
        assert_eq!(out, vec![d, c]);
    }

    #[test]
    fn equal_instants_in_different_offsets_order_identically() {
        // 12:00:00Z and 13:00:00+01:00 are the same instant. They must
        // tie on `ts` and fall through to the producer tiebreak, in both
        // input orders.
        let utc = ts(0) + time::Duration::hours(12);
        let plus_one = utc.to_offset(UtcOffset::from_hms(1, 0, 0).unwrap());
        assert_ne!(
            utc.to_string(),
            plus_one.to_string(),
            "test is vacuous unless the spellings differ"
        );

        let z = event_at(
            "bbb",
            "s",
            utc,
            uuid("00000000-0000-7000-8000-000000000001"),
        );
        let offset = event_at(
            "aaa",
            "s",
            plus_one,
            uuid("00000000-0000-7000-8000-000000000009"),
        );

        let forward = linearize(vec![z.clone(), offset.clone()].into_iter());
        let backward = linearize(vec![offset.clone(), z.clone()].into_iter());
        assert_eq!(forward, backward);
        assert_eq!(
            forward,
            vec![offset, z],
            "producer tiebreak, not offset spelling, decides"
        );
    }

    #[test]
    fn pre_epoch_timestamps_order_correctly() {
        let before = ts(0) - time::Duration::nanoseconds(500_000_000);
        let after = ts(0);
        let a = event_at(
            "p",
            "s",
            after,
            uuid("00000000-0000-7000-8000-000000000001"),
        );
        let b = event_at(
            "p",
            "s",
            before,
            uuid("00000000-0000-7000-8000-000000000002"),
        );
        let out = linearize(vec![a.clone(), b.clone()].into_iter());
        assert_eq!(out, vec![b, a]);
    }

    #[test]
    fn nanosecond_precision_is_honored() {
        let base = ts(10);
        let a = event_at(
            "p",
            "s",
            base + time::Duration::nanoseconds(2),
            uuid("00000000-0000-7000-8000-000000000001"),
        );
        let b = event_at(
            "p",
            "s",
            base + time::Duration::nanoseconds(1),
            uuid("00000000-0000-7000-8000-000000000002"),
        );
        let out = linearize(vec![a.clone(), b.clone()].into_iter());
        assert_eq!(out, vec![b, a], "one nanosecond apart must not tie");
    }

    #[test]
    fn duplicate_event_ids_are_surfaced_never_deduped() {
        let a = event("p", "s", ts(10), "00000000-0000-7000-8000-000000000001");
        let mut divergent = a.clone();
        divergent.session_id = "other-session".to_string();

        let out = linearize_checked(vec![a.clone(), divergent].into_iter());
        assert_eq!(out.events().len(), 2, "both copies retained");
        assert_eq!(out.duplicate_event_ids().len(), 1);
        assert_eq!(out.duplicate_event_ids()[0].count, 2);
        assert!(out.duplicate_event_ids()[0].divergent);
        assert!(!out.is_deterministic());

        // Identical copies are duplicated but not divergent.
        let out = linearize_checked(vec![a.clone(), a].into_iter());
        assert_eq!(out.events().len(), 2);
        assert!(!out.duplicate_event_ids()[0].divergent);
        assert!(out.is_deterministic());
    }

    /// Detection is global, and it has to be: this is the exact shape
    /// two conformant producers took when both harnesses drew from the
    /// same seeded generator.
    #[test]
    fn a_duplicate_across_two_producers_is_still_a_duplicate() {
        let shared = "00000000-0000-7000-8000-000000000001";
        let adapter = event("freshdag-adapter-claude", "s", ts(10), shared);
        let observer = event("freshdag-observer-fsatrace", "s", ts(10), shared);

        let out = linearize_checked(vec![adapter.clone(), observer].into_iter());
        assert_eq!(out.events().len(), 2, "both copies retained");
        assert_eq!(out.duplicate_event_ids().len(), 1);
        assert_eq!(out.duplicate_event_ids()[0].count, 2);
        assert!(
            out.duplicate_event_ids()[0].divergent,
            "different producers make the copies divergent"
        );
        assert!(
            !out.is_deterministic(),
            "a cross-producer collision is not a reproducible replay"
        );

        // The same identifier from one producer only, for contrast.
        let out = linearize_checked(vec![adapter.clone(), adapter].into_iter());
        assert!(out.is_deterministic(), "byte-identical copies are harmless");
    }

    #[test]
    fn clean_stream_is_deterministic() {
        let events = vec![
            event("b", "s", ts(20), "00000000-0000-7000-8000-000000000002"),
            event("a", "s", ts(10), "00000000-0000-7000-8000-000000000001"),
        ];
        let out = linearize_checked(events.into_iter());
        assert!(out.duplicate_event_ids().is_empty());
        assert!(out.is_deterministic());
    }

    #[test]
    fn producer_streams_sort_by_event_id_and_flag_regressions() {
        let physical = vec![
            event("a", "s", ts(10), "00000000-0000-7000-8000-000000000005"),
            event("b", "s", ts(11), "00000000-0000-7000-8000-000000000001"),
            // producer "a" goes backwards: a contract violation.
            event("a", "s", ts(12), "00000000-0000-7000-8000-000000000002"),
        ];
        let streams = ProducerStreams::from_log_order(physical);
        assert_eq!(streams.producers().collect::<Vec<_>>(), vec!["a", "b"]);
        let a = streams.stream("a").expect("stream a");
        assert_eq!(a[0].event_id, uuid("00000000-0000-7000-8000-000000000002"));
        assert_eq!(a[1].event_id, uuid("00000000-0000-7000-8000-000000000005"));
        assert_eq!(streams.violations().len(), 1);
        assert_eq!(streams.violations()[0].producer, "a");
        assert_eq!(streams.violations()[0].log_index, 2);
    }

    #[test]
    fn linearize_is_order_independent_over_permutations() {
        let events = vec![
            event("b", "s", ts(10), "00000000-0000-7000-8000-000000000004"),
            event("a", "s", ts(10), "00000000-0000-7000-8000-000000000003"),
            event("a", "s", ts(10), "00000000-0000-7000-8000-000000000002"),
            event("c", "s", ts(5), "00000000-0000-7000-8000-000000000001"),
        ];
        let canonical = linearize(events.clone().into_iter());
        // All 24 permutations must produce the same linearization.
        let idx = [0usize, 1, 2, 3];
        for p in permutations(&idx) {
            let permuted: Vec<_> = p.iter().map(|i| events[*i].clone()).collect();
            assert_eq!(linearize(permuted.into_iter()), canonical);
        }
    }

    fn permutations(items: &[usize]) -> Vec<Vec<usize>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        let mut out = Vec::new();
        for i in 0..items.len() {
            let mut rest = items.to_vec();
            let head = rest.remove(i);
            for mut tail in permutations(&rest) {
                tail.insert(0, head);
                out.push(tail);
            }
        }
        out
    }
}
