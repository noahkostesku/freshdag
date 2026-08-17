//! The engine's only source of "now".
//!
//! TTL expiry (`ReasonCode::TtlExpired`) and `status.checked` are both
//! functions of the current time. Reading the wall clock ambiently
//! would make every certificate nondeterministic — and `.claude/rules/
//! testing.md` forbids nondeterministic tests — so the clock is
//! injected and the engine never calls [`OffsetDateTime::now_utc`]
//! outside [`SystemClock`].
//!
//! # Why this is not `freshdag_core::determinism::Clock`
//!
//! There are two clocks in this workspace and they answer different
//! questions (ADR 0013):
//!
//! - An **emission** clock stamps the next event a producer emits.
//!   That is core's `Clock`: it advances once per call, because each
//!   call belongs to one event, and it is deliberately `!Sync` — a
//!   producer's event stream is a sequence, not a shared resource.
//! - An **evaluation** clock answers "what time is it now?" for TTL
//!   arithmetic and `status.checked`. That is [`EvalClock`]: it is
//!   idempotent under repeated calls, and `Send + Sync` because
//!   [`Engine`](crate::Engine) is shareable and `check` takes `&self`.
//!
//! Merging them is not possible even in principle: core's
//! `FixedClock` holds a [`Cell`](std::cell::Cell) and so cannot satisfy
//! this trait's bounds, and an auto-advancing clock would make two TTL
//! comparisons within one `check` disagree.
//!
//! They were briefly both called `Clock`, one crate apart, with
//! incompatible bounds and opposite advance semantics. The names here
//! are deliberately different so that no future reader has to discover
//! the distinction by compiler error.

use std::sync::Mutex;

use time::OffsetDateTime;

/// A source of the current instant.
///
/// `Send + Sync` because [`Engine`](crate::Engine) is shareable and
/// takes `&self` on `check`.
pub trait EvalClock: std::fmt::Debug + Send + Sync {
    /// The current instant, in UTC.
    fn now(&self) -> OffsetDateTime;
}

/// The real wall clock. The only place in this crate that reads it.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl EvalClock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// A clock a test drives by hand.
///
/// Interior mutability so a test can hold an `Arc<FrozenClock>`,
/// hand the same `Arc` to the engine, and still advance it between
/// checks.
#[derive(Debug)]
pub struct FrozenClock {
    at: Mutex<OffsetDateTime>,
}

impl FrozenClock {
    /// A clock frozen at `at`.
    #[must_use]
    pub fn new(at: OffsetDateTime) -> Self {
        Self { at: Mutex::new(at) }
    }

    /// Move the clock to an explicit instant.
    ///
    /// # Panics
    ///
    /// If the internal lock is poisoned, which requires a panic while
    /// the lock was held.
    pub fn set(&self, at: OffsetDateTime) {
        *self.at.lock().expect("FrozenClock lock poisoned") = at;
    }

    /// Move the clock forward by `delta`.
    ///
    /// # Panics
    ///
    /// If the internal lock is poisoned.
    pub fn advance(&self, delta: time::Duration) {
        let mut guard = self.at.lock().expect("FrozenClock lock poisoned");
        *guard += delta;
    }
}

impl EvalClock for FrozenClock {
    fn now(&self) -> OffsetDateTime {
        *self.at.lock().expect("FrozenClock lock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_advances_deterministically() {
        let clock = FrozenClock::new(OffsetDateTime::UNIX_EPOCH);
        assert_eq!(clock.now(), OffsetDateTime::UNIX_EPOCH);
        clock.advance(time::Duration::seconds(90));
        assert_eq!(
            clock.now(),
            OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(90)
        );
        clock.set(OffsetDateTime::UNIX_EPOCH);
        assert_eq!(clock.now(), OffsetDateTime::UNIX_EPOCH);
    }
}
