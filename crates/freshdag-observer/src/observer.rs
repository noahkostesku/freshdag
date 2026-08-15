//! The `Observer` trait — every observer backend implements this.
//!
//! An observer wraps a subprocess (or an entire computation) and
//! surfaces its filesystem/process/network effects as canonical IR
//! events. Backends may return `ObserverError::NotSupportedOnPlatform`
//! rather than fake observation — see observer-contract §Platform
//! Matrix.

use std::path::PathBuf;

use freshdag_core::ir::{CoverageManifest, IrEvent};
use thiserror::Error;

/// A description of the subprocess an observer is to wrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInvocation {
    /// Program (the first argv element).
    pub program: PathBuf,
    /// Remaining argv elements (not including `program`).
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Extra environment variables (merged with the observer process's
    /// env; observers MUST NOT scrub the parent env unless explicitly
    /// asked — that is a hermeticity concern, not observation).
    pub env: Vec<(String, String)>,
}

impl CommandInvocation {
    /// Convenience constructor for a program with no args.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: Vec::new(),
        }
    }

    /// Builder-style: append arguments.
    #[must_use]
    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }
}

/// The result of running an observed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationRun {
    /// Exit code (`None` if killed by signal).
    pub exit_code: Option<i32>,
    /// Combined stdout+stderr, truncated at a producer-declared limit.
    /// Adapters should NOT parse this — the canonical stream is
    /// `events`.
    pub combined_output: String,
    /// Canonical IR events emitted by observation, in producer order.
    pub events: Vec<IrEvent>,
}

/// The observer interface.
///
/// Implementations MUST publish a [`CoverageManifest`] so consumers can
/// interpret silences. An observer that cannot run on the current
/// platform MUST return `ObserverError::NotSupportedOnPlatform` from
/// [`Observer::observe`] — fabricated coverage or silent no-op is a
/// contract violation (observer-contract §Required Behavior).
pub trait Observer: Send + Sync {
    /// Static coverage manifest for this observer/backend combination.
    fn coverage(&self) -> CoverageManifest;

    /// Run the invocation and observe its effects. Blocking.
    ///
    /// # Errors
    ///
    /// - [`ObserverError::NotSupportedOnPlatform`] if the backend
    ///   cannot run here (e.g., fsatrace on macOS).
    /// - [`ObserverError::BackendMissing`] if a required external
    ///   dependency (e.g., the `fsatrace` binary) is not installed.
    /// - [`ObserverError::Spawn`] if the subprocess could not be
    ///   spawned.
    fn observe(&self, invocation: &CommandInvocation) -> Result<ObservationRun, ObserverError>;
}

/// Errors an observer may return.
///
/// **No variant means "unknown but treat as successful."** Backends
/// that cannot observe emit `NotSupportedOnPlatform` or
/// `BackendMissing`; both are surfaced to the engine, which then
/// treats the affected computation's fs-effects coverage as absent.
#[derive(Debug, Error)]
pub enum ObserverError {
    /// Backend cannot run on the current platform (e.g., fsatrace on
    /// macOS). This is expected on unsupported platforms — the
    /// coverage manifest's `platforms` list will already reflect it.
    #[error("observer backend does not support this platform: {0}")]
    NotSupportedOnPlatform(String),
    /// A required external dependency is missing (e.g., the `fsatrace`
    /// binary is not on `$PATH`).
    #[error("observer backend dependency missing: {0}")]
    BackendMissing(String),
    /// The subprocess could not be spawned.
    #[error("failed to spawn observed process: {0}")]
    Spawn(#[from] std::io::Error),
    /// The subprocess exited but its output could not be interpreted.
    #[error("failed to interpret observation output: {0}")]
    Malformed(String),
}
