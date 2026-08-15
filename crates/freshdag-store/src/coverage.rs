//! Producer coverage-manifest registration.
//!
//! Producers register their [`CoverageManifest`] with the store; the
//! engine later asks which manifests contributed to a computation or
//! session so it can populate `Certificate.observation_coverage`
//! (see `docs/contracts/certificate-contract.md`).
//!
//! The registry file is itself append-only: a re-registration is a new
//! record, never an edit. The in-memory map is a *projection* (last
//! registration per `(producer, version)` wins) and is therefore
//! disposable and reconstructable from the file, per invariant #5.
//!
//! This module deliberately stops at "which manifests are in play". How
//! the engine consumes them is W4's problem.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use freshdag_core::certificate::CoverageEntry;
use freshdag_core::ir::{CoverageManifest, IrEvent};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::StoreError;

/// Identity of a registered producer: name plus version, exactly the pair
/// an [`IrEvent`] carries in `producer` / `producer_version`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProducerKey {
    /// Producer identity (matches `IrEvent::producer`).
    pub producer: String,
    /// Producer semver (matches `IrEvent::producer_version`).
    pub version: String,
}

impl ProducerKey {
    /// Build a key from its parts.
    pub fn new(producer: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            producer: producer.into(),
            version: version.into(),
        }
    }

    /// The key an event's producer fields imply.
    pub fn of_event(event: &IrEvent) -> Self {
        Self::new(event.producer.clone(), event.producer_version.clone())
    }

    /// The key a manifest declares.
    pub fn of_manifest(manifest: &CoverageManifest) -> Self {
        Self::new(manifest.producer.clone(), manifest.version.clone())
    }
}

impl std::fmt::Display for ProducerKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.producer, self.version)
    }
}

/// One append-only registration record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageRegistration {
    /// When the manifest was registered.
    #[serde(with = "time::serde::rfc3339")]
    pub registered_at: OffsetDateTime,
    /// The manifest as published by the producer.
    pub manifest: CoverageManifest,
}

/// Manifests in play for some scope, and the producers that emitted
/// events without one.
///
/// `unregistered` is not a footnote. `Certificate::check_invariants`
/// rejects a certificate whose event stream names a producer absent from
/// `observation_coverage`, and per invariant #7 a producer whose coverage
/// we cannot read is a producer whose silence we cannot interpret.
#[derive(Debug, Clone, Default)]
pub struct CoverageLookup {
    /// Coverage entries for producers that both emitted in scope and are
    /// registered, in `(producer, version)` order.
    pub entries: Vec<CoverageEntry>,
    /// Producers that emitted events in scope but have no registered
    /// manifest.
    pub unregistered: Vec<ProducerKey>,
}

impl CoverageLookup {
    /// Is every producer that emitted in scope accounted for?
    pub fn is_complete(&self) -> bool {
        self.unregistered.is_empty()
    }
}

/// Append-only registry of producer coverage manifests.
#[derive(Debug, Clone, Default)]
pub struct CoverageRegistry {
    path: Option<PathBuf>,
    manifests: BTreeMap<ProducerKey, CoverageManifest>,
}

