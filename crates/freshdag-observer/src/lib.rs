//! Systems-level observation.
//!
//! Below-tool-layer observers (subprocess syscall tracing, filesystem
//! effects) that surface path-reads, path-writes, and subprocess
//! spawns as canonical IR events. Platform-specific backends sit
//! behind the [`Observer`] trait — see
//! `docs/contracts/observer-contract.md`.
//!
//! W6.1 lands:
//! - The [`Observer`] trait + its [`CommandInvocation`] input type and
//!   [`ObservationRun`] result type.
//! - A [`linux::FsatraceObserver`] backend that wraps a subprocess and
//!   emits `fs.read` / `fs.write` IR events by delegating to the
//!   `fsatrace` binary when available.
//! - A [`stub::StubObserver`] for platforms where syscall observation
//!   is unavailable (currently: macOS) and for tests. It surfaces a
//!   coverage manifest that explicitly declares zero `fs.*` coverage
//!   so the engine's coverage-deficit rule (certificate-contract
//!   §Coverage-Deficit) can force `unknown` on any bash/task tool
//!   invocation.
//! - A [`replay::ScriptedObserver`] test double that emits a
//!   pre-scripted sequence of IR events for adversarial and unit tests.
//!
//! Native syscall observation on macOS is explicitly not supported in
//! v0 — see observer-contract §Platform Matrix. Users on macOS get
//! honest `unknown` verdicts, not silent success.

#![warn(missing_docs)]

mod observer;

pub mod determinism;
pub mod linux;
pub mod replay;
pub mod stub;

pub use observer::{CommandInvocation, ObservationRun, Observer, ObserverError};

#[cfg(test)]
mod tests;
