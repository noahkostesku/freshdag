//! `ScriptedObserver` — test double that emits a pre-authored IR event
//! sequence.
//!
//! Purpose: exercise the Observer trait deterministically on any
//! platform, including in CI on macOS, without depending on fsatrace,
//! subprocesses, or filesystem state. Adversarial fixtures (rename
//! dance, mmap-pessimistic reads, symlink swaps) can be encoded here
//! before the real Linux backend supports them.

use freshdag_core::ir::{CoverageManifest, EventKindPattern, IrEvent, ProducerRole};

use crate::observer::{CommandInvocation, ObservationRun, Observer, ObserverError};

/// Observer that ignores the invocation and returns a pre-scripted
/// event sequence.
///
/// `coverage` is the manifest the scripted run pretends to have. Use
/// this to test consumer behavior under specific coverage claims
/// (including deliberately-lying coverage for adversarial tests).
#[derive(Debug, Clone)]
#[allow(clippy::struct_field_names)] // "scripted_" prefix is meaningful
pub struct ScriptedObserver {
    scripted_events: Vec<IrEvent>,
    scripted_coverage: CoverageManifest,
    scripted_exit_code: Option<i32>,
    scripted_output: String,
}

impl ScriptedObserver {
    /// Construct with a scripted event stream and coverage manifest.
    #[must_use]
    pub fn new(events: Vec<IrEvent>, coverage: CoverageManifest) -> Self {
        Self {
            scripted_events: events,
            scripted_coverage: coverage,
            scripted_exit_code: Some(0),
            scripted_output: String::new(),
        }
    }

    /// Set the exit code the scripted run will report.
    #[must_use]
    pub fn with_exit_code(mut self, code: Option<i32>) -> Self {
        self.scripted_exit_code = code;
        self
    }

    /// Set the combined-output string the scripted run will report.
    #[must_use]
    pub fn with_output(mut self, s: impl Into<String>) -> Self {
        self.scripted_output = s.into();
        self
    }

    /// A convenience "empty" coverage manifest for tests that don't
    /// care about the coverage side of the observation.
    #[must_use]
    pub fn empty_coverage(producer: &str) -> CoverageManifest {
        CoverageManifest {
            producer: producer.to_string(),
            version: "test".to_string(),
            role: ProducerRole::Observer,
            platforms: vec!["any".to_string()],
            emits: vec![EventKindPattern::from("fs.*")],
            partial: std::collections::BTreeMap::new(),
            capabilities: std::collections::BTreeMap::new(),
            known_limitations: vec!["scripted test double".to_string()],
        }
    }
}

impl Observer for ScriptedObserver {
    fn coverage(&self) -> CoverageManifest {
        self.scripted_coverage.clone()
    }

    fn observe(&self, _invocation: &CommandInvocation) -> Result<ObservationRun, ObserverError> {
        Ok(ObservationRun {
            exit_code: self.scripted_exit_code,
            combined_output: self.scripted_output.clone(),
            events: self.scripted_events.clone(),
        })
    }
}
