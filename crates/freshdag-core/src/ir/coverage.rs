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
use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

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
/// Role is about **vantage point**; [`PartialReason`] is about
/// **fidelity**. They are independent axes and the coverage-deficit
/// rule needs both: an observer that declares itself
/// [`BlindInScope`](PartialReason::BlindInScope) for `fs.read` has the
/// right vantage point and still cannot answer the question, while an
/// adapter with flawless fidelity over tool inputs never sees inside a
/// subprocess. Neither field subsumes the other. (Before ADR 0011,
/// `partial` was free text, so a fidelity-based rule was untestable
/// and this comment said role was the only usable signal.)
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

/// The closed vocabulary describing **which direction** a producer's
/// partial coverage errs in.
///
/// `partial` used to be free text, which meant no machine could tell
/// "I see this event but may report it too coarsely" from "I cannot see
/// this event at all." Those two claims have opposite safety
/// consequences, so a consumer forced to guess guesses wrong half the
/// time on the invariant-#7 path. ADR 0011 closes the vocabulary for
/// exactly the reason ADR 0006 closed
/// [`ReasonCode`](crate::dependency::ReasonCode): a contract you cannot
/// test is a convention.
///
/// **The direction of the error is the entire criterion.**
/// Over-approximation yields spurious *dependencies*, hence spurious
/// staleness, which invariant #15 explicitly prefers.
/// Under-approximation and blindness yield spurious *freshness*, which
/// is the invariant-#7 violation this vocabulary exists to prevent.
///
/// See [`PartialReason::discharges`] for the machine-checked
/// consequence.
///
/// Adding a variant is a contract change (see
/// `.claude/rules/architecture.md`): it widens the enum in
/// `schemas/certificate/v0.1.json` that consumers validate against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PartialReason {
    /// May report events that did not happen, or report them more
    /// coarsely than reality — but never *misses* one of this kind.
    ///
    /// Fails safe: the surplus becomes extra dependencies, hence extra
    /// staleness. This is the only variant that discharges an
    /// observation obligation.
    OverApproximates,
    /// May miss real events of this kind.
    ///
    /// Fails unsafe: a missed read is a dependency edge that never
    /// reaches the certificate, so the artifact looks fresher than it
    /// is. Does not discharge.
    ///
    /// **Deliberately one bucket, however narrow the gap** (ADR 0012).
    /// "Misses mmap reads" and "misses everything" are the same variant
    /// on purpose: a bounded *mechanism* does not bound the *harm*,
    /// because the one dependency missed may be the only one that
    /// mattered. A finer member that discharged would be unsound, and
    /// one that did not discharge would be inert — [`Self::discharges`]
    /// is the whole machine-readable content of this vocabulary, so a
    /// variant no decision reads is free text with a schema entry.
    ///
    /// The route out is to close the gap or to over-report instead of
    /// missing: hashing a file at `open` and emitting a pessimistic
    /// read turns a miss into a coarse over-report, which does
    /// discharge (observer-contract §Required Behavior #4).
    UnderApproximates,
    /// Structurally cannot observe this kind within some scope — e.g.
    /// an adapter that sees tool inputs but nothing inside the
    /// subprocess a `bash` invocation spawns.
    ///
    /// Fails unsafe for the same reason as
    /// [`PartialReason::UnderApproximates`], and more sharply: the
    /// producer is not merely lossy, it is blind. Does not discharge.
    BlindInScope,
}

/// Every [`PartialReason`], for exhaustive tests and schema agreement.
pub const ALL_PARTIAL_REASONS: [PartialReason; 3] = [
    PartialReason::OverApproximates,
    PartialReason::UnderApproximates,
    PartialReason::BlindInScope,
];

impl PartialReason {
    /// The canonical wire string.
    ///
    /// The `match` is exhaustive by construction, so adding a variant
    /// fails to COMPILE until this table is updated; the
    /// `partial_reason_serde_and_as_wire_str_agree` and
    /// `schema_partial_reason_enum_matches_rust` tests then keep this
    /// table, serde, and the certificate schema from drifting apart.
    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::OverApproximates => "over-approximates",
            Self::UnderApproximates => "under-approximates",
            Self::BlindInScope => "blind-in-scope",
        }
    }

    /// Does a producer declaring this reason for an event kind still
    /// **discharge** an observation obligation on that kind?
    ///
    /// Only [`PartialReason::OverApproximates`] does. This single
    /// method is the whole machine-readable content of the vocabulary;
    /// nothing may re-derive the answer from
    /// [`PartialCoverage::note`].
    #[must_use]
    pub const fn discharges(self) -> bool {
        match self {
            Self::OverApproximates => true,
            Self::UnderApproximates | Self::BlindInScope => false,
        }
    }
}

impl fmt::Display for PartialReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_wire_str())
    }
}

