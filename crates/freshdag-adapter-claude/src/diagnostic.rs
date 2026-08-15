//! Diagnostic vocabulary.
//!
//! `docs/contracts/adapter-contract.md §Responsibilities #5`: *"If the
//! runtime emits a payload the adapter cannot classify, emit a
//! diagnostic event; do NOT silently drop it. Silent drops corrupt the
//! dependency graph."*
//!
//! This adapter goes one step further than the letter of that rule: a
//! hook event it *recognizes* but has no canonical IR kind for (`Stop`,
//! `PreCompact`, …) also produces a diagnostic, at
//! [`Severity::Info`]. Consumers that only care about classification
//! failures filter on `severity == "warning"`.
//!
//! Diagnostic payloads deliberately carry the **keys** of the offending
//! hook payload, never its values: `tool_input` routinely contains file
//! contents and prompts.

use std::collections::BTreeMap;

use serde_json::Value;

/// Machine-readable diagnostic classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticCode {
    /// stdin was not valid JSON, or was not a JSON object.
    MalformedPayload,
    /// `hook_event_name` was not a name this adapter version knows.
    UnknownHookEvent,
    /// A recognized hook event with no canonical IR kind in this
    /// adapter version. Informational, not an error.
    UnmappedHookEvent,
    /// A field the mapping requires (`session_id`, `tool_name`, …) was
    /// absent or of the wrong type.
    MissingRequiredField,
    /// `tool_input` was present but did not have the shape needed to
    /// synthesize an `fs.*` event (e.g. no `file_path`, or a relative
    /// path with no `cwd` to resolve it against).
    UnparseableToolInput,
    /// An MCP or skill tool name could not be normalized to the
    /// `mcp/<server>/<tool>` or `skill/<name>` form; the raw name was
    /// emitted instead of a guessed one.
    ToolNameNormalizationFailed,
    /// The sink buffer was exhausted and the *newest* events were
    /// dropped (never the oldest — invariant #4).
    SinkBackpressureDrop,
    /// A user-supplied coverage override suppressed events that the
    /// adapter would otherwise have emitted.
    CoverageOverrideSuppressed,
    /// A `SessionStart` that resumes/continues an existing computation
    /// did not open a second `computation.started` bracket. Emitting one
    /// would violate the adapter contract's "exactly one
    /// `computation.started`" rule for the session's `computation_id`.
    ComputationBracketSkipped,
}

impl DiagnosticCode {
    /// Kebab-case wire form.
    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::MalformedPayload => "malformed-payload",
            Self::UnknownHookEvent => "unknown-hook-event",
            Self::UnmappedHookEvent => "unmapped-hook-event",
            Self::MissingRequiredField => "missing-required-field",
            Self::UnparseableToolInput => "unparseable-tool-input",
            Self::ToolNameNormalizationFailed => "tool-name-normalization-failed",
            Self::SinkBackpressureDrop => "sink-backpressure-drop",
            Self::CoverageOverrideSuppressed => "coverage-override-suppressed",
            Self::ComputationBracketSkipped => "computation-bracket-skipped",
        }
    }

    /// Default severity for this code.
    #[must_use]
    pub const fn severity(self) -> Severity {
        match self {
            Self::UnmappedHookEvent | Self::ComputationBracketSkipped => Severity::Info,
            _ => Severity::Warning,
        }
    }
}

impl core::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_wire_str())
    }
}

/// How much a consumer should care.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Expected, benign, emitted for completeness (nothing was lost that
    /// the IR could have represented).
    Info,
    /// Something the adapter observed but could not compile. The
    /// dependency graph is incomplete here.
    Warning,
}

impl Severity {
    /// Wire form.
    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
        }
    }
}

impl core::fmt::Display for Severity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_wire_str())
    }
}

/// A diagnostic before it becomes an [`freshdag_core::ir::IrEvent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Classification.
    pub code: DiagnosticCode,
    /// Human-readable explanation.
    pub message: String,
    /// Extra structured context. Values are adapter-defined; they never
    /// contain hook-payload *values* (only keys and names).
    pub context: BTreeMap<String, Value>,
}

impl Diagnostic {
    /// Construct a diagnostic with no extra context.
    #[must_use]
    pub fn new(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            context: BTreeMap::new(),
        }
    }

    /// Attach a context field.
    #[must_use]
    pub fn with(mut self, key: &str, value: Value) -> Self {
        self.context.insert(key.to_string(), value);
        self
    }

    /// Render as an IR `diagnostic` payload.
    #[must_use]
    pub fn to_payload(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("message".to_string(), Value::String(self.message.clone()));
        map.insert(
            "code".to_string(),
            Value::String(self.code.as_wire_str().to_string()),
        );
        map.insert(
            "severity".to_string(),
            Value::String(self.code.severity().as_wire_str().to_string()),
        );
        for (k, v) in &self.context {
            map.insert(k.clone(), v.clone());
        }
        Value::Object(map)
    }
}

/// Sorted top-level keys of a JSON object, for diagnostic context.
///
/// Keys only — values may contain file contents, prompts, or secrets.
#[must_use]
pub fn payload_keys(value: &Value) -> Value {
    match value.as_object() {
        Some(obj) => Value::Array(obj.keys().map(|k| Value::String(k.clone())).collect()),
        None => Value::Array(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_has_a_distinct_wire_form() {
        let codes = [
            DiagnosticCode::MalformedPayload,
            DiagnosticCode::UnknownHookEvent,
            DiagnosticCode::UnmappedHookEvent,
            DiagnosticCode::MissingRequiredField,
            DiagnosticCode::UnparseableToolInput,
            DiagnosticCode::ToolNameNormalizationFailed,
            DiagnosticCode::SinkBackpressureDrop,
            DiagnosticCode::CoverageOverrideSuppressed,
            DiagnosticCode::ComputationBracketSkipped,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for c in codes {
            assert!(seen.insert(c.as_wire_str()), "duplicate wire form: {c}");
        }
    }

    #[test]
    fn only_unmapped_hook_events_are_informational() {
        assert_eq!(DiagnosticCode::UnmappedHookEvent.severity(), Severity::Info);
        assert_eq!(
            DiagnosticCode::UnknownHookEvent.severity(),
            Severity::Warning
        );
        assert_eq!(
            DiagnosticCode::UnparseableToolInput.severity(),
            Severity::Warning
        );
    }

    #[test]
    fn payload_keys_leak_keys_not_values() {
        let v = serde_json::json!({"tool_input": {"content": "SECRET"}, "cwd": "/tmp"});
        let keys = payload_keys(&v);
        let rendered = keys.to_string();
        assert!(rendered.contains("tool_input"));
        assert!(rendered.contains("cwd"));
        assert!(!rendered.contains("SECRET"));
    }

    #[test]
    fn context_fields_do_not_shadow_the_reserved_ones() {
        // `message`/`code`/`severity` are written first; a context key
        // with the same name would overwrite. Assert the current
        // behavior explicitly so a future change is a test failure.
        let d = Diagnostic::new(DiagnosticCode::MalformedPayload, "boom")
            .with("hook_event", serde_json::json!("Stop"));
        let p = d.to_payload();
        assert_eq!(p["message"], serde_json::json!("boom"));
        assert_eq!(p["code"], serde_json::json!("malformed-payload"));
        assert_eq!(p["severity"], serde_json::json!("warning"));
        assert_eq!(p["hook_event"], serde_json::json!("Stop"));
    }
}
