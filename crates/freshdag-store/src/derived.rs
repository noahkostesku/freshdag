//! On-disk materialization of the derived graph.
//!
//! ```text
//! <root>/
//!   events.jsonl        canonical, append-only, THE source of truth
//!   coverage.jsonl      canonical, append-only producer registrations
//!   derived/            ← everything in here is disposable
//!     manifest.json
//!     computations.jsonl
//!     reverse-index.jsonl
//!     attribution.jsonl
//!     defects.jsonl
//! ```
//!
//! `derived/` may be deleted wholesale at any moment. Nothing reads from
//! it to answer a question the log could not answer; nothing writes to it
//! except a full replay. Invariant #5 and ADR 0005.
//!
//! # Why files at all, then?
//!
//! Because "reconstructable" has to be *demonstrated*, not asserted. The
//! bytes on disk are exactly the bytes a fresh replay produces, so the
//! reconstruction test can compare byte maps rather than trusting a
//! structural `PartialEq`. [`DerivedGraph::to_files`] is the single
//! serialization path, used both by [`DerivedGraph::write_to`] and by
//! the tests.
//!
//! # The manifest carries a digest, not a timestamp
//!
//! A `built_at` field would make two byte-identical replays differ, which
//! would defeat the only check that matters. Instead the manifest carries
//! [`DerivedManifest::source_digest`] — a BLAKE3 over the canonical
//! serialization of the linearized log plus the coverage projection. If
//! the digest matches, this derived directory corresponds to that log.

use std::collections::BTreeMap;
use std::path::Path;

use freshdag_core::ir::IrEvent;
use serde::{Deserialize, Serialize};

use crate::coverage::CoverageRegistry;
use crate::error::StoreError;
use crate::graph::{
    ComputationCoverage, ComputationNode, DerivedGraph, GraphDefect, ReverseIndexEntry,
};

/// Directory name for derived state inside a store root.
pub const DERIVED_DIR_NAME: &str = "derived";

/// Format identifier written into `derived/manifest.json`.
///
/// Bumping this is *not* a migration. Derived layouts change by deleting
/// `derived/` and replaying; that freedom is the whole point of
/// `ARCHITECTURE.md §9`.
pub const DERIVED_FORMAT: &str = "freshdag.derived/v0.1";

/// File names inside `derived/`, in write order.
pub const DERIVED_FILES: &[&str] = &[
    MANIFEST_FILE,
    COMPUTATIONS_FILE,
    REVERSE_INDEX_FILE,
    ATTRIBUTION_FILE,
    DEFECTS_FILE,
];

const MANIFEST_FILE: &str = "manifest.json";
const COMPUTATIONS_FILE: &str = "computations.jsonl";
const REVERSE_INDEX_FILE: &str = "reverse-index.jsonl";
const ATTRIBUTION_FILE: &str = "attribution.jsonl";
const DEFECTS_FILE: &str = "defects.jsonl";

/// `derived/manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedManifest {
    /// Format identifier ([`DERIVED_FORMAT`]).
    pub format: String,
    /// BLAKE3 over the canonical log serialization plus the coverage
    /// projection this graph was built from. See
    /// [`source_digest`].
    pub source_digest: String,
    /// Number of events replayed.
    pub event_count: usize,
    /// Number of computations materialized.
    pub computation_count: usize,
    /// Number of reverse-index entries.
    pub dependency_count: usize,
    /// Whether the replay was reproducible from any physical log order.
    /// False when a producer emitted a divergent duplicate `event_id`.
    pub deterministic: bool,
}

