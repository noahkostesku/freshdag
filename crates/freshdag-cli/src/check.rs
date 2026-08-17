//! `freshdag check <artifact>` — the v0 surface.
//!
//! The wiring is deliberately thin: open the store, replay the
//! canonical log into a derived graph, evaluate validity, print the
//! certificate. Every judgment call about *what* the answer is lives in
//! `freshdag-engine`; this module only decides how to say it and what
//! to exit with.
//!
//! # `check` is read-only, and deliberately so for now
//!
//! `Engine::check` hands back the `probe.checked` and `diagnostic`
//! events the check itself generated, and the engine's own
//! documentation says "the caller appends these to the log". The CLI
//! does **not**, in this revision.
//!
//! Appending them is not currently sound to do from here. The engine
//! emits those events under producer `freshdag-engine` and synthesizes
//! its own `observation_coverage` entry in memory, but nothing
//! registers a coverage manifest for it in the store. Replaying a log
//! that contains them therefore produces
//! `producer-missing-from-coverage`, which caps the status at
//! `unknown`: recording a check would turn a `valid` artifact into an
//! `unknown` one on the next check. It fails in the safe direction,
//! but it is still wrong, and closing it means the engine publishing
//! its coverage manifest — an engine API change, not a CLI one. Until
//! then `check` is a pure query, which is also what a CI job running
//! against a read-only checkout wants.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use freshdag_core::artifact::ArtifactId;
use freshdag_core::certificate::Certificate;
use freshdag_core::ir::{EventKind, IrEvent};
use freshdag_engine::{Engine, EngineError, ProbeRegistry};
use freshdag_probes::{FileProbe, HttpsProbe};
use freshdag_store::{Store, StoreError, LOG_FILE_NAME};

use crate::exit::Exit;

/// How many artifact identities to list when a lookup fails.
const MAX_SUGGESTIONS: usize = 20;

/// Everything that stops `check` from producing a verdict.
#[derive(Debug)]
pub enum CheckError {
    /// The path given is not a FreshDAG store.
    NoStore {
        /// The directory that was looked in.
        path: PathBuf,
    },
    /// The store could not be read or written.
    Store(StoreError),
    /// The engine refused to emit a certificate. This is a tool
    /// failure, not a verdict: a malformed certificate says nothing
    /// about whether the artifact is fresh.
    Engine(EngineError),
    /// Nothing in the store matches the requested artifact.
    NoSuchArtifact {
        /// What the user asked for.
        requested: String,
        /// Known identities, for a suggestion list.
        known: Vec<String>,
    },
    /// A path names more than one artifact, so `check` cannot tell
    /// which one was meant. Picking the newest silently would answer a
    /// question the user did not ask.
    AmbiguousArtifact {
        /// What the user asked for.
        requested: String,
        /// The candidates.
        matches: Vec<String>,
    },
    /// The certificate could not be serialized for `--json`.
    Json(serde_json::Error),
    /// The computation carries no recipe identity, so no certificate
    /// may claim `valid` or `likely-valid` (invariant #9, certificate
    /// contract §Field Rules).
    ///
    /// This is a **verdict**, not a tool failure, which is why it is
    /// lifted out of [`CheckError::Engine`]. Every dependency may have
    /// verified; what is missing is the identity of the computation
    /// they belong to, and an artifact that cannot be tied to its
    /// producer is not provably valid. Exit 2 says exactly that.
    NoRecipeIdentity {
        /// The engine's own account of the refusal.
        detail: String,
    },
}

