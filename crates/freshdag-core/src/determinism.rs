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
/// [`CONFORMANCE_EPOCH_UNIX_MS`]; the counter occupies the low bytes.
/// Version (7) and RFC 4122 variant bits are set correctly so the
/// values are indistinguishable from real `UUIDv7`s to any consumer that
/// only inspects the version and orders lexicographically.
#[derive(Debug, Clone)]
pub struct SeededIdGen {
    epoch_ms: u64,
    counter: u64,
}

impl SeededIdGen {
    /// Construct with an explicit epoch and starting counter.
    #[must_use]
    pub fn new(epoch_ms: u64, start: u64) -> Self {
        Self {
            epoch_ms,
            counter: start,
        }
    }

    /// The canonical conformance generator: conformance epoch,
    /// counter starting at 1.
    #[must_use]
    pub fn conformance() -> Self {
        Self::new(CONFORMANCE_EPOCH_UNIX_MS, 1)
    }
}

impl Default for SeededIdGen {
    fn default() -> Self {
        Self::conformance()
    }
}

impl IdGen for SeededIdGen {
    fn next_id(&mut self) -> Uuid {
        let n = self.counter;
        self.counter += 1;

        let mut bytes = [0u8; 16];
        // Bytes 0..6: 48-bit big-endian Unix-millis timestamp field.
        bytes[0..6].copy_from_slice(&self.epoch_ms.to_be_bytes()[2..8]);
        // Byte 6 high nibble: version 7.
        bytes[6] = 0x70;
        bytes[7] = 0x00;
        // Byte 8 high bits: RFC 4122 variant (0b10).
        bytes[8] = 0x80;
        bytes[9] = 0x00;
        // Bytes 10..16: 48-bit big-endian counter (monotonic).
        bytes[10..16].copy_from_slice(&n.to_be_bytes()[2..8]);

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