impl CoverageRegistry {
    /// An in-memory registry with no backing file.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Open (creating if necessary) a registry backed by `path`, replaying
    /// every registration record already present.
    ///
    /// A missing file is an empty registry, not an error.
    ///
    /// # Errors
    ///
    /// - [`StoreError::Io`] if the file exists but cannot be read.
    /// - [`StoreError::MalformedRecord`] if a registration record does not
    ///   parse. Registrations are never skipped quietly.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let mut registry = Self {
            path: Some(path.clone()),
            manifests: BTreeMap::new(),
        };
        let raw = match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(registry),
            Err(e) => return Err(StoreError::io(&path, e)),
        };
        for (i, line) in raw.split(|b| *b == b'\n').enumerate() {
            if line.is_empty() {
                continue;
            }
            let record: CoverageRegistration =
                serde_json::from_slice(line).map_err(|e| StoreError::MalformedRecord {
                    path: path.clone(),
                    line: i as u64 + 1,
                    byte_offset: 0,
                    message: e.to_string(),
                })?;
            // Last registration wins; the record itself is never rewritten.
            registry
                .manifests
                .insert(ProducerKey::of_manifest(&record.manifest), record.manifest);
        }
        Ok(registry)
    }

    /// Path backing this registry, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Register a producer's manifest, appending a record to the backing
    /// file if there is one.
    ///
    /// # Errors
    ///
    /// - [`StoreError::Serialize`] if the record does not serialize.
    /// - [`StoreError::Io`] if the append fails.
    pub fn register(&mut self, manifest: CoverageManifest) -> Result<(), StoreError> {
        self.register_at(manifest, OffsetDateTime::now_utc())
    }

    /// [`Self::register`] with an explicit registration timestamp. Tests
    /// and replays use this to stay deterministic.
    ///
    /// # Errors
    ///
    /// As [`Self::register`].
    pub fn register_at(
        &mut self,
        manifest: CoverageManifest,
        registered_at: OffsetDateTime,
    ) -> Result<(), StoreError> {
        if let Some(path) = &self.path {
            let record = CoverageRegistration {
                registered_at,
                manifest: manifest.clone(),
            };
            let mut buf = serde_json::to_vec(&record).map_err(StoreError::Serialize)?;
            buf.push(b'\n');
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|e| StoreError::io(parent, e))?;
                }
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| StoreError::io(path, e))?;
            file.write_all(&buf).map_err(|e| StoreError::io(path, e))?;
            file.sync_data().map_err(|e| StoreError::io(path, e))?;
        }
        self.manifests
            .insert(ProducerKey::of_manifest(&manifest), manifest);
        Ok(())
    }

    /// Number of registered manifests.
    pub fn len(&self) -> usize {
        self.manifests.len()
    }

    /// Is the registry empty?
    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }

    /// The manifest registered for an exact `(producer, version)` pair.
    pub fn manifest(&self, key: &ProducerKey) -> Option<&CoverageManifest> {
        self.manifests.get(key)
    }

    /// Every registered manifest, in `(producer, version)` order.
    pub fn manifests(&self) -> impl Iterator<Item = (&ProducerKey, &CoverageManifest)> {
        self.manifests.iter()
    }

    /// Coverage for every producer appearing in `events`.
    pub fn coverage_for<'e>(
        &self,
        events: impl IntoIterator<Item = &'e IrEvent>,
    ) -> CoverageLookup {
        self.lookup(events.into_iter().map(ProducerKey::of_event))
    }

    /// Coverage for the producers that emitted into one session.
    pub fn coverage_for_session<'e>(
        &self,
        events: impl IntoIterator<Item = &'e IrEvent>,
        session_id: &str,
    ) -> CoverageLookup {
        self.lookup(
            events
                .into_iter()
                .filter(|e| e.session_id == session_id)
                .map(ProducerKey::of_event),
        )
    }

    /// Coverage for the producers that emitted into one computation.
    ///
    /// Events with no `computation_id` (infrastructural events, per
    /// execution-ir.md §Event Envelope) do not belong to any computation
    /// and are excluded.
    pub fn coverage_for_computation<'e>(
        &self,
        events: impl IntoIterator<Item = &'e IrEvent>,
        computation_id: &str,
    ) -> CoverageLookup {
        self.lookup(
            events
                .into_iter()
                .filter(|e| e.computation_id.as_deref() == Some(computation_id))
                .map(ProducerKey::of_event),
        )
    }

    fn lookup(&self, keys: impl Iterator<Item = ProducerKey>) -> CoverageLookup {
        let observed: BTreeSet<ProducerKey> = keys.collect();
        let mut lookup = CoverageLookup::default();
        for key in observed {
            match self.manifests.get(&key) {
                Some(manifest) => lookup.entries.push(CoverageEntry::from(manifest)),
                None => lookup.unregistered.push(key),
            }
        }
        lookup
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{event, manifest, ts};

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn registration_survives_a_reopen() {
        let dir = tmp();
        let path = dir.path().join("coverage.jsonl");
        {
            let mut reg = CoverageRegistry::open(&path).expect("open");
            reg.register(manifest("observer-fsatrace", "0.1.0", &["fs.*"]))
                .expect("register");
            reg.register(manifest("adapter-claude", "0.1.0", &["tool.*"]))
                .expect("register");
        }
        let reg = CoverageRegistry::open(&path).expect("reopen");
        assert_eq!(reg.len(), 2);
        assert!(reg
            .manifest(&ProducerKey::new("observer-fsatrace", "0.1.0"))
            .is_some());
    }

    #[test]
    fn re_registration_appends_and_last_wins() {
        let dir = tmp();
        let path = dir.path().join("coverage.jsonl");
        let mut reg = CoverageRegistry::open(&path).expect("open");
        reg.register(manifest("observer-x", "0.1.0", &["fs.read"]))
            .expect("register");
        reg.register(manifest("observer-x", "0.1.0", &["fs.read", "fs.write"]))
            .expect("re-register");

        let raw = std::fs::read_to_string(&path).expect("read");
        assert_eq!(
            raw.lines().count(),
            2,
            "history is append-only: the first registration is still on disk"
        );

        let reopened = CoverageRegistry::open(&path).expect("reopen");
        let m = reopened
            .manifest(&ProducerKey::new("observer-x", "0.1.0"))
            .expect("manifest");
        assert_eq!(m.emits.len(), 2, "projection takes the last registration");
    }

    #[test]
    fn versions_are_distinct_producers() {
        let mut reg = CoverageRegistry::in_memory();
        reg.register(manifest("observer-x", "0.1.0", &["fs.read"]))
            .expect("register");
        reg.register(manifest("observer-x", "0.2.0", &["fs.*"]))
            .expect("register");
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn scoped_lookup_reports_unregistered_producers() {
        let mut reg = CoverageRegistry::in_memory();
        reg.register(manifest("adapter-claude", "0.1.0", &["tool.*"]))
            .expect("register");

        let mut adapter = event(
            "adapter-claude",
            "s1",
            ts(10),
            "00000000-0000-7000-8000-000000000001",
        );
        adapter.computation_id = Some("comp-1".into());
        let mut rogue = event(
            "observer-ghost",
            "s1",
            ts(20),
            "00000000-0000-7000-8000-000000000002",
        );
        rogue.computation_id = Some("comp-1".into());
        let mut other_session = event(
            "observer-ghost",
            "s2",
            ts(30),
            "00000000-0000-7000-8000-000000000003",
        );
        other_session.computation_id = Some("comp-2".into());
        let events = vec![adapter, rogue, other_session];

        let all = reg.coverage_for(&events);
        assert_eq!(all.entries.len(), 1);
        assert_eq!(all.unregistered.len(), 1);
        assert!(!all.is_complete());

        let comp = reg.coverage_for_computation(&events, "comp-1");
        assert_eq!(comp.entries.len(), 1);
        assert_eq!(comp.entries[0].producer, "adapter-claude");
        assert_eq!(
            comp.unregistered,
            vec![ProducerKey::new("observer-ghost", "0.1.0")]
        );

        let session = reg.coverage_for_session(&events, "s2");
        assert!(session.entries.is_empty());
        assert_eq!(session.unregistered.len(), 1);
    }

    #[test]
    fn events_without_a_computation_id_are_out_of_computation_scope() {
        let mut reg = CoverageRegistry::in_memory();
        reg.register(manifest("adapter-claude", "0.1.0", &["tool.*"]))
            .expect("register");
        let infra = event(
            "adapter-claude",
            "s1",
            ts(10),
            "00000000-0000-7000-8000-000000000001",
        );
        assert!(infra.computation_id.is_none());
        let lookup = reg.coverage_for_computation(&[infra], "comp-1");
        assert!(lookup.entries.is_empty());
        assert!(lookup.is_complete());
    }

    #[test]
    fn coverage_entry_carries_emits_and_limitations() {
        let mut reg = CoverageRegistry::in_memory();
        let mut m = manifest("observer-fsatrace", "0.1.0", &["fs.*"]);
        m.known_limitations = vec!["mmap reads are invisible".into()];
        reg.register(m).expect("register");

        let e = event(
            "observer-fsatrace",
            "s1",
            ts(10),
            "00000000-0000-7000-8000-000000000001",
        );
        let lookup = reg.coverage_for(&[e]);
        assert_eq!(lookup.entries.len(), 1);
        assert!(lookup.entries[0].covers(freshdag_core::ir::EventKind::FsWrite));
        assert_eq!(lookup.entries[0].known_limitations.len(), 1);
    }

    #[test]
    fn malformed_registration_is_an_error_not_a_skip() {
        let dir = tmp();
        let path = dir.path().join("coverage.jsonl");
        std::fs::write(&path, b"{\"registered_at\":\"nope\"}\n").expect("write");
        assert!(matches!(
            CoverageRegistry::open(&path),
            Err(StoreError::MalformedRecord { line: 1, .. })
        ));
    }
}
