//! `freshdag mark <path>` — declare that a file is an artifact.
//!
//! # Why this exists at all
//!
//! Nothing else emits `artifact.produced`, and without it a store has no
//! artifact to check: `freshdag check` reports "the store records no
//! artifacts at all" over a log full of perfectly good observations.
//!
//! The adapter cannot close this gap on its own. It sees every file the
//! agent writes — scratch files, logs, temporary output — and **which of
//! them is "the artifact" is a user declaration, not an observation.**
//! An adapter that promoted every write would be asserting something
//! nobody told it.
//!
//! # What `mark` does and does not assert
//!
//! `mark` does not claim authorship. It *finds* the producing
//! computation in the log — the most recent recorded `fs.write` of this
//! path — and refuses if there is none. The artifact's identity is the
//! bytes on disk now; the computation is whichever one the store
//! records writing them.
//!
//! `mark` refuses unless the recorded write carries a content hash AND
//! the file's current bytes match it.
//!
//! A mismatch means something outside FreshDAG's observation changed the
//! file after the computation wrote it, so that computation did not
//! produce these bytes.
//!
//! **An absent hash is refused too**, and an earlier revision got this
//! wrong: it let the check fall through, reasoning that
//! `Edit`/`MultiEdit`/`NotebookEdit` carry only a splice so there was
//! nothing to compare. But these `fs.write` events are synthesized at
//! PreToolUse, so even a *denied* edit records one — and the
//! fall-through then attributed the artifact to whichever computation
//! last touched the path, on no evidence, erasing the real producer's
//! dependencies from the certificate. Minting an artifact on an
//! unverifiable write is the invariant-#9 failure this tool exists to
//! prevent, so it does not.

use std::path::{Path, PathBuf};

use freshdag_core::artifact::ArtifactId;
use freshdag_core::ir::{
    CoverageManifest, EventKind, EventKindPattern, Hash, HashAlgo, IrEvent, ProducerRole,
};
use freshdag_store::{
    linearize, AppendOutcome, CoverageRegistry, ProducerKey, Store, StoreError, COVERAGE_FILE_NAME,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::exit::Exit;

/// Producer identity for events this CLI mints.
pub const CLI_PRODUCER: &str = "freshdag-cli";
/// Producer version for events this CLI mints.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything that stops `mark` from recording an artifact.
#[derive(Debug)]
pub enum MarkError {
    /// The path given is not a FreshDAG store.
    NoStore { path: PathBuf },
    /// The store could not be read or written.
    Store(StoreError),
    /// The file to mark could not be read.
    Unreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Nothing in the log records writing this path.
    NoRecordedWrite { path: PathBuf },
    /// The file's current bytes differ from the bytes the recorded
    /// computation wrote.
    ContentDrifted {
        path: PathBuf,
        recorded: String,
        on_disk: String,
    },
    /// The recorded write has no computation to attribute the artifact
    /// to.
    NoComputation { path: PathBuf },
    /// The store accepted the call but dropped the event under its byte
    /// budget.
    Dropped { total_dropped: u64 },
    /// The most recent recorded write of this path carries no content
    /// hash, so there is no way to tell whether the bytes on disk are
    /// the ones that computation wrote.
    UnverifiableWrite { path: PathBuf, tool_hint: String },
}

impl std::fmt::Display for MarkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoStore { path } => {
                write!(f, "no FreshDAG store at {}", path.display())
            }
            Self::Store(err) => write!(f, "{err}"),
            Self::Unreadable { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            Self::NoRecordedWrite { path } => write!(
                f,
                "nothing in this store records writing {}. `mark` attributes an \
                 artifact to the computation that produced it, and will not invent \
                 one",
                path.display()
            ),
            Self::ContentDrifted {
                path,
                recorded,
                on_disk,
            } => write!(
                f,
                "{} has changed since the computation that wrote it: recorded={recorded} \
                 on-disk={on_disk}. That computation did not produce these bytes, so \
                 attributing them to it would be false",
                path.display()
            ),
            Self::NoComputation { path } => write!(
                f,
                "the recorded write of {} carries no computation_id, so there is \
                 nothing to attribute the artifact to",
                path.display()
            ),
            Self::Dropped { total_dropped } => write!(
                f,
                "the store dropped the artifact.produced event under its byte budget \
                 ({total_dropped} dropped so far), so nothing was recorded"
            ),
            Self::UnverifiableWrite { path, tool_hint } => write!(
                f,
                "the most recent recorded write of {} ({tool_hint}) carries no content \
                 hash, so nothing can show the bytes on disk are the ones that \
                 computation wrote. Attributing them to it would be a guess",
                path.display()
            ),
        }
    }
}

impl MarkError {
    /// Every one of these is a tool failure, never a validity verdict:
    /// refusing to record an artifact says nothing about whether any
    /// artifact is fresh.
    pub fn exit(&self) -> Exit {
        match self {
            Self::NoStore { .. } | Self::Store(_) | Self::Dropped { .. } => Exit::StoreError,
            Self::Unreadable { .. }
            | Self::NoRecordedWrite { .. }
            | Self::ContentDrifted { .. }
            | Self::NoComputation { .. }
            | Self::UnverifiableWrite { .. } => Exit::Usage,
        }
    }
}