/// One entry of [`CoverageManifest::partial`]: a machine-readable
/// [`PartialReason`] plus a non-normative human note.
///
/// This mirrors [`ValidityReason`](crate::dependency::ValidityReason)'s
/// `reason` + `detail` split, and `OpenVEX`'s `justification` +
/// `impact_statement` (`docs/NOVELTY.md §1`), and carries the same
/// rules for the free-text half:
///
/// - **`note` is NEVER load-bearing.** No consumer — engine, store,
///   CLI, UI, or third-party re-checker — may branch on its contents,
///   pattern-match it, or treat its absence as meaningful. Decisions
///   key off [`PartialCoverage::reason`] and nothing else.
/// - **`note` MUST be deterministic.** It reaches the certificate and
///   is therefore inside the `cert_id` preimage: no timestamps, PIDs,
///   ports, elapsed times, or retry counters.
/// - **`note` MUST NOT carry secrets.** Certificates are shareable.
///
/// # Wire form
///
/// Serializes as `{"reason": "...", "note": "..."}`. Deserializes from
/// *either* that object or a **bare string**, which is the pre-ADR-0011
/// shape and decodes as [`PartialReason::UnderApproximates`] — see
/// [`PartialCoverage::from_legacy_note`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct PartialCoverage {
    /// Which direction this producer's coverage of the kind errs in.
    /// This — and only this — is what consumers decide on.
    ///
    /// REQUIRED in the object wire form: there is deliberately no
    /// `#[serde(default)]`, for the same reason
    /// [`CoverageManifest::role`] has none. The *only* fallback is the
    /// legacy bare string, and it falls to the conservative answer.
    pub reason: PartialReason,
    /// Human-readable explanation. Non-normative; see the type docs.
    #[serde(default)]
    pub note: String,
}

impl PartialCoverage {
    /// Construct an entry from a reason and a note.
    #[must_use]
    pub fn new(reason: PartialReason, note: impl Into<String>) -> Self {
        Self {
            reason,
            note: note.into(),
        }
    }

    /// Interpret a pre-ADR-0011 bare-string `partial` entry.
    ///
    /// The note survives verbatim; the reason becomes
    /// [`PartialReason::UnderApproximates`].
    ///
    /// **This default is the migration's entire safety argument.** A
    /// legacy manifest cannot tell us which direction it errs in, and
    /// the two candidate answers are not symmetric: guessing
    /// `over-approximates` turns every unmigrated producer into a
    /// silent-wrong-answer generator on the invariant-#7 path, while
    /// guessing `under-approximates` costs at worst spurious staleness
    /// (invariant #15's explicit preference) until its owner
    /// reclassifies it. A producer that deserves to discharge must now
    /// say so out loud.
    #[must_use]
    pub fn from_legacy_note(note: impl Into<String>) -> Self {
        Self::new(PartialReason::UnderApproximates, note)
    }

    /// Does this declaration still discharge an observation obligation
    /// on the kind it annotates? Delegates to
    /// [`PartialReason::discharges`].
    #[must_use]
    pub const fn discharges(&self) -> bool {
        self.reason.discharges()
    }
}

impl<'de> Deserialize<'de> for PartialCoverage {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct PartialCoverageVisitor;

        impl<'de> Visitor<'de> for PartialCoverageVisitor {
            type Value = PartialCoverage;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(
                    "a partial-coverage object {\"reason\": …, \"note\": …} or a legacy \
                     bare note string (which decodes as `under-approximates`)",
                )
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(PartialCoverage::from_legacy_note(v))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(PartialCoverage::from_legacy_note(v))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut reason: Option<PartialReason> = None;
                let mut note: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "reason" => {
                            if reason.is_some() {
                                return Err(de::Error::duplicate_field("reason"));
                            }
                            reason = Some(map.next_value()?);
                        }
                        "note" => {
                            if note.is_some() {
                                return Err(de::Error::duplicate_field("note"));
                            }
                            note = Some(map.next_value()?);
                        }
                        _ => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                // No default. An object that forgot `reason` is a
                // producer bug and must fail loudly rather than be
                // guessed at in either direction.
                let reason = reason.ok_or_else(|| de::Error::missing_field("reason"))?;
                Ok(PartialCoverage {
                    reason,
                    note: note.unwrap_or_default(),
                })
            }
        }

        d.deserialize_any(PartialCoverageVisitor)
    }
}

/// The one definition of the discharge predicate (ADR 0011 §3).
///
/// A producer discharges an observation obligation for `kind` when it
/// declares it emits that kind **and** its partial declaration for that
/// kind, if any, errs in the safe direction.
///
/// Shared by [`CoverageManifest::discharges`] and
/// [`CoverageEntry::discharges`](crate::certificate::CoverageEntry::discharges)
/// so the manifest-side and certificate-side answers cannot diverge —
/// two implementations of silence semantics that disagreed is the
/// finding ADR 0011 records.
///
/// **Every** matching declaration must discharge, not just the most
/// specific one. A producer may annotate both `fs.*` and `fs.read`; if
/// the wildcard says `blind-in-scope` ("nothing inside a subprocess")
/// and the specific entry says `over-approximates`, the blindness is
/// still real. Letting the more specific key win would let a producer
/// annotate its way out of its own broadest admission.
pub(crate) fn declares_dischargeable(
    emits: &[EventKindPattern],
    partial: &BTreeMap<String, PartialCoverage>,
    kind: EventKind,
) -> bool {
    emits.iter().any(|p| p.matches(kind))
        && matching_partial(partial, kind).all(PartialCoverage::discharges)
}

