//! This crate's ambient time and identity sources.
//!
//! The [`Clock`] and [`IdGen`] traits, and the deterministic
//! [`FixedClock`] / [`SeededIdGen`] the conformance harness wires, live
//! in [`freshdag_core::determinism`] — they are shared with
//! `freshdag-observer` and are pure values.
//!
//! What stays here is the pair that reads the environment. Core "has no
//! I/O and no dependency on any runtime" (`ARCHITECTURE.md`), and
//! calling `now_utc()` or `now_v7()` is precisely that read. Six lines
//! per producer crate is the correct price for keeping the boundary
//! honest.

pub use freshdag_core::determinism::{
    Clock, FixedClock, IdGen, SeededIdGen, CONFORMANCE_EPOCH_UNIX_MS,
};

use time::OffsetDateTime;
use uuid::Uuid;

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