impl CheckError {
    /// The exit code this failure maps to. Always `> 2`: none of these
    /// is a statement about the artifact.
    pub fn exit(&self) -> Exit {
        match self {
            Self::NoStore { .. } | Self::Store(_) | Self::Json(_) => Exit::StoreError,
            Self::Engine(_) => Exit::EngineRefused,
            Self::NoSuchArtifact { .. } | Self::AmbiguousArtifact { .. } => Exit::Usage,
            // Not `> 2`: this one *is* a statement about the artifact.
            Self::NoRecipeIdentity { .. } => Exit::Unknown,
        }
    }
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoStore { path } => write!(
                f,
                "no FreshDAG store at `{}` (expected `{LOG_FILE_NAME}` inside it)",
                path.display()
            ),
            Self::Store(e) => write!(f, "store error: {e}"),
            Self::Json(e) => write!(f, "the certificate could not be serialized: {e}"),
            Self::NoRecipeIdentity { detail } => write!(
                f,
                "unknown — this computation has no recipe identity, so no certificate \
                 may claim it valid ({detail}).\n\
                 Every dependency may have verified; what is missing is the identity \
                 of the computation that produced the artifact (invariant #9). \
                 Unknown is not fresh: do not reuse it on the strength of this check."
            ),
            Self::Engine(e) => write!(
                f,
                "the engine refused to emit a certificate: {e}\n\
                 This is a tool failure, not a verdict about the artifact."
            ),
            Self::NoSuchArtifact { requested, known } => {
                write!(f, "no artifact `{requested}` in this store")?;
                if known.is_empty() {
                    write!(f, "; the store records no artifacts at all")
                } else {
                    write!(f, ". Known artifacts:")?;
                    for k in known {
                        write!(f, "\n  {k}")?;
                    }
                    Ok(())
                }
            }
            Self::AmbiguousArtifact {
                requested,
                matches: candidates,
            } => {
                write!(
                    f,
                    "`{requested}` names {} artifacts; re-run with the one you mean:",
                    candidates.len()
                )?;
                for m in candidates {
                    write!(f, "\n  {m}")?;
                }
                Ok(())
            }
        }
    }
}

impl From<StoreError> for CheckError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// A completed check.
#[derive(Debug)]
pub struct Checked {
    /// The certificate the engine emitted.
    pub certificate: Certificate,
    /// Non-fatal notes for stderr (a probe that could not be built, an
    /// event the store dropped). Never silently swallowed.
    pub warnings: Vec<String>,
}

/// Run the check.
///
/// # Errors
///
/// Any [`CheckError`]; every one exits `> 2`.
pub fn run(store_root: &Path, artifact: &str) -> Result<Checked, CheckError> {
    // Checked before `Store::open`, which would otherwise create the
    // directory: a query must not fabricate the thing it queries.
    if !store_root.join(LOG_FILE_NAME).is_file() {
        return Err(CheckError::NoStore {
            path: store_root.to_path_buf(),
        });
    }

    let store = Store::open(store_root)?;
    let events = store.read_log()?;
    let artifact_id = resolve(&events, artifact)?;

    let mut warnings = Vec::new();
    let registry = probes(&mut warnings);

    // `build` is where the canonical log is replayed into the derived
    // graph (invariant #5: derived state is a function of the log).
    let engine = Engine::builder()
        .events(events)
        .coverage(store.coverage().clone())
        .probes(registry)
        .build();

    let outcome = engine.check(&artifact_id).map_err(|err| match err {
        // The engine refuses to seal a `valid`/`likely-valid`
        // certificate without a recipe hash. For an adapter that cannot
        // supply one — Claude Code exposes no recipe — that refusal is
        // not a defect, it is the honest ceiling, and reporting it as a
        // tool error (exit 3, "ignore this result") would be a worse
        // signal to a CI job than reporting it as unknown (exit 2, "do
        // not reuse").
        EngineError::MissingRecipeHash { .. } => CheckError::NoRecipeIdentity {
            detail: err.to_string(),
        },
        other => CheckError::Engine(other),
    })?;

    Ok(Checked {
        certificate: outcome.certificate,
        warnings,
    })
}

/// Serialize a certificate for `--json`.
///
/// # Errors
///
/// [`CheckError::Json`] if serialization fails.
pub fn to_json(certificate: &Certificate) -> Result<String, CheckError> {
    serde_json::to_string_pretty(certificate).map_err(CheckError::Json)
}

/// The probes `check` dispatches to.
///
/// A probe that cannot be constructed is *left out*, which makes its
/// scheme's dependencies `no-probe-available` → `unknown`. It is never
/// substituted for, and its absence is never silent: the caller prints
/// the warning.
fn probes(warnings: &mut Vec<String>) -> ProbeRegistry {
    let mut registry = ProbeRegistry::new();

    // Absolute `file://` keys ignore the base directory; relative ones
    // resolve against the invoking directory, which is what a user
    // typing `freshdag check` in a repo expects.
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Err(e) = registry.register(Arc::new(FileProbe::new(base))) {
        warnings.push(format!("could not register the file:// probe: {e}"));
    }

    match HttpsProbe::new() {
        Ok(probe) => {
            if let Err(e) = registry.register(Arc::new(probe)) {
                warnings.push(format!("could not register the https:// probe: {e}"));
            }
        }
        Err(e) => warnings.push(format!(
            "no https:// probe: {e}. https dependencies will report \
             `no-probe-available`, i.e. unknown — never fresh."
        )),
    }

    registry
}

