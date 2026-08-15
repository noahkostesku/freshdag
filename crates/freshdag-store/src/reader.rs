//! JSONL log reader.
//!
//! Two entry points, deliberately:
//!
//! - [`read_log`] — strict. Any defect is a [`StoreError`].
//! - [`scan_log`] — tolerant. Returns the events that parsed *plus* a
//!   [`LogDefect`] for every byte the reader could not turn into an
//!   event.
//!
//! There is no third mode that skips bad records quietly. A record the
//! store cannot read is evidence that may bear on freshness, and
//! pretending it was never there is the "silence means nothing happened"
//! failure invariant #7 forbids.

use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use freshdag_core::ir::IrEvent;

use crate::error::StoreError;

/// A byte range of the log that did not yield an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogDefect {
    /// A newline-terminated record that failed to parse.
    MalformedRecord {
        /// 1-based line number.
        line: u64,
        /// Byte offset of the record's first byte.
        byte_offset: u64,
        /// The raw bytes, preserved verbatim for forensics.
        raw: Vec<u8>,
        /// Parser error description.
        message: String,
    },
    /// The final record has no terminating newline — the signature of a
    /// process killed mid-append.
    TruncatedTrailingRecord {
        /// 1-based line number.
        line: u64,
        /// Byte offset of the fragment's first byte.
        byte_offset: u64,
        /// The raw surviving bytes, preserved verbatim.
        raw: Vec<u8>,
    },
}

impl LogDefect {
    /// Byte offset of the first byte of the offending region.
    pub fn byte_offset(&self) -> u64 {
        match self {
            Self::MalformedRecord { byte_offset, .. }
            | Self::TruncatedTrailingRecord { byte_offset, .. } => *byte_offset,
        }
    }

    /// 1-based line number of the offending region.
    pub fn line(&self) -> u64 {
        match self {
            Self::MalformedRecord { line, .. } | Self::TruncatedTrailingRecord { line, .. } => {
                *line
            }
        }
    }

    /// The raw bytes of the offending region.
    pub fn raw(&self) -> &[u8] {
        match self {
            Self::MalformedRecord { raw, .. } | Self::TruncatedTrailingRecord { raw, .. } => raw,
        }
    }
}

impl fmt::Display for LogDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedRecord {
                line,
                byte_offset,
                raw,
                message,
            } => write!(
                f,
                "malformed record at line {line} (byte offset {byte_offset}, {} bytes): {message}",
                raw.len()
            ),
            Self::TruncatedTrailingRecord {
                line,
                byte_offset,
                raw,
            } => write!(
                f,
                "truncated trailing record at line {line} (byte offset {byte_offset}, {} bytes \
                 with no terminating newline)",
                raw.len()
            ),
        }
    }
}

/// Result of a tolerant read.
#[derive(Debug, Clone, Default)]
pub struct LogScan {
    /// Events that parsed, in physical (append) order.
    pub events: Vec<IrEvent>,
    /// Every byte range that did not yield an event.
    pub defects: Vec<LogDefect>,
}

impl LogScan {
    /// Did the whole log parse?
    pub fn is_clean(&self) -> bool {
        self.defects.is_empty()
    }
}

/// Read a log strictly. Every record must parse and the file must end
/// with a complete record.
///
/// Events are returned in **physical (append) order**. Use
/// [`ProducerStreams`](crate::ProducerStreams) for per-producer
/// `event_id` order, or [`linearize`](crate::linearize) for the canonical
/// cross-producer total order.
///
/// # Errors
///
/// - [`StoreError::Io`] if the log cannot be opened or read.
/// - [`StoreError::MalformedRecord`] on the first unparseable record.
/// - [`StoreError::TruncatedTrailingRecord`] if the log ends mid-record.
pub fn read_log(path: impl AsRef<Path>) -> Result<Vec<IrEvent>, StoreError> {
    let path = path.as_ref();
    let scan = scan_log(path)?;
    if let Some(defect) = scan.defects.into_iter().next() {
        return Err(match defect {
            LogDefect::MalformedRecord {
                line,
                byte_offset,
                message,
                ..
            } => StoreError::MalformedRecord {
                path: path.to_path_buf(),
                line,
                byte_offset,
                message,
            },
            LogDefect::TruncatedTrailingRecord {
                byte_offset, raw, ..
            } => StoreError::TruncatedTrailingRecord {
                path: path.to_path_buf(),
                byte_offset,
                len: raw.len() as u64,
            },
        });
    }
    Ok(scan.events)
}

/// Read a log tolerantly, reporting defects instead of failing.
///
/// A missing log file is *not* a defect — it reads as an empty log,
/// because "no producer has appended yet" is a legitimate state. A
/// present-but-damaged log always reports.
///
/// # Errors
///
/// [`StoreError::Io`] if the log exists but cannot be opened or read.
pub fn scan_log(path: impl AsRef<Path>) -> Result<LogScan, StoreError> {
    let path = path.as_ref();
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(LogScan::default()),
        Err(e) => return Err(StoreError::io(path, e)),
    };
    scan_reader(BufReader::new(file), path)
}

