//! Append-only JSONL sink for canonical IR events.
//!
//! One event per line, newline-delimited JSON, opened `O_APPEND`. History
//! is never rewritten (invariant #4) — the only write this module ever
//! performs is an append of a complete record.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use freshdag_core::ir::{EventKind, IrEvent};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::StoreError;

/// Producer identity the store uses for the diagnostics it authors itself.
pub const STORE_PRODUCER: &str = "freshdag-store";

/// Bytes held back from [`SinkOptions::max_bytes`] so a drop diagnostic can
/// always be written. A budget that leaves no room to say "I dropped
/// something" would make the store silent, which is a bug, not a
/// configuration.
const DIAGNOSTIC_RESERVE_BYTES: u64 = 2048;

/// Sink configuration.
#[derive(Debug, Clone, Default)]
pub struct SinkOptions {
    /// Optional byte budget for the log file.
    ///
    /// `None` (the default) means "bounded only by the filesystem".
    /// When set and exhausted, the sink drops the **newest** event and
    /// records a `diagnostic`, never the oldest — see
    /// `docs/contracts/adapter-contract.md §Errors and Backpressure` and
    /// invariant #4.
    pub max_bytes: Option<u64>,
    /// Call `fsync` after every append.
    ///
    /// Off by default: the durability the log needs is "a record is
    /// either wholly present or detectably truncated", which the reader
    /// provides without paying an fsync per event. Turn this on where
    /// the log must survive a machine (not just a process) crash.
    pub sync_on_append: bool,
}

/// What an [`JsonlSink::append`] call actually did.
///
/// Returned rather than swallowed so that back-pressure can never be
/// invisible to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "an append may have dropped the event; check the outcome"]
pub enum AppendOutcome {
    /// The event was written to the log.
    Appended,
    /// The byte budget was exhausted; the *newest* event (this one) was
    /// dropped and a `diagnostic` was recorded in its place.
    DroppedNewest {
        /// Total events dropped by this sink so far, including this one.
        total_dropped: u64,
    },
}

/// An append-only newline-delimited JSON event sink.
///
/// Not `Sync`-safe for concurrent use from multiple handles in the same
/// process — use one sink per writer. Multiple *processes* appending to
/// the same file are safe for records small enough to be written
/// atomically under `O_APPEND`; the reader detects torn records either
/// way.
#[derive(Debug)]
pub struct JsonlSink {
    path: PathBuf,
    file: File,
    options: SinkOptions,
    bytes_written: u64,
    dropped: u64,
    announced_drop: bool,
}

