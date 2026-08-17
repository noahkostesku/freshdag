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
//! If the recorded write carried a content hash and the file's current
//! bytes do not match it, `mark` **refuses**. Something outside
//! FreshDAG's observation changed the file after the computation wrote
//! it, so that computation did not produce these bytes, and minting an
//! artifact that says otherwise would be exactly the invariant-#7
//! failure this tool exists to prevent. (`Edit`/`MultiEdit` writes carry
//! no hash — the hook payload has only a splice — so for those there is
//! nothing to compare and `mark` proceeds.)

use std::path::{Path, PathBuf};

use freshdag_core::artifact::ArtifactId;
use freshdag_core::ir::{
    CoverageManifest, EventKind, EventKindPattern, Hash, HashAlgo, IrEvent, ProducerRole,
};
use freshdag_store::{
    AppendOutcome, CoverageRegistry, ProducerKey, Store, StoreError, COVERAGE_FILE_NAME,
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
            | Self::NoComputation { .. } => Exit::Usage,
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

    let write = latest_write_of(&events, &target).ok_or_else(|| MarkError::NoRecordedWrite {
        path: target.clone(),
    })?;

    // A recorded hash is the only thing that can tell us the file still
    // holds the bytes that computation wrote. When there is one and it
    // disagrees, refuse.
    if let Some(recorded) = write.payload.get("hash").and_then(|v| v.as_str()) {
        if recorded != on_disk.to_string() {
            return Err(MarkError::ContentDrifted {
                path: target,
                recorded: recorded.to_string(),
                on_disk: on_disk.to_string(),
            });
        }
    }

    let computation = write
        .computation_id
        .clone()
        .ok_or_else(|| MarkError::NoComputation {
            path: target.clone(),
        })?;

    publish_manifest(store_root)?;

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
            "path": target.display().to_string(),
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

/// The most recent `fs.write` recorded for `path`, in log order.
fn latest_write_of<'e>(events: &'e [IrEvent], path: &Path) -> Option<&'e IrEvent> {
    events.iter().rev().find(|e| {
        e.kind == EventKind::FsWrite
            && e.payload
                .get("path")
                .and_then(|v| v.as_str())
                .is_some_and(|p| Path::new(p) == path)
    })
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
