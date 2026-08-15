//! Enumerated event kinds. See `schemas/execution-ir/v0.1.json`.

use serde::{Deserialize, Serialize};

/// The kind of a canonical execution IR event.
///
/// Wire form matches `schemas/execution-ir/v0.1.json` exactly (dotted
/// lowercase strings like `"fs.read"`). Adapters and observers extend the
/// IR by adding payload fields to existing kinds, not by inventing new
/// kinds — see `docs/contracts/execution-ir.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EventKind {
    /// A session began.
    #[serde(rename = "session.started")]
    SessionStarted,
    /// A session ended.
    #[serde(rename = "session.ended")]
    SessionEnded,
    /// A computation (one agent-produced-artifact scope) began.
    #[serde(rename = "computation.started")]
    ComputationStarted,
    /// A computation ended.
    #[serde(rename = "computation.ended")]
    ComputationEnded,
    /// A tool was invoked (before it ran).
    #[serde(rename = "tool.invoked")]
    ToolInvoked,
    /// A tool completed (result observed).
    #[serde(rename = "tool.completed")]
    ToolCompleted,
    /// A file was read.
    #[serde(rename = "fs.read")]
    FsRead,
    /// A file was written.
    #[serde(rename = "fs.write")]
    FsWrite,
    /// A file's existence was observed (negative dependencies matter).
    #[serde(rename = "fs.stat")]
    FsStat,
    /// A file was renamed.
    #[serde(rename = "fs.rename")]
    FsRename,
    /// A file was unlinked.
    #[serde(rename = "fs.unlink")]
    FsUnlink,
    /// A directory listing was observed.
    #[serde(rename = "fs.dirlist")]
    FsDirlist,
    /// A process was spawned.
    #[serde(rename = "proc.spawn")]
    ProcSpawn,
    /// A process exited.
    #[serde(rename = "proc.exit")]
    ProcExit,
    /// A network connection was opened.
    #[serde(rename = "net.connect")]
    NetConnect,
    /// A network fetch (higher-level than connect) was observed.
    #[serde(rename = "net.fetch")]
    NetFetch,
    /// A probe checked an external dependency.
    #[serde(rename = "probe.checked")]
    ProbeChecked,
    /// An artifact was produced.
    #[serde(rename = "artifact.produced")]
    ArtifactProduced,
    /// A producer emitted a diagnostic (an unclassifiable runtime event,
    /// backpressure warning, etc.). Silence is a bug; diagnostics are
    /// how producers surface it.
    #[serde(rename = "diagnostic")]
    Diagnostic,
}

impl EventKind {
    /// Wire-format string as it appears in serialized IR events.
    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::SessionStarted => "session.started",
            Self::SessionEnded => "session.ended",
            Self::ComputationStarted => "computation.started",
            Self::ComputationEnded => "computation.ended",
            Self::ToolInvoked => "tool.invoked",
            Self::ToolCompleted => "tool.completed",
            Self::FsRead => "fs.read",
            Self::FsWrite => "fs.write",
            Self::FsStat => "fs.stat",
            Self::FsRename => "fs.rename",
            Self::FsUnlink => "fs.unlink",
            Self::FsDirlist => "fs.dirlist",
            Self::ProcSpawn => "proc.spawn",
            Self::ProcExit => "proc.exit",
            Self::NetConnect => "net.connect",
            Self::NetFetch => "net.fetch",
            Self::ProbeChecked => "probe.checked",
            Self::ArtifactProduced => "artifact.produced",
            Self::Diagnostic => "diagnostic",
        }
    }
}

impl core::fmt::Display for EventKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_wire_str())
    }
}
