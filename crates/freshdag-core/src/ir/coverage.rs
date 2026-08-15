//! Producer coverage manifests.
//!
//! Every adapter and observer publishes a static manifest declaring the
//! event kinds it emits, its platforms, its capabilities, and its
//! known limitations. Consumers use this to interpret silence:
//! per invariant #7, absence of an event from a producer that does not
//! declare coverage for that kind is *not* the same as "nothing
//! happened."
//!
//! See `docs/contracts/observer-contract.md` and
//! `docs/contracts/adapter-contract.md`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::kind::EventKind;

/// A wildcard-allowing event-kind pattern used in coverage manifests
/// (e.g., `"fs.*"` matches `EventKind::FsRead`, `EventKind::FsWrite`,
/// `EventKind::FsStat`, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventKindPattern(String);

impl EventKindPattern {
    /// Construct a pattern from its wire string.
    #[must_use]
    pub fn new(pattern: impl Into<String>) -> Self {
        Self(pattern.into())
    }

    /// The raw pattern string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Does this pattern match a concrete event kind?
    #[must_use]
    pub fn matches(&self, kind: EventKind) -> bool {
        let wire = kind.as_wire_str();
        match self.0.strip_suffix(".*") {
            Some(prefix) => wire.starts_with(prefix) && wire[prefix.len()..].starts_with('.'),
            None => self.0 == wire,
        }
    }
}

impl From<&str> for EventKindPattern {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for EventKindPattern {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// What vantage point a producer observes from.
///
/// Load-bearing for the coverage-deficit rule
/// (`docs/contracts/certificate-contract.md §Coverage-Deficit Rule`):
/// only an [`Observer`](ProducerRole::Observer) sees below the agent-tool
/// layer, so only an `Observer` can discharge the observation obligation
/// created by a `bash`/`task` invocation.
///
/// This is deliberately *not* expressed through `partial`. `partial` is
/// about **fidelity** — the fsatrace observer legitimately carries
/// partial notes (rename-atomic writes, mmap reads), so a partial-based
/// rule would mean nothing could ever discharge the obligation. What
/// matters here is **vantage point**, which is a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProducerRole {
    /// Compiles a runtime's telemetry into IR. Sees only what the
    /// runtime exposes at the tool boundary; blind inside subprocesses.
    Adapter,
    /// Observes below the tool layer (syscalls, filesystem, processes).
    Observer,
    /// Reports external-state freshness checks.
    Probe,
}

/// A producer's declared coverage.
///
/// This is the machine-readable version of the observer/adapter contract
/// coverage manifests. The `capabilities` map is a free-form
/// key/value grab-bag for producer-specific claims that don't fit the
/// event-kind pattern list (e.g., `"symlink_resolution":
/// "at-observation-time"` from the observer contract).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageManifest {
    /// Producer identity (matches `IrEvent::producer`).
    pub producer: String,
    /// Producer semver.
    pub version: String,
    /// What vantage point this producer observes from. REQUIRED — there
    /// is deliberately no `#[serde(default)]`, because a defaulted role
    /// is a silent-wrong-answer generator on the invariant-#7 path.
    pub role: ProducerRole,
    /// Platforms this manifest applies to (e.g., `["linux-x86_64",
    /// "linux-arm64"]`). An empty list means "any platform."
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Event kinds (or wildcard patterns like `"fs.*"`) this producer
    /// emits.
    #[serde(default)]
    pub emits: Vec<EventKindPattern>,
    /// Kinds this producer emits *partially*, with a human-readable
    /// note. Consumers should treat partial-covered silences with the
    /// same suspicion as uncovered silences.
    #[serde(default)]
    pub partial: BTreeMap<String, String>,
    /// Producer-specific capability claims. Free-form.
    #[serde(default)]
    pub capabilities: BTreeMap<String, serde_json::Value>,
    /// Human-readable known limitations (surfaces on the certificate).
    #[serde(default)]
    pub known_limitations: Vec<String>,
}

impl CoverageManifest {
    /// Does this manifest declare coverage for the given event kind?
    ///
    /// Returns `true` if any pattern in `emits` matches. Does not
    /// consider `partial` — that's a separate consumer-side signal.
    #[must_use]
    pub fn covers(&self, kind: EventKind) -> bool {
        self.emits.iter().any(|p| p.matches(kind))
    }

    /// Is coverage for this kind declared as partial? Returns the note
    /// if so.
    #[must_use]
    pub fn partial_note(&self, kind: EventKind) -> Option<&str> {
        // Match by exact kind wire-name first; then by pattern.
        let wire = kind.as_wire_str();
        if let Some(note) = self.partial.get(wire) {
            return Some(note.as_str());
        }
        for (pat, note) in &self.partial {
            if EventKindPattern::new(pat.clone()).matches(kind) {
                return Some(note.as_str());
            }
        }
        None
    }
}