/// Resolve a user-supplied selector to an artifact id.
///
/// Accepts the artifact id itself, or the path the `artifact.produced`
/// event recorded. A path matching several artifacts is an error
/// rather than a guess.
fn resolve(events: &[IrEvent], selector: &str) -> Result<ArtifactId, CheckError> {
    let mut by_path: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut ids: BTreeSet<&str> = BTreeSet::new();

    for event in events {
        if event.kind != EventKind::ArtifactProduced {
            continue;
        }
        let Some(id) = event.payload.get("artifact_id").and_then(|v| v.as_str()) else {
            continue;
        };
        ids.insert(id);
        if let Some(path) = event.payload.get("path").and_then(|v| v.as_str()) {
            by_path.entry(path).or_default().insert(id);
        }
    }

    if ids.contains(selector) {
        return Ok(ArtifactId(selector.to_string()));
    }

    match by_path.get(selector) {
        Some(matches) if matches.len() == 1 => {
            let id = matches.iter().next().expect("len == 1");
            Ok(ArtifactId((*id).to_string()))
        }
        Some(matches) => Err(CheckError::AmbiguousArtifact {
            requested: selector.to_string(),
            matches: matches.iter().map(|s| (*s).to_string()).collect(),
        }),
        None => Err(CheckError::NoSuchArtifact {
            requested: selector.to_string(),
            known: ids
                .iter()
                .take(MAX_SUGGESTIONS)
                .map(|s| (*s).to_string())
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    fn produced(id: &str, path: &str) -> IrEvent {
        IrEvent {
            event_id: uuid_from(id),
            producer: "test".to_string(),
            producer_version: "0.0.0".to_string(),
            session_id: "s".to_string(),
            computation_id: Some("comp:1".to_string()),
            parent_id: None,
            causal_inputs: None,
            ts: OffsetDateTime::UNIX_EPOCH,
            kind: EventKind::ArtifactProduced,
            payload: serde_json::json!({ "artifact_id": id, "path": path }),
        }
    }

    fn uuid_from(seed: &str) -> uuid::Uuid {
        let mut bytes = [0u8; 16];
        for (i, b) in seed.bytes().enumerate() {
            bytes[i % 16] ^= b;
        }
        uuid::Uuid::from_bytes(bytes)
    }

    #[test]
    fn an_id_resolves_to_itself() {
        let events = vec![produced("blake3:aa", "out.md")];
        assert_eq!(
            resolve(&events, "blake3:aa").expect("resolves"),
            ArtifactId("blake3:aa".to_string())
        );
    }

    #[test]
    fn a_path_resolves_to_its_artifact() {
        let events = vec![produced("blake3:aa", "briefs/acme.md")];
        assert_eq!(
            resolve(&events, "briefs/acme.md").expect("resolves"),
            ArtifactId("blake3:aa".to_string())
        );
    }

    #[test]
    fn a_path_naming_two_artifacts_is_an_error_not_a_guess() {
        let events = vec![
            produced("blake3:aa", "out.md"),
            produced("blake3:bb", "out.md"),
        ];
        let err = resolve(&events, "out.md").expect_err("ambiguous");
        assert!(matches!(err, CheckError::AmbiguousArtifact { .. }));
        assert_eq!(err.exit(), Exit::Usage);
    }

    #[test]
    fn an_unknown_selector_lists_what_exists() {
        let events = vec![produced("blake3:aa", "out.md")];
        let err = resolve(&events, "nope.md").expect_err("unknown");
        assert!(err.to_string().contains("blake3:aa"));
    }

    #[test]
    fn every_failure_exits_above_two() {
        // A check failure is never a verdict about the artifact.
        let errors = [
            CheckError::NoStore {
                path: PathBuf::from("/nowhere"),
            },
            CheckError::Engine(EngineError::NondeterministicReplay),
            CheckError::NoSuchArtifact {
                requested: "x".to_string(),
                known: Vec::new(),
            },
            CheckError::AmbiguousArtifact {
                requested: "x".to_string(),
                matches: Vec::new(),
            },
        ];
        for error in errors {
            assert!(
                error.exit().code() > 2,
                "{error:?} exits {}",
                error.exit().code()
            );
        }
    }

    #[test]
    fn a_structural_defect_is_a_tool_error_not_a_verdict() {
        // The engine refuses to emit on a malformed certificate.
        // Reporting that as 1 or 2 would let an engine bug pass for
        // evidence about the world.
        let error = CheckError::Engine(EngineError::InternalInconsistency {
            message: "self-audit failed".to_string(),
        });
        assert_eq!(error.exit(), Exit::EngineRefused);
        assert!(error.exit().code() > 2);
    }
}