/// BLAKE3 digest binding a derived directory to the log it came from.
///
/// Computed over the canonical linearization's serialized bytes, then a
/// `\0` separator, then each registered manifest serialized in
/// `(producer, version)` order. Deterministic and offset-insensitive:
/// [`linearize`] compares timestamps as instants, and the event bytes are
/// whatever the producer wrote.
pub fn source_digest(events: &[IrEvent], coverage: &CoverageRegistry) -> String {
    let mut hasher = blake3::Hasher::new();
    for event in events {
        if let Ok(bytes) = serde_json::to_vec(event) {
            hasher.update(&bytes);
        }
        hasher.update(b"\n");
    }
    hasher.update(b"\0");
    for (key, manifest) in coverage.manifests() {
        hasher.update(key.to_string().as_bytes());
        if let Ok(bytes) = serde_json::to_vec(manifest) {
            hasher.update(&bytes);
        }
        hasher.update(b"\n");
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

impl DerivedGraph {
    /// The exact bytes this graph materializes to, keyed by file name.
    ///
    /// This is the single serialization path. Two graphs are equal on
    /// disk iff these maps are equal, which is what the reconstruction
    /// test asserts.
    pub fn to_files(
        &self,
        source_digest: &str,
    ) -> Result<BTreeMap<&'static str, Vec<u8>>, StoreError> {
        let manifest = DerivedManifest {
            format: DERIVED_FORMAT.to_string(),
            source_digest: source_digest.to_string(),
            event_count: self.event_count(),
            computation_count: self.computations().count(),
            dependency_count: self.reverse_index().count(),
            deterministic: self.is_deterministic(),
        };

        let mut files = BTreeMap::new();
        files.insert(
            MANIFEST_FILE,
            serde_json::to_vec(&manifest).map_err(StoreError::Serialize)?,
        );
        files.insert(COMPUTATIONS_FILE, jsonl(self.computations())?);
        files.insert(REVERSE_INDEX_FILE, jsonl(self.reverse_index())?);
        files.insert(ATTRIBUTION_FILE, jsonl(self.attributions())?);
        files.insert(DEFECTS_FILE, jsonl(self.defects().iter())?);
        Ok(files)
    }

    /// Write the derived directory, replacing whatever was there.
    ///
    /// The directory is removed first: a partial overwrite could leave a
    /// stale file that no replay would ever produce, and a derived
    /// directory that does not correspond to any log is worse than none.
    pub fn write_to(&self, dir: &Path, source_digest: &str) -> Result<(), StoreError> {
        drop_derived(dir)?;
        std::fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
        for (name, bytes) in self.to_files(source_digest)? {
            let path = dir.join(name);
            std::fs::write(&path, &bytes).map_err(|e| StoreError::io(&path, e))?;
        }
        Ok(())
    }

    /// Read a previously written derived directory.
    ///
    /// Returns `Ok(None)` when the directory does not exist. A directory
    /// that exists but is damaged is an error, never a silent empty
    /// graph — the caller must be able to tell "no derived state" from
    /// "derived state I could not read."
    pub fn read_from(dir: &Path) -> Result<Option<LoadedDerived>, StoreError> {
        let manifest_path = dir.join(MANIFEST_FILE);
        let raw = match std::fs::read(&manifest_path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(StoreError::io(&manifest_path, e)),
        };
        let manifest: DerivedManifest =
            serde_json::from_slice(&raw).map_err(|e| StoreError::MalformedRecord {
                path: manifest_path.clone(),
                line: 1,
                byte_offset: 0,
                message: e.to_string(),
            })?;

        let computations: Vec<ComputationNode> = read_jsonl(&dir.join(COMPUTATIONS_FILE))?;
        let reverse_index: Vec<ReverseIndexEntry> = read_jsonl(&dir.join(REVERSE_INDEX_FILE))?;
        let attribution: Vec<ComputationCoverage> = read_jsonl(&dir.join(ATTRIBUTION_FILE))?;
        let defects: Vec<GraphDefect> = read_jsonl(&dir.join(DEFECTS_FILE))?;

        Ok(Some(LoadedDerived {
            manifest,
            computations,
            reverse_index,
            attribution,
            defects,
        }))
    }
}

/// A derived directory read back from disk.
///
/// Deliberately *not* a [`DerivedGraph`]. A loaded directory is a cache
/// whose correspondence to the log is only as good as its
/// [`DerivedManifest::source_digest`]; handing back a `DerivedGraph`
/// would invite callers to treat it as authoritative. Check
/// [`Self::matches`] first, and if it fails, replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedDerived {
    /// The manifest that was on disk.
    pub manifest: DerivedManifest,
    /// Computation nodes, in the order they were written.
    pub computations: Vec<ComputationNode>,
    /// Reverse `depends_on` index entries (the blast-radius view).
    pub reverse_index: Vec<ReverseIndexEntry>,
    /// Per-computation producer attribution.
    pub attribution: Vec<ComputationCoverage>,
    /// Structural defects recorded at build time.
    pub defects: Vec<GraphDefect>,
}

impl LoadedDerived {
    /// Does this derived directory correspond to the given log digest?
    pub fn matches(&self, source_digest: &str) -> bool {
        self.manifest.format == DERIVED_FORMAT && self.manifest.source_digest == source_digest
    }
}

/// Delete a derived directory. A missing directory is success.
pub fn drop_derived(dir: &Path) -> Result<(), StoreError> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StoreError::io(dir, e)),
    }
}

fn jsonl<'a, T: Serialize + 'a>(items: impl Iterator<Item = &'a T>) -> Result<Vec<u8>, StoreError> {
    let mut out = Vec::new();
    for item in items {
        out.extend_from_slice(&serde_json::to_vec(item).map_err(StoreError::Serialize)?);
        out.push(b'\n');
    }
    Ok(out)
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>, StoreError> {
    // A missing file here is an error, not an empty vector: the manifest
    // exists, so the directory claims to be complete.
    let raw = std::fs::read(path).map_err(|e| StoreError::io(path, e))?;
    let mut out = Vec::new();
    for (i, line) in raw.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        out.push(
            serde_json::from_slice(line).map_err(|e| StoreError::MalformedRecord {
                path: path.to_path_buf(),
                line: i as u64 + 1,
                byte_offset: 0,
                message: e.to_string(),
            })?,
        );
    }
    Ok(out)
}