impl JsonlSink {
    /// Open (creating if necessary) an append-only log at `path`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`] if the file cannot be opened or its length read.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::with_options(path, SinkOptions::default())
    }

    /// Open an append-only log with explicit options.
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`] if the file cannot be opened or its length read.
    pub fn with_options(path: impl AsRef<Path>, options: SinkOptions) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| StoreError::io(parent, e))?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| StoreError::io(&path, e))?;
        let bytes_written = file.metadata().map_err(|e| StoreError::io(&path, e))?.len();
        Ok(Self {
            path,
            file,
            options,
            bytes_written,
            dropped: 0,
            announced_drop: false,
        })
    }

    /// Path of the log this sink appends to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes appended to the log (including bytes present before this
    /// sink opened it).
    pub fn len_bytes(&self) -> u64 {
        self.bytes_written
    }

    /// Number of events this sink dropped due to an exhausted budget.
    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }

    /// Append one event.
    ///
    /// Writes the record as a single `write_all` of `json + b'\n'`, so a
    /// crash can leave at most one partial record, at the tail, which the
    /// reader detects.
    ///
    /// # Errors
    ///
    /// - [`StoreError::Serialize`] if the event does not serialize.
    /// - [`StoreError::Io`] if the write or optional `fsync` fails.
    pub fn append(&mut self, event: &IrEvent) -> Result<AppendOutcome, StoreError> {
        let record = encode(event)?;
        let len = record.len() as u64;

        if self.would_exceed_budget(len) {
            self.dropped += 1;
            let total_dropped = self.dropped;
            if !self.announced_drop {
                self.announced_drop = true;
                let diagnostic = drop_diagnostic(event, total_dropped);
                // Written into the reserved headroom; a budget that cannot
                // fit its own diagnostic is a misconfiguration, and we
                // would rather overshoot the budget than go silent.
                let encoded = encode(&diagnostic)?;
                self.write_record(&encoded)?;
            }
            return Ok(AppendOutcome::DroppedNewest { total_dropped });
        }

        self.write_record(&record)?;
        Ok(AppendOutcome::Appended)
    }

    /// Append many events, stopping at the first I/O or serialization
    /// error. Returns the outcome of each attempted append, in order.
    ///
    /// # Errors
    ///
    /// As [`Self::append`].
    pub fn append_all<'e>(
        &mut self,
        events: impl IntoIterator<Item = &'e IrEvent>,
    ) -> Result<Vec<AppendOutcome>, StoreError> {
        events.into_iter().map(|e| self.append(e)).collect()
    }

    /// Flush OS buffers for this log to stable storage.
    ///
    /// # Errors
    ///
    /// [`StoreError::Io`] if the flush or `fsync` fails.
    pub fn sync(&mut self) -> Result<(), StoreError> {
        self.file
            .flush()
            .map_err(|e| StoreError::io(&self.path, e))?;
        self.file
            .sync_data()
            .map_err(|e| StoreError::io(&self.path, e))
    }

    /// Record a final `diagnostic` naming the total number of dropped
    /// events, if any were dropped, and sync.
    ///
    /// Call this when a producer's session ends. It is not run from
    /// `Drop` because it can fail and `Drop` cannot report that.
    ///
    /// # Errors
    ///
    /// As [`Self::append`] and [`Self::sync`].
    pub fn finish(&mut self, session_id: &str) -> Result<(), StoreError> {
        if self.dropped > 0 {
            let summary = drop_summary(session_id, self.dropped);
            let encoded = encode(&summary)?;
            self.write_record(&encoded)?;
        }
        self.sync()
    }

    fn would_exceed_budget(&self, record_len: u64) -> bool {
        match self.options.max_bytes {
            None => false,
            Some(max) => {
                let usable = max.saturating_sub(DIAGNOSTIC_RESERVE_BYTES);
                self.bytes_written.saturating_add(record_len) > usable
            }
        }
    }

    fn write_record(&mut self, record: &[u8]) -> Result<(), StoreError> {
        self.file
            .write_all(record)
            .map_err(|e| StoreError::io(&self.path, e))?;
        self.bytes_written = self.bytes_written.saturating_add(record.len() as u64);
        if self.options.sync_on_append {
            self.sync()?;
        }
        Ok(())
    }
}

/// Serialize one event as a complete JSONL record (compact JSON plus the
/// terminating newline) in a single buffer.
fn encode(event: &IrEvent) -> Result<Vec<u8>, StoreError> {
    let mut buf = serde_json::to_vec(event).map_err(StoreError::Serialize)?;
    buf.push(b'\n');
    Ok(buf)
}

/// Build the `diagnostic` event announcing that back-pressure forced a
/// drop of the newest event.
fn drop_diagnostic(dropped: &IrEvent, total_dropped: u64) -> IrEvent {
    IrEvent {
        event_id: Uuid::now_v7(),
        producer: STORE_PRODUCER.to_string(),
        producer_version: env!("CARGO_PKG_VERSION").to_string(),
        session_id: dropped.session_id.clone(),
        computation_id: dropped.computation_id.clone(),
        parent_id: None,
        causal_inputs: Some(vec![dropped.event_id]),
        ts: OffsetDateTime::now_utc(),
        kind: EventKind::Diagnostic,
        payload: serde_json::json!({
            "message": "sink byte budget exhausted; dropping newest events",
            "reason": "sink_budget_exhausted",
            "dropped_newest": true,
            "dropped_event_id": dropped.event_id,
            "dropped_kind": dropped.kind.as_wire_str(),
            "dropped_producer": dropped.producer,
            "total_dropped": total_dropped,
        }),
    }
}

/// Build the closing `diagnostic` summarizing total drops for a session.
fn drop_summary(session_id: &str, total_dropped: u64) -> IrEvent {
    IrEvent {
        event_id: Uuid::now_v7(),
        producer: STORE_PRODUCER.to_string(),
        producer_version: env!("CARGO_PKG_VERSION").to_string(),
        session_id: session_id.to_string(),
        computation_id: None,
        parent_id: None,
        causal_inputs: None,
        ts: OffsetDateTime::now_utc(),
        kind: EventKind::Diagnostic,
        payload: serde_json::json!({
            "message": "sink closed after dropping events under back-pressure",
            "reason": "sink_budget_exhausted",
            "dropped_newest": true,
            "total_dropped": total_dropped,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::read_log;
    use crate::testing::{event, ts};

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn fixture(n: u8, at_secs: i64) -> IrEvent {
        event(
            "adapter-a",
            "s1",
            ts(at_secs),
            &format!("00000000-0000-7000-8000-0000000000{n:02}"),
        )
    }

    #[test]
    fn each_record_is_one_line() {
        let dir = tmp();
        let path = dir.path().join("events.jsonl");
        let mut sink = JsonlSink::open(&path).expect("open");
        for n in 1..=3 {
            assert_eq!(
                sink.append(&fixture(n, i64::from(n))).expect("append"),
                AppendOutcome::Appended
            );
        }
        sink.sync().expect("sync");

        let raw = std::fs::read_to_string(&path).expect("read");
        assert_eq!(raw.lines().count(), 3);
        assert!(raw.ends_with('\n'), "a complete log ends with a newline");
        assert_eq!(sink.len_bytes(), raw.len() as u64);
        assert!(!raw.contains("\n\n"));
    }

    #[test]
    fn nested_directories_are_created() {
        let dir = tmp();
        let path = dir.path().join("a/b/c/events.jsonl");
        let mut sink = JsonlSink::open(&path).expect("open");
        assert_eq!(
            sink.append(&fixture(1, 1)).expect("append"),
            AppendOutcome::Appended
        );
        sink.sync().expect("sync");
        assert_eq!(read_log(&path).expect("read").len(), 1);
    }

    #[test]
    fn exhausted_budget_drops_the_newest_and_says_so() {
        let dir = tmp();
        let path = dir.path().join("events.jsonl");
        // Room for the reserve plus roughly two records.
        let one = serde_json::to_vec(&fixture(1, 1)).expect("encode").len() as u64 + 1;
        let mut sink = JsonlSink::with_options(
            &path,
            SinkOptions {
                max_bytes: Some(DIAGNOSTIC_RESERVE_BYTES + one * 2),
                sync_on_append: false,
            },
        )
        .expect("open");

        assert_eq!(
            sink.append(&fixture(1, 1)).expect("a"),
            AppendOutcome::Appended
        );
        assert_eq!(
            sink.append(&fixture(2, 2)).expect("b"),
            AppendOutcome::Appended
        );
        assert_eq!(
            sink.append(&fixture(3, 3)).expect("c"),
            AppendOutcome::DroppedNewest { total_dropped: 1 }
        );
        assert_eq!(
            sink.append(&fixture(4, 4)).expect("d"),
            AppendOutcome::DroppedNewest { total_dropped: 2 }
        );
        sink.finish("s1").expect("finish");

        let log = read_log(&path).expect("read");
        // Invariant #4: the OLDEST events survive; the newest were dropped.
        let ids: Vec<_> = log
            .iter()
            .filter(|e| e.producer == "adapter-a")
            .map(|e| e.event_id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], fixture(1, 1).event_id);
        assert_eq!(ids[1], fixture(2, 2).event_id);

        // And the drop is never silent.
        let diagnostics: Vec<_> = log
            .iter()
            .filter(|e| e.kind == EventKind::Diagnostic)
            .collect();
        assert_eq!(diagnostics.len(), 2, "one drop announcement, one summary");
        assert_eq!(diagnostics[0].producer, STORE_PRODUCER);
        assert_eq!(diagnostics[0].payload["reason"], "sink_budget_exhausted");
        assert_eq!(diagnostics[0].payload["dropped_newest"], true);
        assert_eq!(
            diagnostics[0].payload["dropped_event_id"],
            serde_json::json!(fixture(3, 3).event_id)
        );
        assert_eq!(diagnostics[1].payload["total_dropped"], 2);
        assert_eq!(sink.dropped_count(), 2);
    }

    #[test]
    fn a_clean_run_emits_no_drop_summary() {
        let dir = tmp();
        let path = dir.path().join("events.jsonl");
        let mut sink = JsonlSink::open(&path).expect("open");
        assert_eq!(
            sink.append(&fixture(1, 1)).expect("append"),
            AppendOutcome::Appended
        );
        sink.finish("s1").expect("finish");
        let log = read_log(&path).expect("read");
        assert_eq!(log.len(), 1);
        assert_eq!(sink.dropped_count(), 0);
    }

    #[test]
    fn sync_on_append_still_writes_one_line_per_event() {
        let dir = tmp();
        let path = dir.path().join("events.jsonl");
        let mut sink = JsonlSink::with_options(
            &path,
            SinkOptions {
                max_bytes: None,
                sync_on_append: true,
            },
        )
        .expect("open");
        for n in 1..=2 {
            assert_eq!(
                sink.append(&fixture(n, i64::from(n))).expect("append"),
                AppendOutcome::Appended
            );
        }
        assert_eq!(read_log(&path).expect("read").len(), 2);
    }
}
