//! Injected time and identity sources for the fsatrace parser.
//!
//! `IrEvent::event_id` and `IrEvent::ts` are ambient non-determinism,
//! and ambient non-determinism is incompatible with the golden-file
//! conformance harness `docs/contracts/observer-contract.md §Testing`
//! requires, and with `.claude/rules/testing.md` ("if a test depends on
//! time, mock it; if it depends on randomness, seed it").
//!
//! So [`crate::linux::parse_fsatrace_lines_with`] never calls
//! `OffsetDateTime::now_utc()` or `Uuid::new_v4()`. It calls a [`Clock`]
//! and an [`IdGen`] the caller supplies. Production wires
//! [`SystemClock`] + [`UuidV7Gen`]; the conformance harness wires
//! [`FixedClock`] + [`SeededIdGen`].
//!
//! # Duplication, deliberately flagged
//!
//! `freshdag-adapter-claude::determinism` defines the same four types
//! for the same reason, and shares [`CONFORMANCE_EPOCH_UNIX_MS`] so the
//! two crates' golden streams sit on one timeline. They are duplicated
//! rather than shared because an observer must not depend on an adapter
//! crate — `CLAUDE.md`'s "adapters do not leak" cuts both directions,
//! and the dependency edge would be backwards besides.
//!
//! Promoting `Clock`/`IdGen` into `freshdag-core` is the obvious fix and
//! is deliberately NOT done here: `freshdag-core` is the contract
//! surface, so adding a shared abstraction to it is a contract change
//! needing `architect` sign-off, not something to smuggle in with a
//! fixture set.

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
/// Implementations MUST return strictly increasing identifiers so the
/// per-producer total order in `docs/contracts/execution-ir.md
/// §Ordering` holds.
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

/// The fixed base instant the conformance harness runs on:
/// `2026-01-01T00:00:00Z`. Identical to the adapter crate's, so the two
/// golden sets share one timeline.
pub const CONFORMANCE_EPOCH_UNIX_MS: u64 = 1_767_225_600_000;

/// A clock starting at a fixed instant and advancing a fixed step per
/// call, so golden IR streams are byte-stable.
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

    /// The clock the conformance harness uses: base
    /// [`CONFORMANCE_EPOCH_UNIX_MS`], one millisecond per call.
    ///
    /// # Panics
    ///
    /// If [`CONFORMANCE_EPOCH_UNIX_MS`] is not a representable instant,
    /// which is a compile-time-constant impossibility.
    #[must_use]
    pub fn conformance() -> Self {
        let base = OffsetDateTime::from_unix_timestamp_nanos(
            i128::from(CONFORMANCE_EPOCH_UNIX_MS) * 1_000_000,
        )
        .expect("conformance epoch is representable");
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

/// Deterministic, strictly increasing identifiers for the conformance
/// harness.
///
/// These are NOT UUIDv7 and are not claimed to be: they are a stable
/// counter rendered in UUID shape, so a golden file pins *which* event
/// carries *which* payload without pinning a random draw.
#[derive(Debug, Default)]
pub struct SeededIdGen {
    counter: u64,
}

impl SeededIdGen {
    /// A generator starting at zero.
    #[must_use]
    pub fn conformance() -> Self {
        Self::default()
    }
}

impl IdGen for SeededIdGen {
    fn next_id(&mut self) -> Uuid {
        self.counter += 1;
        Uuid::from_u128(u128::from(self.counter))
    }
}
