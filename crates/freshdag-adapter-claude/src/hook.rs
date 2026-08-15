//! Claude Code hook vocabulary.
//!
//! **Everything Claude-Code-specific lives here or in
//! [`crate::compile`].** Invariants #1, #2 and #14 forbid these names
//! (`PreToolUse`, `tool_use_id`, `transcript_path`, …) from reaching
//! `freshdag-core`; nothing in this module is re-exported into a core
//! type.
//!
//! Hook payloads are parsed leniently. A payload the adapter cannot
//! classify becomes a `diagnostic` IR event, never a silent drop
//! (`docs/contracts/adapter-contract.md §Responsibilities #5`).

/// Hook event names this adapter recognizes.
///
/// Registered with matcher `.*` for every variant, per
/// `docs/contracts/adapter-contract.md §The Claude Code Adapter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HookEvent {
    /// Fires before a tool runs. Compiles to `tool.invoked` (+ any
    /// synthesized `fs.*`).
    PreToolUse,
    /// Fires after a tool ran. Compiles to `tool.completed`.
    PostToolUse,
    /// Fires when the user submits a prompt. Recognized, but the v0 IR
    /// has no kind for a conversational turn — see
    /// [`crate::diagnostic::DiagnosticCode::UnmappedHookEvent`].
    UserPromptSubmit,
    /// Fires when the main agent stops responding. Recognized, unmapped.
    Stop,
    /// Fires when a `Task` subagent stops. Recognized, unmapped.
    SubagentStop,
    /// Fires at session start. Compiles to `session.started` +
    /// `computation.started`.
    SessionStart,
    /// Fires at session end. Compiles to `computation.ended` +
    /// `session.ended`.
    SessionEnd,
    /// Fires before context compaction. Recognized, unmapped.
    PreCompact,
    /// Fires on a runtime notification. Recognized, unmapped.
    Notification,
}

impl HookEvent {
    /// All recognized hook events, in declaration order.
    pub const ALL: [Self; 9] = [
        Self::PreToolUse,
        Self::PostToolUse,
        Self::UserPromptSubmit,
        Self::Stop,
        Self::SubagentStop,
        Self::SessionStart,
        Self::SessionEnd,
        Self::PreCompact,
        Self::Notification,
    ];

    /// Parse the `hook_event_name` field. Returns `None` for names this
    /// adapter version does not recognize; the caller emits a
    /// `diagnostic` rather than dropping the payload.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "PreToolUse" => Some(Self::PreToolUse),
            "PostToolUse" => Some(Self::PostToolUse),
            "UserPromptSubmit" => Some(Self::UserPromptSubmit),
            "Stop" => Some(Self::Stop),
            "SubagentStop" => Some(Self::SubagentStop),
            "SessionStart" => Some(Self::SessionStart),
            "SessionEnd" => Some(Self::SessionEnd),
            "PreCompact" => Some(Self::PreCompact),
            "Notification" => Some(Self::Notification),
            _ => None,
        }
    }

    /// The wire name as Claude Code spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
            Self::SubagentStop => "SubagentStop",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::PreCompact => "PreCompact",
            Self::Notification => "Notification",
        }
    }

    /// Does this hook event compile to at least one non-`diagnostic` IR
    /// event in this adapter version?
    #[must_use]
    pub const fn has_ir_mapping(self) -> bool {
        matches!(
            self,
            Self::PreToolUse | Self::PostToolUse | Self::SessionStart | Self::SessionEnd
        )
    }
}

impl core::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Prefix Claude Code uses for MCP tool names: `mcp__<server>__<tool>`.
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// The Claude Code tool name for a skill invocation.
pub const SKILL_TOOL_NAME: &str = "Skill";

/// The Claude Code tool name for a shell subprocess.
pub const BASH_TOOL_NAME: &str = "Bash";

/// The Claude Code tool name for a subagent delegation.
pub const TASK_TOOL_NAME: &str = "Task";

/// Tool-input keys that name a file the tool reads.
pub const READ_TOOLS: [&str; 1] = ["Read"];

/// Tools whose `tool_input` names a file the tool writes.
///
/// `NotebookEdit` uses `notebook_path` rather than `file_path`; see
/// [`file_path_key`].
pub const WRITE_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// The `tool_input` key that carries the target path for a given tool.
#[must_use]
pub fn file_path_key(tool_name: &str) -> &'static str {
    if tool_name == "NotebookEdit" {
        "notebook_path"
    } else {
        "file_path"
    }
}

/// Split an MCP tool name of the form `mcp__<server>__<tool>` into its
/// server and tool halves.
///
/// Returns `None` when the name does not have all three segments — the
/// caller emits a
/// [`crate::diagnostic::DiagnosticCode::ToolNameNormalizationFailed`]
/// diagnostic and keeps the raw name rather than inventing one.
#[must_use]
pub fn split_mcp_tool_name(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.strip_prefix(MCP_TOOL_PREFIX)?;
    // Server names do not contain `__`; tool names may.
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

/// Extract a skill name from a `Skill` tool's `tool_input`.
///
/// Claude Code has spelled this key differently across versions; we
/// accept the known spellings and give up (loudly) otherwise.
#[must_use]
pub fn skill_name_from_input(tool_input: &serde_json::Value) -> Option<&str> {
    for key in ["command", "name", "skill", "skill_name"] {
        if let Some(s) = tool_input.get(key).and_then(serde_json::Value::as_str) {
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_every_registered_hook_event() {
        for ev in HookEvent::ALL {
            assert_eq!(HookEvent::parse(ev.as_str()), Some(ev));
        }
    }

    #[test]
    fn unknown_hook_event_is_not_silently_accepted() {
        assert_eq!(HookEvent::parse("Frobnicate"), None);
        assert_eq!(HookEvent::parse("pretooluse"), None);
    }

    #[test]
    fn mcp_names_split_on_double_underscore() {
        assert_eq!(
            split_mcp_tool_name("mcp__linear__create_issue"),
            Some(("linear", "create_issue"))
        );
        // Tool halves may themselves contain `__`.
        assert_eq!(split_mcp_tool_name("mcp__srv__a__b"), Some(("srv", "a__b")));
    }

    #[test]
    fn malformed_mcp_names_are_rejected_not_guessed() {
        assert_eq!(split_mcp_tool_name("mcp__linear"), None);
        assert_eq!(split_mcp_tool_name("mcp____tool"), None);
        assert_eq!(split_mcp_tool_name("Read"), None);
    }

    #[test]
    fn notebook_edit_uses_its_own_path_key() {
        assert_eq!(file_path_key("NotebookEdit"), "notebook_path");
        assert_eq!(file_path_key("Write"), "file_path");
    }

    #[test]
    fn skill_name_extraction_accepts_known_spellings() {
        assert_eq!(
            skill_name_from_input(&serde_json::json!({"command": "pdf"})),
            Some("pdf")
        );
        assert_eq!(
            skill_name_from_input(&serde_json::json!({"skill_name": "xlsx"})),
            Some("xlsx")
        );
        assert_eq!(skill_name_from_input(&serde_json::json!({"x": 1})), None);
        assert_eq!(
            skill_name_from_input(&serde_json::json!({"command": ""})),
            None
        );
    }

    #[test]
    fn unmapped_events_are_declared_as_such() {
        assert!(HookEvent::PreToolUse.has_ir_mapping());
        assert!(HookEvent::SessionEnd.has_ir_mapping());
        assert!(!HookEvent::Stop.has_ir_mapping());
        assert!(!HookEvent::PreCompact.has_ir_mapping());
    }
}