/// What `mark` recorded.
pub struct Marked {
    /// The minted artifact identity.
    pub artifact: ArtifactId,
    /// The computation it was attributed to.
    pub computation: String,
    /// Absolute path recorded for it.
    pub path: PathBuf,
}

/// Record `path` as an artifact in the store at `store_root`.
///
/// # Errors
///
/// See [`MarkError`]. All of them are tool failures; none is a verdict
/// about freshness.
pub fn run(store_root: &Path, path: &str, kind: Option<&str>) -> Result<Marked, MarkError> {
    if !store_root.is_dir() {
        return Err(MarkError::NoStore {
            path: store_root.to_path_buf(),
        });
    }

    let target = std::fs::canonicalize(path).map_err(|source| MarkError::Unreadable {
        path: PathBuf::from(path),
        source,
    })?;

    let bytes = std::fs::read(&target).map_err(|source| MarkError::Unreadable {
        path: target.clone(),
        source,
    })?;
    let on_disk = blake3_hash(&bytes);

    let mut store = Store::open(store_root).map_err(MarkError::Store)?;
    let events = store.read_log().map_err(MarkError::Store)?;

    let write = latest_write_of(events, &target).ok_or_else(|| MarkError::NoRecordedWrite {
        path: target.clone(),
    })?;

    // A recorded hash is the only thing that can tell us the file still
    // holds the bytes that computation wrote.
    //
    // When there is none, REFUSE. This previously fell through — `if let
    // Some(recorded)` simply skipped the check — on the reasoning that
    // `Edit`/`MultiEdit`/`NotebookEdit` payloads carry only a splice, so
    // there was nothing to compare and refusing would make `mark`
    // useless on edited files. That was wrong, and a verifier
    // reproduced it: the fall-through attributes the artifact to
    // whichever computation last *touched* the path, on no evidence at
    // all. Because these `fs.write` events are synthesized at
    // PreToolUse, even a **denied** edit records one — so an unrelated
    // session that tried and failed to edit a file could claim
    // authorship of another session's bytes, and the real producer's
    // dependencies would vanish from the certificate. That is invariant
    // #9 ("every artifact is traceable to the computation that produced
    // it") broken outright.
    //
    // Refusing costs the ability to mark an edited file until the
    // adapter can fingerprint an edit's result. That is the correct
    // trade: an unmarkable artifact is an inconvenience, a
    // misattributed one is a false provenance claim.
    let Some(recorded) = write.payload.get("hash").and_then(|v| v.as_str()) else {
        return Err(MarkError::UnverifiableWrite {
            path: target,
            tool_hint: "no `hash` on the recorded fs.write — Edit/MultiEdit/NotebookEdit \
                        payloads carry only a splice, and a denied tool call still records \
                        the write"
                .to_string(),
        });
    };
    if recorded != on_disk.to_string() {
        return Err(MarkError::ContentDrifted {
            path: target,
            recorded: recorded.to_string(),
            on_disk: on_disk.to_string(),
        });
    }

    let computation = write
        .computation_id
        .clone()
        .ok_or_else(|| MarkError::NoComputation {
            path: target.clone(),
        })?;

    publish_manifest(store_root)?;

    // Record the path in the form the PRODUCING WRITE used, not the
    // realpath `fs::canonicalize` gave us. Every other path in the IR is
    // in producer form — this adapter canonicalizes lexically and does
    // not resolve symlinks — so minting the artifact under a realpath
    // introduced a second convention, and `freshdag check <path>`, which
    // matches exactly, then could not find it. On macOS that is every
    // path under `mktemp -d` (`/var` -> `/private/var`), which broke
    // `scripts/demo.sh` on the project's own development platform.
    let recorded_path = write
        .payload
        .get("path")
        .and_then(|v| v.as_str())
        .map_or_else(|| target.display().to_string(), ToString::to_string);

    let artifact = ArtifactId::from_hash(&on_disk);
    let event = IrEvent {
        event_id: Uuid::now_v7(),
        producer: CLI_PRODUCER.to_string(),
        producer_version: CLI_VERSION.to_string(),
        session_id: write.session_id.clone(),
        computation_id: Some(computation.clone()),
        parent_id: None,
        // The write this artifact's bytes came from is its causal input.
        causal_inputs: Some(vec![write.event_id]),
        ts: OffsetDateTime::now_utc(),
        kind: EventKind::ArtifactProduced,
        payload: serde_json::json!({
            "artifact_id": artifact.0,
            "path": recorded_path,
            "content_hash": on_disk.to_string(),
            "kind": kind.map_or_else(|| media_type_of(&target).to_string(), ToString::to_string),
            "size": bytes.len() as u64,
            "produced_by": computation,
        }),
    };

    // An append can be *dropped* under the sink's byte budget. Reporting
    // a mark that recorded nothing would be precisely the silent success
    // this tool exists to refuse.
    match store.append(&event).map_err(MarkError::Store)? {
        AppendOutcome::Appended => {}
        AppendOutcome::DroppedNewest { total_dropped } => {
            return Err(MarkError::Dropped { total_dropped })
        }
    }
    store.sync().map_err(MarkError::Store)?;

    Ok(Marked {
        artifact,
        computation,
        path: target,
    })
}