fn scan_reader<R: BufRead>(mut reader: R, path: &Path) -> Result<LogScan, StoreError> {
    let mut scan = LogScan::default();
    let mut byte_offset: u64 = 0;
    let mut line: u64 = 0;
    let mut buf: Vec<u8> = Vec::new();

    loop {
        buf.clear();
        let read = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| StoreError::io(path, e))?;
        if read == 0 {
            break;
        }
        line += 1;

        let terminated = buf.last() == Some(&b'\n');
        let record: &[u8] = if terminated {
            &buf[..buf.len() - 1]
        } else {
            &buf
        };

        if !terminated {
            // Last bytes in the file, no newline: a torn append.
            scan.defects.push(LogDefect::TruncatedTrailingRecord {
                line,
                byte_offset,
                raw: record.to_vec(),
            });
            break;
        }

        match serde_json::from_slice::<IrEvent>(record) {
            Ok(event) => scan.events.push(event),
            Err(e) => scan.defects.push(LogDefect::MalformedRecord {
                line,
                byte_offset,
                raw: record.to_vec(),
                message: e.to_string(),
            }),
        }

        byte_offset += read as u64;
    }

    Ok(scan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{event, ts};
    use crate::JsonlSink;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn missing_log_reads_as_empty() {
        let dir = tmp();
        let scan = scan_log(dir.path().join("nope.jsonl")).expect("scan");
        assert!(scan.events.is_empty());
        assert!(scan.is_clean());
    }

    #[test]
    fn round_trips_events_in_append_order() {
        let dir = tmp();
        let path = dir.path().join("events.jsonl");
        let events = vec![
            event(
                "adapter-a",
                "s1",
                ts(30),
                "00000000-0000-7000-8000-000000000003",
            ),
            event(
                "adapter-a",
                "s1",
                ts(10),
                "00000000-0000-7000-8000-000000000001",
            ),
            event(
                "adapter-b",
                "s1",
                ts(20),
                "00000000-0000-7000-8000-000000000002",
            ),
        ];
        let mut sink = JsonlSink::open(&path).expect("open");
        sink.append_all(&events).expect("append");
        sink.sync().expect("sync");

        let read = read_log(&path).expect("read");
        assert_eq!(read, events, "reader must preserve physical append order");
    }

    #[test]
    fn truncated_trailing_line_is_an_error_not_a_skip() {
        let dir = tmp();
        let path = dir.path().join("events.jsonl");
        let e = event(
            "adapter-a",
            "s1",
            ts(10),
            "00000000-0000-7000-8000-000000000001",
        );
        let mut sink = JsonlSink::open(&path).expect("open");
        assert_eq!(
            sink.append(&e).expect("append"),
            crate::AppendOutcome::Appended
        );
        sink.sync().expect("sync");
        drop(sink);

        // Simulate a process killed mid-write: a partial record with no
        // terminating newline.
        let fragment = br#"{"event_id":"00000000-0000-7000-8000-0000000"#;
        let mut raw = std::fs::read(&path).expect("read");
        raw.extend_from_slice(fragment);
        std::fs::write(&path, &raw).expect("write");

        let err = read_log(&path).expect_err("strict read must fail");
        match err {
            StoreError::TruncatedTrailingRecord { len, .. } => {
                assert_eq!(len, fragment.len() as u64);
            }
            other => panic!("wrong error: {other}"),
        }

        let scan = scan_log(&path).expect("scan");
        assert_eq!(scan.events, vec![e], "surviving events must still be read");
        assert_eq!(scan.defects.len(), 1);
        assert!(matches!(
            scan.defects[0],
            LogDefect::TruncatedTrailingRecord { line: 2, .. }
        ));
    }

    #[test]
    fn malformed_interior_record_is_reported_with_its_bytes() {
        let dir = tmp();
        let path = dir.path().join("events.jsonl");
        let a = event(
            "adapter-a",
            "s1",
            ts(10),
            "00000000-0000-7000-8000-000000000001",
        );
        let b = event(
            "adapter-a",
            "s1",
            ts(20),
            "00000000-0000-7000-8000-000000000002",
        );
        let mut raw = serde_json::to_vec(&a).unwrap();
        raw.push(b'\n');
        raw.extend_from_slice(b"{not json}\n");
        raw.extend_from_slice(&serde_json::to_vec(&b).unwrap());
        raw.push(b'\n');
        std::fs::write(&path, &raw).expect("write");

        assert!(matches!(
            read_log(&path),
            Err(StoreError::MalformedRecord { line: 2, .. })
        ));

        let scan = scan_log(&path).expect("scan");
        assert_eq!(scan.events, vec![a, b]);
        assert_eq!(scan.defects.len(), 1);
        assert_eq!(scan.defects[0].raw(), b"{not json}");
        assert_eq!(scan.defects[0].line(), 2);
    }

    #[test]
    fn blank_record_is_reported_never_skipped() {
        let dir = tmp();
        let path = dir.path().join("events.jsonl");
        std::fs::write(&path, b"\n").expect("write");
        let scan = scan_log(&path).expect("scan");
        assert!(scan.events.is_empty());
        assert_eq!(
            scan.defects.len(),
            1,
            "an empty line is a defect, not a no-op"
        );
    }
}
