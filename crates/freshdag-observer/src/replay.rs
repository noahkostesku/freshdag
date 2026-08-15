//! `ScriptedObserver` — test double that emits a pre-authored IR event
//! sequence.
//!
//! Purpose: exercise the Observer trait deterministically on any
//! platform, including in CI on macOS, without depending on fsatrace,
//! subprocesses, or filesystem state. Adversarial fixtures (rename
//! dance, mmap-pessimistic reads, symlink swaps) can be encoded here
//! before the real Linux backend supports them.
//!
//! The coverage manifest a scripted run publishes is a first-class
//! part of the double: [`ScriptedObserver::zero_coverage`] and
//! [`ScriptedObserver::full_fs_coverage`] are the two poles the
//! certificate contract's §Coverage-Deficit rule discriminates
//! between. Pick deliberately — a test that wants "this producer saw
//! nothing" but reaches for the full-coverage manifest passes
//! vacuously.

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

    /// A manifest that declares **no coverage at all** (`emits: []`).
    ///
    /// This is the manifest to reach for when a test needs a producer
    /// that admits it saw nothing: it models the macOS/no-observer
    /// posture, and it is what makes the certificate contract's
    /// §Coverage-Deficit rule bite (invariant #7 — silence from a
    /// producer that declares no coverage is not evidence of absence).
    ///
    /// Pairs with [`ScriptedObserver::full_fs_coverage`]. The two are
    /// deliberately named for what they claim, because a test that
    /// picks the wrong one passes vacuously.
    #[must_use]
    pub fn zero_coverage(producer: &str) -> CoverageManifest {
        CoverageManifest {
            producer: producer.to_string(),
            version: "test".to_string(),
            platforms: vec!["any".to_string()],
            emits: vec![],
            partial: std::collections::BTreeMap::new(),
            capabilities: std::collections::BTreeMap::new(),
            known_limitations: vec![
                "scripted test double".to_string(),
                "declares zero coverage: any silence from this producer is uninterpretable"
                    .to_string(),
            ],
        }
    }

    /// A manifest that declares **full filesystem coverage**
    /// (`emits: ["fs.*"]`).
    ///
    /// Use this when a test needs a producer that satisfies the
    /// certificate contract's §Coverage-Deficit obligation for a
    /// `bash`/`task` invocation. It claims more than any real v0
    /// backend delivers — that is the point; it is a test double, and
    /// no shipped observer returns this manifest.
    ///
    /// Pairs with [`ScriptedObserver::zero_coverage`].
    #[must_use]
    pub fn full_fs_coverage(producer: &str) -> CoverageManifest {
        CoverageManifest {
            producer: producer.to_string(),
            version: "test".to_string(),
            role: ProducerRole::Observer,
            platforms: vec!["any".to_string()],
            emits: vec![EventKindPattern::from("fs.*")],
            partial: std::collections::BTreeMap::new(),
            capabilities: std::collections::BTreeMap::new(),
            known_limitations: vec![
                "scripted test double".to_string(),
                "claims blanket fs.* coverage; no real v0 backend does".to_string(),
            ],
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
