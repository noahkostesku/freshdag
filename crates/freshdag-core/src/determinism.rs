//! Injected time and identity sources.
//!
//! `IrEvent::event_id` is a UUIDv7 (which embeds wall-clock time) and
//! `IrEvent::ts` is wall-clock. Both are ambient non-determinism, and
//! ambient non-determinism is incompatible with the golden-file
//! conformance harnesses `docs/contracts/adapter-contract.md §Testing`
//! and `docs/contracts/observer-contract.md §Testing` require, and with
//! `.claude/rules/testing.md` ("if a test depends on time, mock it").
//!
//! So a producer's compile or parse path never calls
//! `OffsetDateTime::now_utc()` or `Uuid::now_v7()` directly. It calls a
//! [`Clock`] and an [`IdGen`] its caller supplies.
//!
//! # What lives here, and what deliberately does not
//!
//! This module holds the **traits** and the **deterministic**
//! implementations. [`FixedClock`] and [`SeededIdGen`] are pure — a base
//! instant plus a counter — so they belong to the domain model as
//! naturally as any other value type.
//!
//! The *ambient* implementations do NOT live here. A `SystemClock`
//! calling `now_utc()`, or a `UuidV7Gen` calling `now_v7()`, reads the
//! environment, and `ARCHITECTURE.md` says of this crate: "It has no
//! I/O and no dependency on any runtime." Each producer crate keeps its
//! own six-line ambient pair. That is not a failure to deduplicate — it
//! is the boundary doing its job: core owns the contract and the
//! reproducible half, and every crate owns its own read of the world.
//!
//! Both were previously defined twice, in `freshdag-adapter-claude` and
//! `freshdag-observer`, with the observer's `SeededIdGen` producing
//! `Uuid::from_u128(counter)` — not UUIDv7-shaped at all, despite
//! `docs/contracts/execution-ir.md` calling `event_id` a UUIDv7. The
//! unification adopts the adapter's shape, which is correct, and that
//! correction is why the observer's goldens move in the same commit.
//!
//! Unifying them also removed an *accidental* disambiguator: two
//! generators in disjoint id spaces became one shared counter, so both
//! conformance harnesses minted the same `event_id`s and any store
//! ingesting both streams saw cross-producer duplicates. Producers now
//! seed with [`SeededIdGen::for_producer`], which restores disjointness
//! deliberately rather than by accident.

use std::cell::Cell;

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Source of event timestamps.
pub trait Clock: core::fmt::Debug {
    /// The time to stamp the next emitted event with.
    fn now(&self) -> OffsetDateTime;
}

/// Source of event identifiers.
///
/// Implementations MUST return strictly increasing identifiers so that
/// the per-producer total order defined by
/// `docs/contracts/execution-ir.md §Ordering` holds.
pub trait IdGen: core::fmt::Debug {
    /// Mint the next event identifier.
    fn next_id(&mut self) -> Uuid;
}

/// The fixed base instant used by [`FixedClock::conformance`] and
/// [`SeededIdGen`]: `2026-01-01T00:00:00Z`.
pub const CONFORMANCE_EPOCH_UNIX_MS: u64 = 1_767_225_600_000;

/// A clock that starts at a fixed instant and advances by a fixed step
/// on every call to [`Clock::now`].
///
/// Used by the conformance harness so golden IR streams are stable.
#[derive(Debug)]
pub struct FixedClock {
    base: OffsetDateTime,
    step: Duration,
    calls: Cell<u32>,
}

impl FixedClock {
    /// Construct with an explicit base instant and per-call step.
    #[must_use]
    pub fn new(base: OffsetDateTime, step: Duration) -> Self {
        Self {
            base,
            step,
            calls: Cell::new(0),
        }
    }

    /// The canonical conformance clock: base `2026-01-01T00:00:00Z`,
    /// one millisecond per event.
    ///
    /// # Panics
    ///
    /// Never in practice: [`CONFORMANCE_EPOCH_UNIX_MS`] is a valid
    /// Unix timestamp.
    #[must_use]
    pub fn conformance() -> Self {
        let nanos = i128::from(CONFORMANCE_EPOCH_UNIX_MS) * 1_000_000;
        let base = OffsetDateTime::from_unix_timestamp_nanos(nanos)
            .expect("CONFORMANCE_EPOCH_UNIX_MS is a valid Unix timestamp");
        Self::new(base, Duration::milliseconds(1))
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        let n = self.calls.get();
        self.calls.set(n + 1);
        self.base + self.step * n
    }
}

