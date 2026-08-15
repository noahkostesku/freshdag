//! The on-disk store: a directory holding the canonical log and the
//! producer coverage registry.
//!
//! ```text
//! <root>/
//!   events.jsonl     canonical append-only observation log
//!   coverage.jsonl   append-only producer coverage-manifest registrations
//! ```
//!
//! Both files are append-only (invariant #4). Nothing else lives here
//! yet; derived-state layouts land in a follow-up and, per invariant #5,
//! will be droppable and rebuildable from these two files alone.

use std::path::{Path, PathBuf};

use freshdag_core::ir::{CoverageManifest, IrEvent};

use crate::coverage::CoverageRegistry;
use crate::error::StoreError;
use crate::reader::{read_log, scan_log, LogScan};
use crate::sink::{AppendOutcome, JsonlSink, SinkOptions};

/// File name of the canonical observation log inside a store root.
pub const LOG_FILE_NAME: &str = "events.jsonl";
/// File name of the producer coverage registry inside a store root.
pub const COVERAGE_FILE_NAME: &str = "coverage.jsonl";

/// An open FreshDAG store.
#[derive(Debug)]
pub struct Store {
    root: PathBuf,
    sink: JsonlSink,
    coverage: CoverageRegistry,
}

impl Store {
    /// Open (creating if necessary) a store rooted at `root`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`] if the directory or files cannot be opened, or
    /// [`StoreError::MalformedRecord`] if the coverage registry is damaged.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::with_options(root, SinkOptions::default())
    }

    /// Open a store with explicit sink options.
    ///
    /// # Errors
    ///
    /// As [`Self::open`].
    pub fn with_options(root: impl AsRef<Path>, options: SinkOptions) -> Result<Self, StoreError> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root).map_err(|e| StoreError::io(&root, e))?;
        let sink = JsonlSink::with_options(root.join(LOG_FILE_NAME), options)?;
        let coverage = CoverageRegistry::open(root.join(COVERAGE_FILE_NAME))?;
        Ok(Self {
            root,
            sink,
            coverage,
        })
    }

    /// Store root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of the canonical observation log.
    pub fn log_path(&self) -> PathBuf {
        self.root.join(LOG_FILE_NAME)
    }

    /// Path of the coverage registry.
    pub fn coverage_path(&self) -> PathBuf {
        self.root.join(COVERAGE_FILE_NAME)
    }

    /// Append one event to the canonical log.
    ///
    /// # Errors
    ///
    /// As [`JsonlSink::append`].
    pub fn append(&mut self, event: &IrEvent) -> Result<AppendOutcome, StoreError> {
        self.sink.append(event)
    }

    /// Append many events to the canonical log.
    ///
    /// # Errors
    ///
    /// As [`JsonlSink::append`].
    pub fn append_all<'e>(
        &mut self,
        events: impl IntoIterator<Item = &'e IrEvent>,
    ) -> Result<Vec<AppendOutcome>, StoreError> {
        self.sink.append_all(events)
    }

    /// Flush the log to stable storage.
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`] if the flush fails.
    pub fn sync(&mut self) -> Result<(), StoreError> {
        self.sink.sync()
    }

    /// Register a producer's coverage manifest.
    ///
    /// # Errors
    ///
    /// As [`CoverageRegistry::register`].
    pub fn register_producer(&mut self, manifest: CoverageManifest) -> Result<(), StoreError> {
        self.coverage.register(manifest)
    }

    /// The coverage registry.
    pub fn coverage(&self) -> &CoverageRegistry {
        &self.coverage
    }

    /// Read the canonical log strictly, in physical (append) order.
    ///
    /// # Errors
    ///
    /// As [`read_log`].
    pub fn read_log(&self) -> Result<Vec<IrEvent>, StoreError> {
        read_log(self.log_path())
    }

    /// Read the canonical log tolerantly, reporting defects.
    ///
    /// # Errors
    ///
    /// As [`scan_log`].
    pub fn scan_log(&self) -> Result<LogScan, StoreError> {
        scan_log(self.log_path())
    }

    /// Number of events dropped by this store's sink under back-pressure.
    pub fn dropped_count(&self) -> u64 {
        self.sink.dropped_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{event, manifest, ts};
    use crate::{linearize, ProducerStreams};

    #[test]
    fn store_round_trips_log_and_coverage() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Physical append order is deliberately not canonical order.
        // Within `adapter-a`, `event_id`s still ascend — a producer that
        // emitted out of `event_id` order would be a contract violation
        // and is asserted against below.
        let events = vec![
            event(
                "adapter-a",
                "s1",
                ts(30),
                "00000000-0000-7000-8000-000000000001",
            ),
            event(
                "observer-b",
                "s1",
                ts(10),
                "00000000-0000-7000-8000-000000000002",
            ),
            event(
                "adapter-a",
                "s1",
                ts(20),
                "00000000-0000-7000-8000-000000000003",
            ),
        ];
        {
            let mut store = Store::open(dir.path()).expect("open");
            store
                .register_producer(manifest("adapter-a", "0.1.0", &["tool.*"]))
                .expect("register");
            store
                .register_producer(manifest("observer-b", "0.1.0", &["fs.*"]))
                .expect("register");
            store.append_all(&events).expect("append");
            store.sync().expect("sync");
        }

        let store = Store::open(dir.path()).expect("reopen");
        let read = store.read_log().expect("read");
        assert_eq!(read, events);
        assert_eq!(store.coverage().len(), 2);
        let lookup = store.coverage().coverage_for(&read);
        assert!(lookup.is_complete());
        assert_eq!(lookup.entries.len(), 2);

        let canonical = linearize(read.clone().into_iter());
        assert_eq!(canonical[0].event_id, events[1].event_id);
        let streams = ProducerStreams::from_log_order(read);
        assert!(streams.violations().is_empty());
        assert_eq!(streams.stream("adapter-a").expect("stream").len(), 2);
    }

    #[test]
    fn reopening_appends_rather_than_truncating() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = event("p", "s", ts(10), "00000000-0000-7000-8000-000000000001");
        let b = event("p", "s", ts(20), "00000000-0000-7000-8000-000000000002");
        {
            let mut store = Store::open(dir.path()).expect("open");
            assert_eq!(store.append(&a).expect("append"), AppendOutcome::Appended);
            store.sync().expect("sync");
        }
        {
            let mut store = Store::open(dir.path()).expect("reopen");
            assert_eq!(store.append(&b).expect("append"), AppendOutcome::Appended);
            store.sync().expect("sync");
        }
        let store = Store::open(dir.path()).expect("reopen");
        assert_eq!(store.read_log().expect("read"), vec![a, b]);
    }
}
