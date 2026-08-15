//! Typed payload variants for the S0 event kinds.
//!
//! [`TypedPayload`] is a convenience for producers and consumers that
//! want strongly-typed access to payload fields. The envelope
//! ([`super::IrEvent`]) always stores the payload as `serde_json::Value`
//! so that unknown-kind events round-trip losslessly. Use
//! [`super::IrEvent::decode_payload`] to project into a `TypedPayload`.
//!
//! Only the S0 minimum is typed here (`FsRead`, `FsWrite`,
//! `ToolInvoked`, `ToolCompleted`). Additional variants land alongside
//! the producers that emit them.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::hash::Hash;

/// Read-mode annotation for `fs.read` — currently only used to mark
/// `mmap`-observed reads pessimistically per the observer contract.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FsReadKind {
    /// Ordinary `read()` or equivalent syscall.
    #[default]
    Direct,
    /// Observer emitted this at `mmap` time because it cannot see faulted
    /// pages. Pessimistic: assume full read.
    MmapPessimistic,
}

/// Payload for [`super::EventKind::FsRead`].
///
/// `path` is expected to be canonicalized to an absolute path by the
/// producer; `raw_path` retains the observed form for debugging.
/// `hash` is `None` when the producer chose not to hash (e.g., very
/// large files with a cheaper heuristic elsewhere). `None` never means
/// "unknown-but-treat-as-valid" — see invariant #7.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsRead {
    /// Canonicalized absolute path.
    pub path: PathBuf,
    /// Bytes read (best-effort; producer-declared).
    pub size: u64,
    /// Content hash if the producer computed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<Hash>,
    /// If `path` was a symlink, the resolved target at observation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_symlink_target: Option<PathBuf>,
    /// The path as observed before canonicalization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_path: Option<PathBuf>,
    /// Read mode (`direct` vs pessimistic `mmap`).
    #[serde(default, skip_serializing_if = "is_default_read_kind")]
    pub read_kind: FsReadKind,
    /// Producer flags this read as impure (e.g., `/dev/urandom`, clock).
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub impure: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if takes &T
fn is_default_read_kind(k: &FsReadKind) -> bool {
    matches!(k, FsReadKind::Direct)
}

/// Write mode for [`FsWrite`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsWriteMode {
    /// New file created.
    Create,
    /// Appended to existing file.
    Append,
    /// Existing file truncated and rewritten.
    Truncate,
}

/// Payload for [`super::EventKind::FsWrite`].
///
/// After a rename-atomic write (`write(foo.tmp)` + `rename(foo.tmp, foo)`)
/// the observer emits a synthetic `FsWrite { path: foo }` on the rename
/// target — see `docs/contracts/observer-contract.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsWrite {
    /// Canonicalized absolute path of the final write target.
    pub path: PathBuf,
    /// Bytes written (final size on close).
    pub size: u64,
    /// Content hash if the producer computed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<Hash>,
    /// Write mode.
    pub mode: FsWriteMode,
    /// The path as observed before canonicalization (or the pre-rename name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_path: Option<PathBuf>,
}

/// Category of tool being invoked; drives adapter-level fan-out to
/// specialized handling (e.g., MCP tool names get a `mcp/<server>/<tool>`
/// prefix per contract §Tool interaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    /// Runtime-native tool (e.g., Claude Code's `Read`, `Write`, `Edit`).
    Builtin,
    /// MCP tool (`mcp/<server>/<tool>`).
    Mcp,
    /// Skill invocation.
    Skill,
    /// Subagent/task delegation.
    Task,
    /// Shell subprocess.
    Bash,
}

/// Payload for [`super::EventKind::ToolInvoked`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvoked {
    /// Tool identifier, normalized per §Tool interaction.
    pub tool_name: String,
    /// Category of tool.
    pub tool_kind: ToolKind,
    /// Serialized tool input; opaque to the core.
    pub tool_input: serde_json::Value,
    /// Working directory at invocation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

/// Payload for [`super::EventKind::ToolCompleted`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCompleted {
    /// Serialized tool output; opaque to the core.
    pub tool_output: serde_json::Value,
    /// Whether the tool call errored (transport-level, not application-level).
    pub is_error: bool,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// Typed projection of the payload for the S0 subset of event kinds.
///
/// Access via [`super::IrEvent::decode_payload`] returns this enum for
/// events whose kind has a typed variant; other kinds return an error
/// pointing the consumer at the raw `payload` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypedPayload {
    /// `fs.read` payload.
    FsRead(FsRead),
    /// `fs.write` payload.
    FsWrite(FsWrite),
    /// `tool.invoked` payload.
    ToolInvoked(ToolInvoked),
    /// `tool.completed` payload.
    ToolCompleted(ToolCompleted),
}