/// A deterministic, monotonically increasing, `UUIDv7`-shaped identifier
/// generator.
///
/// The 48-bit timestamp field is pinned to
/// [`CONFORMANCE_EPOCH_UNIX_MS`]; the low 48 bits carry a 16-bit
/// **producer tag** followed by a 32-bit counter. Version (7) and RFC
/// 4122 variant bits are set correctly so the values are
/// indistinguishable from real `UUIDv7`s to any consumer that only
/// inspects the version and orders lexicographically.
///
/// # Why the producer tag exists
///
/// `event_id` is unique *per producer* by
/// `docs/contracts/execution-ir.md §Event Envelope`, but
/// `freshdag_store::order::linearize_checked` detects duplicates
/// **globally** — it cannot do otherwise, since a duplicate across
/// producers is equally fatal to a reproducible linearization. When
/// every conformance harness drew from [`SeededIdGen::conformance`],
/// two individually conformant streams shared `event_id`s, and
/// ingesting both into one store yielded `is_deterministic() == false`.
/// Producers therefore seed with [`SeededIdGen::for_producer`], which
/// gives each producer name a disjoint identifier space.
#[derive(Debug, Clone)]
pub struct SeededIdGen {
    epoch_ms: u64,
    producer_tag: u16,
    counter: u32,
}

impl SeededIdGen {
    /// Construct with an explicit epoch and starting counter, in the
    /// untagged (producer tag `0`) space.
    #[must_use]
    pub fn new(epoch_ms: u64, start: u32) -> Self {
        Self {
            epoch_ms,
            producer_tag: 0,
            counter: start,
        }
    }

    /// The canonical *untagged* conformance generator: conformance
    /// epoch, producer tag `0`, counter starting at 1.
    ///
    /// Use this only where a single stream is under test and no other
    /// producer's events can reach the same store. Anything that emits
    /// a real IR stream — every adapter and observer — must use
    /// [`SeededIdGen::for_producer`] instead.
    #[must_use]
    pub fn conformance() -> Self {
        Self::new(CONFORMANCE_EPOCH_UNIX_MS, 1)
    }

    /// The conformance generator for one producer identity: conformance
    /// epoch, a producer tag derived from `producer`, counter starting
    /// at 1.
    ///
    /// Deterministic in `producer` alone, so goldens are stable, and
    /// disjoint from every other producer name that derives a different
    /// tag — which is what keeps two conformant streams from colliding
    /// in a shared store.
    ///
    /// The tag is never `0`: that value is reserved for
    /// [`SeededIdGen::conformance`], so a producer can never
    /// accidentally land in the untagged space.
    #[must_use]
    pub fn for_producer(producer: &str) -> Self {
        Self {
            epoch_ms: CONFORMANCE_EPOCH_UNIX_MS,
            producer_tag: producer_tag(producer),
            counter: 1,
        }
    }
}

/// Derive a non-zero 16-bit tag from a producer name.
///
/// FNV-1a, folded to 16 bits. Core deliberately does not hold a
/// registry of producer names — that would be an adapter concept inside
/// `freshdag-core` (invariant #14) — so the tag is derived from the name
/// the producer already declares in `IrEvent::producer`.
fn producer_tag(producer: &str) -> u16 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in producer.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Fold the 64-bit hash down to 16 bits so every byte contributes.
    // Truncation is the point, so take the low bytes explicitly rather
    // than casting.
    let low = (hash ^ (hash >> 16) ^ (hash >> 32) ^ (hash >> 48)).to_le_bytes();
    let folded = u16::from_le_bytes([low[0], low[1]]);
    // 0 is reserved for the untagged conformance space.
    if folded == 0 {
        u16::MAX
    } else {
        folded
    }
}

impl Default for SeededIdGen {
    fn default() -> Self {
        Self::conformance()
    }
}

