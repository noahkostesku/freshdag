//! Store error type.

use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by the store.
///
/// Every variant that can arise from reading a log names the exact
/// physical location (path, line, byte offset) of the problem. Per
/// invariant #7 a store must never make a defect look like an absence of
/// events, so nothing here is recoverable-by-ignoring.
#[derive(Debug, Error)]
pub enum StoreError {
    /// An I/O operation failed.
    #[error("i/o error on `{path}`: {source}")]
    Io {
        /// Path the operation targeted.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// A record terminated by a newline did not parse as an [`IrEvent`].
    ///
    /// [`IrEvent`]: freshdag_core::ir::IrEvent
    #[error("malformed record at `{path}` line {line} (byte offset {byte_offset}): {message}")]
    MalformedRecord {
        /// Log path.
        path: PathBuf,
        /// 1-based line number.
        line: u64,
        /// Byte offset of the first byte of the record.
        byte_offset: u64,
        /// Parse-error description.
        message: String,
    },

    /// The final record in the log has no terminating newline.
    ///
    /// This is what a process killed mid-`write` leaves behind. It is an
    /// error rather than a silently-skipped line: silence about a
    /// half-written observation is exactly the failure invariant #7
    /// forbids. Callers that want to proceed anyway use
    /// [`scan_log`](crate::scan_log), which reports the same condition as
    /// a [`LogDefect`](crate::LogDefect) alongside the events that *did*
    /// survive.
    #[error(
        "truncated trailing record at `{path}` (byte offset {byte_offset}, {len} bytes with no \
         terminating newline); the writing process was probably killed mid-append"
    )]
    TruncatedTrailingRecord {
        /// Log path.
        path: PathBuf,
        /// Byte offset of the first byte of the truncated record.
        byte_offset: u64,
        /// Length in bytes of the truncated fragment.
        len: u64,
    },

    /// An event could not be serialized for append.
    #[error("failed to serialize event for append: {0}")]
    Serialize(#[source] serde_json::Error),
}

impl StoreError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
