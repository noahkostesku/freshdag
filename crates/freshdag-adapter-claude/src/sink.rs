//! Append-only JSONL sink.
//!
//! `docs/contracts/adapter-contract.md §Errors and Backpressure`:
//! *"Adapters MUST NOT block the underlying runtime on IR emission. If
//! the downstream sink is unavailable, the adapter buffers to a local
//! append-only file and drops the newest events with a diagnostic if the
//! buffer is exhausted (never the oldest — invariant #4)."*
//!
//! Concretely:
//!
//! - Writes are `O_APPEND`. Existing bytes are never rewritten, moved or
//!   truncated (invariant #4).
//! - If the configured sink cannot be opened, events go to a sibling
//!   buffer file instead. Failing that, the caller reports to stderr and
//!   the process still exits 0 — the runtime is never blocked.
//! - When the byte cap is reached the **newest** event is dropped and
//!   the caller is told how many, so it can append a
//!   [`crate::diagnostic::DiagnosticCode::SinkBackpressureDrop`]
//!   diagnostic.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use freshdag_core::ir::IrEvent;

/// Default sink byte cap: 64 MiB.
pub const DEFAULT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Bytes held back from the cap so a back-pressure diagnostic can still
/// be appended after the cap is hit.
const DIAGNOSTIC_RESERVE_BYTES: u64 = 8 * 1024;

/// Suffix appended to the sink path when the sink itself cannot be
/// opened.
pub const BUFFER_SUFFIX: &str = ".freshdag-buffer.jsonl";

/// What happened during a [`JsonlSink::write_all`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SinkOutcome {
    /// Events successfully appended.
    pub written: usize,
    /// Newest events dropped because the byte cap was reached.
    pub dropped: usize,
    /// Set when events landed in the fallback buffer instead of the
    /// configured sink.
    pub buffered_to: Option<PathBuf>,
    /// Non-fatal errors, for the caller to report on stderr.
    pub errors: Vec<String>,
}

/// Append-only JSONL writer.
#[derive(Debug, Clone)]
pub struct JsonlSink {
    path: PathBuf,
    max_bytes: u64,
}

impl JsonlSink {
    /// Construct a sink writing to `path` with the default byte cap.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// Override the byte cap.
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// The configured sink path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// First-choice buffer: a sibling of the sink.
    ///
    /// Handles the common failure — the sink *file* is locked, held open
    /// by a crashed reader, or otherwise unwritable while its directory
    /// is fine.
    #[must_use]
    pub fn buffer_path(&self) -> PathBuf {
        let mut s = self.path.clone().into_os_string();
        s.push(BUFFER_SUFFIX);
        PathBuf::from(s)
    }

    /// Last-resort buffer, in the system temp directory.
    ///
    /// A sibling buffer is useless when it is the sink's *directory*
    /// that is unusable (missing, read-only, or shadowed by a file), so
    /// the final fallback deliberately lives somewhere with no
    /// dependency on the configured path. The name is derived from the
    /// sink path so two differently-configured hooks cannot collide.
    #[must_use]
    pub fn fallback_buffer_path(&self) -> PathBuf {
        let digest = blake3::hash(self.path.to_string_lossy().as_bytes())
            .to_hex()
            .to_string();
        std::env::temp_dir().join(format!("freshdag-claude-{}.buffer.jsonl", &digest[..16]))
    }

    /// Open the first usable target: sink, sibling buffer, temp buffer.
    ///
    /// Records every failed attempt in `outcome.errors` so a degraded
    /// path is never invisible.
    fn open_target(&self, outcome: &mut SinkOutcome) -> Option<(PathBuf, std::fs::File)> {
        let candidates = [
            (self.path.clone(), false),
            (self.buffer_path(), true),
            (self.fallback_buffer_path(), true),
        ];
        for (path, is_buffer) in candidates {
            match open_append(&path) {
                Ok(file) => {
                    if is_buffer {
                        outcome.buffered_to = Some(path.clone());
                    }
                    return Some((path, file));
                }
                Err(err) => outcome
                    .errors
                    .push(format!("{} unavailable: {err}", path.display())),
            }
        }
        None
    }

    /// Append `events`, in order, dropping the newest on overflow.
    ///
    /// Never panics and never propagates an I/O error to the caller's
    /// control flow: everything lands in [`SinkOutcome`].
    pub fn write_all(&self, events: &[IrEvent]) -> SinkOutcome {
        let mut outcome = SinkOutcome::default();
        if events.is_empty() {
            return outcome;
        }

        let Some((target, mut file)) = self.open_target(&mut outcome) else {
            outcome.errors.push(format!(
                "no sink or buffer could be opened; {} event(s) not persisted",
                events.len()
            ));
            outcome.dropped = events.len();
            return outcome;
        };

        let mut size = file_len(&target);
        for event in events {
            let Some(mut line) = serialize_line(event, &mut outcome) else {
                continue;
            };
            let len = line.len() as u64;
            if size.saturating_add(len) > self.max_bytes.saturating_sub(DIAGNOSTIC_RESERVE_BYTES) {
                // Drop the NEWEST, never the oldest (invariant #4).
                outcome.dropped += 1;
                continue;
            }
            line.push(b'\n');
            match file.write_all(&line) {
                Ok(()) => {
                    outcome.written += 1;
                    size += len + 1;
                }
                Err(err) => {
                    outcome
                        .errors
                        .push(format!("append to {} failed: {err}", target.display()));
                    outcome.dropped += 1;
                }
            }
        }
        if let Err(err) = file.flush() {
            outcome
                .errors
                .push(format!("flush of {} failed: {err}", target.display()));
        }
        outcome
    }

    /// Append a single event ignoring the byte cap, using the reserve.
    ///
    /// Used only to record a back-pressure drop: the record of a drop
    /// must never itself be dropped.
    pub fn force_write(&self, event: &IrEvent) -> SinkOutcome {
        let mut outcome = SinkOutcome::default();
        let Some((path, mut file)) = self.open_target(&mut outcome) else {
            outcome
                .errors
                .push("the record of a dropped event could not itself be persisted".to_string());
            return outcome;
        };
        if let Some(mut line) = serialize_line(event, &mut outcome) {
            line.push(b'\n');
            match file.write_all(&line) {
                Ok(()) => outcome.written += 1,
                Err(err) => outcome
                    .errors
                    .push(format!("append to {} failed: {err}", path.display())),
            }
        }
        outcome
    }
}

fn open_append(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

fn serialize_line(event: &IrEvent, outcome: &mut SinkOutcome) -> Option<Vec<u8>> {
    match serde_json::to_vec(event) {
        Ok(bytes) => Some(bytes),
        Err(err) => {
            outcome.errors.push(format!(
                "event {} failed to serialize: {err}",
                event.event_id
            ));
            outcome.dropped += 1;
            None
        }
    }
}
