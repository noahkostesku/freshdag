//! The engine's only source of "now".
//!
//! TTL expiry (`ReasonCode::TtlExpired`) and `status.checked` are both
//! functions of the current time. Reading the wall clock ambiently
//! would make every certificate nondeterministic — and `.claude/rules/
//! testing.md` forbids nondeterministic tests — so the clock is
//! injected and the engine never calls [`OffsetDateTime::now_utc`]
//! outside [`SystemClock`].

use std::sync::Mutex;

use time::OffsetDateTime;

/// A source of the current instant.
///
/// `Send + Sync` because [`Engine`](crate::Engine) is shareable and
/// takes `&self` on `check`.
pub trait Clock: std::fmt::Debug + Send + Sync {
    /// The current instant, in UTC.
    fn now(&self) -> OffsetDateTime;
}

/// The real wall clock. The only place in this crate that reads it.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// A clock a test drives by hand.
///
/// Interior mutability so a test can hold an `Arc<FixedClock>`,
/// hand the same `Arc` to the engine, and still advance it between
/// checks.
#[derive(Debug)]
pub struct FixedClock {
    at: Mutex<OffsetDateTime>,
}

impl FixedClock {
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
        *self.at.lock().expect("FixedClock lock poisoned") = at;
    }

    /// Move the clock forward by `delta`.
    ///
    /// # Panics
    ///
    /// If the internal lock is poisoned.
    pub fn advance(&self, delta: time::Duration) {
        let mut guard = self.at.lock().expect("FixedClock lock poisoned");
        *guard += delta;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        *self.at.lock().expect("FixedClock lock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_advances_deterministically() {
        let clock = FixedClock::new(OffsetDateTime::UNIX_EPOCH);
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