/// Every partial declaration whose key matches `kind`, exact wire name
/// or wildcard pattern.
pub(crate) fn matching_partial(
    partial: &BTreeMap<String, PartialCoverage>,
    kind: EventKind,
) -> impl Iterator<Item = &PartialCoverage> {
    partial.iter().filter_map(move |(pat, entry)| {
        (pat == kind.as_wire_str() || EventKindPattern::new(pat.clone()).matches(kind))
            .then_some(entry)
    })
}

/// The partial declaration to *show* for `kind`: exact wire name first,
/// then the first matching wildcard. Presentation only — decisions use
/// [`declares_dischargeable`], which considers every match.
pub(crate) fn lookup_partial(
    partial: &BTreeMap<String, PartialCoverage>,
    kind: EventKind,
) -> Option<&PartialCoverage> {
    if let Some(entry) = partial.get(kind.as_wire_str()) {
        return Some(entry);
    }
    partial
        .iter()
        .find(|(pat, _)| EventKindPattern::new((*pat).clone()).matches(kind))
        .map(|(_, entry)| entry)
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
    /// Kinds this producer emits *partially*, keyed by event-kind
    /// pattern.
    ///
    /// Each value is a [`PartialCoverage`]: a machine-readable
    /// [`PartialReason`] plus a non-normative note. Consumers decide on
    /// the reason via [`CoverageManifest::discharges`] and MUST NOT
    /// pattern-match the note.
    ///
    /// The wire form still accepts a bare string per entry (the
    /// pre-ADR-0011 shape), which decodes as
    /// [`PartialReason::UnderApproximates`]. See
    /// [`PartialCoverage::from_legacy_note`] for why that direction.
    #[serde(default)]
    pub partial: BTreeMap<String, PartialCoverage>,
    /// Producer-specific capability claims. Free-form.
    #[serde(default)]
    pub capabilities: BTreeMap<String, serde_json::Value>,
    /// Human-readable known limitations (surfaces on the certificate).
    #[serde(default)]
    pub known_limitations: Vec<String>,
}

impl CoverageManifest {
    /// Does this manifest **syntactically** declare the given event
    /// kind, i.e. does any pattern in `emits` match?
    ///
    /// This deliberately remains purely syntactic and ignores
    /// `partial`. It answers "is this kind in the producer's
    /// vocabulary at all", which is the right question for routing,
    /// display, and deficit *enumeration*.
    ///
    /// It is NOT the right question for deciding whether a silence can
    /// be trusted or an observation obligation is discharged. Use
    /// [`CoverageManifest::discharges`] for that. ADR 0011 exists
    /// because this method's old doc comment said `partial` was "a
    /// separate consumer-side signal" and no consumer ever consulted
    /// it; quietly widening `covers` to mean both things would repeat
    /// that mistake in the other direction, silently changing every
    /// existing call site's meaning with no compiler help. A second,
    /// differently-named predicate forces each call site to say which
    /// question it is asking.
    #[must_use]
    pub fn covers(&self, kind: EventKind) -> bool {
        self.emits.iter().any(|p| p.matches(kind))
    }

    /// Does this producer **discharge an observation obligation** for
    /// the given event kind (ADR 0011 §3)?
    ///
    /// True iff it [`covers`](CoverageManifest::covers) the kind and
    /// every matching `partial` declaration errs in the safe direction
    /// ([`PartialReason::OverApproximates`]).
    ///
    /// This is the predicate that decides whether a silence from this
    /// producer means "nothing happened." A producer declaring
    /// `under-approximates` or `blind-in-scope` for the kind does not
    /// discharge, however broad its `emits` list.
    #[must_use]
    pub fn discharges(&self, kind: EventKind) -> bool {
        declares_dischargeable(&self.emits, &self.partial, kind)
    }

    /// The partial declaration for this kind, if any — reason plus
    /// note.
    ///
    /// Exact wire name wins over a wildcard. This is a *presentation*
    /// accessor; decisions must go through
    /// [`CoverageManifest::discharges`], which considers every matching
    /// declaration rather than the most specific one.
    #[must_use]
    pub fn partial_coverage(&self, kind: EventKind) -> Option<&PartialCoverage> {
        lookup_partial(&self.partial, kind)
    }

    /// The human-readable note for this kind's partial declaration, if
    /// any.
    ///
    /// Non-normative. No consumer may branch on the returned string;
    /// see [`PartialCoverage`]. Use
    /// [`CoverageManifest::partial_coverage`] when you need the reason.
    #[must_use]
    pub fn partial_note(&self, kind: EventKind) -> Option<&str> {
        self.partial_coverage(kind).map(|p| p.note.as_str())
    }
}