/// The most recent `fs.write` recorded for `path`, in **canonical**
/// order.
///
/// Two things here were wrong and are load-bearing.
///
/// **Order.** This used physical log order (`events.iter().rev()`).
/// `Store::read_log` returns events in the order they landed on disk,
/// and the store explicitly supports arbitrary physical order — the
/// graph and engine both sort by `order::linearize` first. With a
/// batching producer or two processes appending concurrently, "last in
/// the file" is not "last in time", so `mark` could attribute an
/// artifact to an *earlier* write. It now linearizes first.
///
/// **Path form.** The comparison was `Path == Path` between a
/// realpath (`fs::canonicalize`, used on the target) and whatever the
/// producer recorded — and this adapter canonicalizes *lexically*,
/// without resolving symlinks. On macOS every `/var/...` path is a
/// symlink to `/private/var/...`, so the two never matched and `mark`
/// refused every artifact under a `mktemp -d` workdir, including in
/// `scripts/demo.sh`. Both sides are now put through the same
/// resolution before comparing, falling back to the raw form for paths
/// that no longer exist.
fn latest_write_of(events: Vec<IrEvent>, path: &Path) -> Option<IrEvent> {
    let ordered = linearize(events.into_iter());
    ordered.into_iter().rev().find(|e| {
        e.kind == EventKind::FsWrite
            && e.payload
                .get("path")
                .and_then(|v| v.as_str())
                .is_some_and(|p| same_file(Path::new(p), path))
    })
}

/// Do these two paths name the same file, allowing for one side being
/// lexical and the other a realpath?
fn same_file(recorded: &Path, target: &Path) -> bool {
    if recorded == target {
        return true;
    }
    match (
        std::fs::canonicalize(recorded),
        std::fs::canonicalize(target),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// This CLI's coverage manifest, published once per `(producer,
/// version)` so its `artifact.produced` events do not read as an
/// unregistered producer.
///
/// Role is [`ProducerRole::Adapter`] deliberately. `mark` observes
/// nothing — it translates a user declaration into IR — and only
/// `Observer` discharges the subprocess observation obligation. An
/// `Adapter` declaring `artifact.produced` discharges nothing, which is
/// the correct and safe reading.
#[must_use]
pub fn coverage_manifest() -> CoverageManifest {
    CoverageManifest {
        role: ProducerRole::Adapter,
        producer: CLI_PRODUCER.to_string(),
        version: CLI_VERSION.to_string(),
        platforms: vec!["any".to_string()],
        emits: vec![EventKindPattern::new("artifact.produced")],
        // Not partial: `mark` emits exactly the declaration the user
        // made, and nothing else is in its scope to miss.
        partial: std::collections::BTreeMap::new(),
        capabilities: [(
            "attributes_to".to_string(),
            serde_json::json!("the most recent recorded fs.write of the marked path"),
        )]
        .into_iter()
        .collect(),
        known_limitations: vec![
            "records an artifact only when a user runs `freshdag mark`; it observes \
             nothing on its own"
                .to_string(),
            "attributes the artifact to the last computation the store records writing \
             the path, and refuses when the file's bytes no longer match that write"
                .to_string(),
        ],
    }
}

fn publish_manifest(store_root: &Path) -> Result<(), MarkError> {
    let manifest = coverage_manifest();
    let path = store_root.join(COVERAGE_FILE_NAME);
    let mut registry = CoverageRegistry::open(&path).map_err(MarkError::Store)?;
    if registry
        .manifest(&ProducerKey::of_manifest(&manifest))
        .is_some()
    {
        return Ok(());
    }
    registry.register(manifest).map_err(MarkError::Store)
}

/// A media type for the artifact, guessed from the file extension.
///
/// `execution-ir.md §Event Payloads` requires `kind` on
/// `artifact.produced` and the engine refuses a certificate without it.
/// It is **advisory metadata** — no validity decision reads it — so a
/// guess is acceptable where a guess about a dependency would not be.
/// `--kind` overrides it, and anything unrecognized falls to
/// `application/octet-stream` rather than to a more specific-sounding
/// lie.
fn media_type_of(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("md" | "markdown") => "text/markdown",
        Some("txt" | "text") => "text/plain",
        Some("json") => "application/json",
        Some("jsonl" | "ndjson") => "application/x-ndjson",
        Some("csv") => "text/csv",
        Some("html" | "htm") => "text/html",
        Some("toml") => "application/toml",
        Some("yaml" | "yml") => "application/yaml",
        Some("rs") => "text/x-rust",
        Some("py") => "text/x-python",
        Some("js" | "mjs") => "text/javascript",
        Some("ts") => "text/x-typescript",
        Some("sql") => "application/sql",
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn blake3_hash(bytes: &[u8]) -> Hash {
    let digest = blake3::hash(bytes).to_hex().to_string();
    Hash::new(HashAlgo::Blake3, digest).expect("blake3 hex is 64 lowercase hex chars")
}