impl IdGen for SeededIdGen {
    /// # Panics
    ///
    /// If more than `u32::MAX` identifiers are drawn from one
    /// generator. Wrapping would silently re-issue identifiers the
    /// store treats as a producer bug, so this fails loudly instead.
    fn next_id(&mut self) -> Uuid {
        let n = self.counter;
        self.counter = self
            .counter
            .checked_add(1)
            .expect("a SeededIdGen drew more than u32::MAX identifiers");

        let mut bytes = [0u8; 16];
        // Bytes 0..6: 48-bit big-endian Unix-millis timestamp field.
        bytes[0..6].copy_from_slice(&self.epoch_ms.to_be_bytes()[2..8]);
        // Byte 6 high nibble: version 7.
        bytes[6] = 0x70;
        bytes[7] = 0x00;
        // Byte 8 high bits: RFC 4122 variant (0b10).
        bytes[8] = 0x80;
        bytes[9] = 0x00;
        // Bytes 10..12: 16-bit big-endian producer tag. Fixed for the
        // life of the generator, so it never perturbs monotonicity.
        bytes[10..12].copy_from_slice(&self.producer_tag.to_be_bytes());
        // Bytes 12..16: 32-bit big-endian counter (monotonic).
        bytes[12..16].copy_from_slice(&n.to_be_bytes());

        Uuid::from_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seeded generator must produce values a consumer cannot tell
    /// from real `UUIDv7`s.
    ///
    /// `docs/contracts/execution-ir.md` calls `event_id` a UUIDv7, and a
    /// deterministic generator is still bound by that. Before this
    /// module was shared, `freshdag-observer` had its own generator
    /// returning `Uuid::from_u128(counter)` — version nibble 0, no
    /// variant bits, not a UUIDv7 by any reading. It shipped goldens
    /// full of `00000000-0000-0000-0000-000000000001` and nothing
    /// noticed, because nothing asserted the shape. This is that
    /// assertion.
    #[test]
    fn seeded_ids_are_indistinguishable_from_real_uuid_v7s() {
        let mut ids = SeededIdGen::conformance();
        for _ in 0..8 {
            let id = ids.next_id();
            assert_eq!(id.get_version_num(), 7, "{id} is not version 7");
            // RFC 4122 variant: the two high bits of byte 8 are 0b10.
            assert_eq!(
                id.as_bytes()[8] & 0xC0,
                0x80,
                "{id} has the wrong variant bits"
            );
        }
    }

    /// Ordering is contractual: `execution-ir.md §Ordering` defines a
    /// per-producer total order, and a generator that went backwards
    /// would break replay determinism rather than merely look odd.
    #[test]
    fn seeded_ids_increase_strictly() {
        let mut ids = SeededIdGen::conformance();
        let mut prev = ids.next_id();
        for _ in 0..64 {
            let next = ids.next_id();
            assert!(next > prev, "{next} did not follow {prev}");
            prev = next;
        }
    }

    /// Two generators built the same way agree event for event — the
    /// property every golden file rests on.
    #[test]
    fn two_conformance_generators_agree() {
        let (mut a, mut b) = (SeededIdGen::conformance(), SeededIdGen::conformance());
        for _ in 0..16 {
            assert_eq!(a.next_id(), b.next_id());
        }
    }

    /// Two producers must never mint the same identifier.
    ///
    /// This is the property whose absence made two individually
    /// conformant streams collide: `linearize_checked` detects duplicate
    /// `event_id`s globally, so a shared counter across producers turned
    /// a legal ingest into `is_deterministic() == false`.
    #[test]
    fn different_producers_draw_from_disjoint_id_spaces() {
        let names = [
            "freshdag-adapter-claude",
            "freshdag-observer-fsatrace",
            "freshdag-adapter-langgraph",
        ];

        let mut seen: std::collections::BTreeSet<Uuid> = std::collections::BTreeSet::new();
        for name in names {
            let mut ids = SeededIdGen::for_producer(name);
            for _ in 0..256 {
                assert!(
                    seen.insert(ids.next_id()),
                    "{name} collided with another producer"
                );
            }
        }

        // And none of them lands in the untagged conformance space.
        let mut untagged = SeededIdGen::conformance();
        for _ in 0..256 {
            assert!(
                !seen.contains(&untagged.next_id()),
                "a producer collided with the untagged conformance space"
            );
        }
    }

    /// Tagging must not cost the per-producer total order that
    /// `execution-ir.md §Ordering` requires.
    #[test]
    fn a_tagged_generator_is_still_strictly_increasing() {
        let mut ids = SeededIdGen::for_producer("freshdag-observer-fsatrace");
        let mut prev = ids.next_id();
        for _ in 0..64 {
            let next = ids.next_id();
            assert!(next > prev, "{next} did not follow {prev}");
            assert_eq!(next.get_version_num(), 7, "{next} is not version 7");
            prev = next;
        }
    }

    /// Goldens rest on the tag depending on the producer name and
    /// nothing else.
    #[test]
    fn for_producer_is_reproducible_from_the_name_alone() {
        let (mut a, mut b) = (
            SeededIdGen::for_producer("freshdag-adapter-claude"),
            SeededIdGen::for_producer("freshdag-adapter-claude"),
        );
        for _ in 0..16 {
            assert_eq!(a.next_id(), b.next_id());
        }
    }

    /// The reserved tag has to be unreachable, or a producer could land
    /// in the untagged space and the disjointness argument collapses.
    #[test]
    fn no_producer_name_derives_the_reserved_zero_tag() {
        for i in 0..5_000u32 {
            assert_ne!(producer_tag(&format!("producer-{i}")), 0);
        }
        assert_ne!(producer_tag(""), 0);
    }

    /// The fixed clock advances by its step and never reads the wall
    /// clock; two runs must line up exactly.
    #[test]
    fn the_fixed_clock_is_reproducible_and_monotonic() {
        let (a, b) = (FixedClock::conformance(), FixedClock::conformance());
        let mut prev = a.now();
        assert_eq!(prev, b.now());
        for _ in 0..16 {
            let next = a.now();
            assert!(next > prev, "the conformance clock went backwards");
            assert_eq!(next, b.now(), "two conformance clocks diverged");
            prev = next;
        }
    }
}
