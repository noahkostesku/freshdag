//! A probe whose answers are scripted, for fixtures and tests.
//!
//! `fixtures/scenarios/*/scenario.json` carries an optional
//! `input_probes` block keyed by dependency key. Wave 2 ships probes for
//! `file` and `https` only, so a scenario exercising `attio://` — or one
//! that must be reproducible without touching the filesystem or the
//! network — supplies the probe's answer directly.
//!
//! This is a **fixture** path, not a production one. A real `attio://`
//! dependency with no registered probe yields
//! [`ReasonCode::NoProbeAvailable`](freshdag_core::dependency::ReasonCode::NoProbeAvailable),
//! and that is the intended v0 behaviour rather than a gap to be papered
//! over.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use freshdag_core::dependency::Fingerprint;
use freshdag_core::probe::{Probe, ProbeResult};

/// A probe that answers from a table.
///
/// Keys are matched against the full dependency key as it appears in
/// `depends_on[].key` (`file:///repo/notes.md`, `attio://company/acme`).
/// A key with no scripted answer falls back to
/// [`ScriptedProbe::with_fallback`], and with no fallback returns
/// `Unknown { retryable: false }` — never a fabricated `Match`.
#[derive(Debug)]
pub struct ScriptedProbe {
    scheme: &'static str,
    host_pattern: Option<&'static str>,
    priority: u32,
    results: Mutex<BTreeMap<String, ProbeResult>>,
    fallback: Mutex<Option<ProbeResult>>,
}

impl ScriptedProbe {
    /// A scripted probe for `scheme`.
    ///
    /// The [`Probe`] trait requires `scheme() -> &'static str` but
    /// scenario schemes are read from JSON at runtime, so the string is
    /// leaked. One small, bounded leak per constructed probe, in a
    /// fixture-only type, is the cheaper trade against widening a core
    /// contract.
    #[must_use]
    pub fn new(scheme: &str) -> Self {
        Self {
            scheme: Box::leak(scheme.to_string().into_boxed_str()),
            host_pattern: None,
            priority: 0,
            results: Mutex::new(BTreeMap::new()),
            fallback: Mutex::new(None),
        }
    }

    /// Set this probe's arbitration priority.
    #[must_use]
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    /// Set this probe's host pattern (see probe-contract §Probe
    /// Arbitration). Leaked for the same reason as the scheme.
    #[must_use]
    pub fn with_host_pattern(mut self, pattern: &str) -> Self {
        self.host_pattern = Some(Box::leak(pattern.to_string().into_boxed_str()));
        self
    }

    /// Script an answer for one dependency key.
    #[must_use]
    pub fn with_result(self, key: impl Into<String>, result: ProbeResult) -> Self {
        self.set(key, result);
        self
    }

    /// Script the answer for any key with no explicit entry.
    #[must_use]
    pub fn with_fallback(self, result: ProbeResult) -> Self {
        *self.fallback.lock().expect("ScriptedProbe lock poisoned") = Some(result);
        self
    }

    /// Change a scripted answer after registration.
    ///
    /// Scenario harnesses use this to model "the world mutated": check
    /// once, re-script, check again.
    ///
    /// # Panics
    ///
    /// If the internal lock is poisoned.
    pub fn set(&self, key: impl Into<String>, result: ProbeResult) {
        self.results
            .lock()
            .expect("ScriptedProbe lock poisoned")
            .insert(key.into(), result);
    }
}

impl Probe for ScriptedProbe {
    fn scheme(&self) -> &'static str {
        self.scheme
    }

    fn host_pattern(&self) -> Option<&'static str> {
        self.host_pattern
    }

    fn priority(&self) -> u32 {
        self.priority
    }

    fn check(&self, key: &str, _recorded: &Fingerprint, _ttl: Option<Duration>) -> ProbeResult {
        if let Some(result) = self
            .results
            .lock()
            .expect("ScriptedProbe lock poisoned")
            .get(key)
        {
            return result.clone();
        }
        if let Some(result) = self
            .fallback
            .lock()
            .expect("ScriptedProbe lock poisoned")
            .clone()
        {
            return result;
        }
        // Absence of a script is absence of evidence. Invariant #7.
        ProbeResult::Unknown {
            reason: "no-scripted-result".to_string(),
            retryable: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use freshdag_core::dependency::{FingerprintKind, TrustClass};

    use super::*;

    fn fp() -> Fingerprint {
        Fingerprint::new(FingerprintKind::Version, "42")
    }

    #[test]
    fn an_unscripted_key_is_unknown_not_match() {
        let probe = ScriptedProbe::new("attio");
        assert!(matches!(
            probe.check("attio://company/other", &fp(), None),
            ProbeResult::Unknown { .. }
        ));
    }

    #[test]
    fn scripted_answers_can_be_changed_between_checks() {
        let probe = ScriptedProbe::new("attio").with_result(
            "attio://company/acme",
            ProbeResult::Match {
                observed_fp: fp(),
                observed_trust_class: TrustClass::Versioned,
            },
        );
        assert!(matches!(
            probe.check("attio://company/acme", &fp(), None),
            ProbeResult::Match { .. }
        ));
        probe.set(
            "attio://company/acme",
            ProbeResult::Drift {
                observed_fp: Fingerprint::new(FingerprintKind::Version, "43"),
                observed_trust_class: TrustClass::Versioned,
            },
        );
        assert!(matches!(
            probe.check("attio://company/acme", &fp(), None),
            ProbeResult::Drift { .. }
        ));
    }
}
