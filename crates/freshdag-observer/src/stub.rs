//! Stub observer for platforms without native syscall observation.
//!
//! On macOS (and anywhere fsatrace is unavailable) this observer runs
//! the requested subprocess normally and returns an empty event list.
//! Its coverage manifest explicitly declares ZERO `fs.*` coverage —
//! the certificate contract's Coverage-Deficit rule then forces any
//! computation invoking `bash`/`task` to `unknown` rather than
//! silently succeeding.
//!
//! This is invariant #7 enforced at the observation layer: absence of
//! events from a producer that admits zero coverage is honest silence,
//! not fabricated success.

use std::process::{Command, Stdio};

use freshdag_core::ir::{CoverageManifest, EventKindPattern};

use crate::observer::{CommandInvocation, ObservationRun, Observer, ObserverError};

/// Runs subprocesses without observing their I/O. Coverage manifest
/// declares zero fs/proc/net coverage.
#[derive(Debug, Clone)]
pub struct StubObserver {
    version: String,
}

impl StubObserver {
    /// Construct.
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

impl Default for StubObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl Observer for StubObserver {
    fn coverage(&self) -> CoverageManifest {
        CoverageManifest {
            producer: "freshdag-observer-stub".to_string(),
            version: self.version.clone(),
            platforms: vec!["any".to_string()],
            emits: vec![],
            partial: std::collections::BTreeMap::new(),
            capabilities: {
                let mut m: std::collections::BTreeMap<String, serde_json::Value> =
                    std::collections::BTreeMap::new();
                m.insert("fs.read".to_string(), serde_json::json!(false));
                m.insert("fs.write".to_string(), serde_json::json!(false));
                m.insert("proc.spawn".to_string(), serde_json::json!(false));
                m.insert("net.connect".to_string(), serde_json::json!(false));
                m
            },
            known_limitations: vec![
                "stub observer: runs the subprocess without instrumentation".to_string(),
                "certificates for computations that invoke bash/task on this platform MUST be capped at `unknown` by the engine's coverage-deficit rule"
                    .to_string(),
            ],
        }
    }

    fn observe(&self, invocation: &CommandInvocation) -> Result<ObservationRun, ObserverError> {
        // We still run the subprocess so higher layers see its output.
        // We just don't observe I/O.
        let output = Command::new(&invocation.program)
            .args(&invocation.args)
            .current_dir(&invocation.cwd)
            .envs(invocation.env.iter().map(|(k, v)| (k.clone(), v.clone())))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;

        Ok(ObservationRun {
            exit_code: output.status.code(),
            combined_output: format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            ),
            events: vec![],
        })
    }
}

// Keep the pattern type used for parity with real observers (helps the
// module compile whether or not the coverage manifest ever populates
// its `emits` list).
#[allow(dead_code)]
fn _emit_pattern_placeholder() -> EventKindPattern {
    EventKindPattern::from("fs.*")
}
