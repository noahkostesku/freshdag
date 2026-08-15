//! Injected time and identity sources.
//!
//! `IrEvent::event_id` is a UUIDv7 (which embeds wall-clock time) and
//! `IrEvent::ts` is wall-clock. Both are ambient non-determinism, and
//! ambient non-determinism is incompatible with the golden-file
//! conformance harness required by `docs/contracts/adapter-contract.md
//! §Testing` and by `.claude/rules/testing.md` ("if a test depends on
//! time, mock it").
//!
//! So the compile path never calls `OffsetDateTime::now_utc()` or
//! `Uuid::now_v7()`. It calls a [`Clock`] and an [`IdGen`] that the
//! caller supplies. Production wires [`SystemClock`] + [`UuidV7Gen`];
//! tests and fixtures wire [`FixedClock`] + [`SeededIdGen`].

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

/// Real wall-clock time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// Real UUIDv7 identifiers.
#[derive(Debug, Clone, Copy, Default)]
pub struct UuidV7Gen;

impl IdGen for UuidV7Gen {
    fn next_id(&mut self) -> Uuid {
        Uuid::now_v7()
    }
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
