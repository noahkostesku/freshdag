//! The hook-payload → canonical-IR compile path.
//!
//! [`Compiler::compile_str`] is a **pure function** of
//! `(payload bytes, clock, id generator, adapter config)`. It never
//! reads the filesystem, never calls `OffsetDateTime::now_utc()` and
//! never calls `Uuid::now_v7()` — those come from the injected
//! [`Clock`] and [`IdGen`]. That is what makes the golden-file
//! conformance harness deterministic.
//!
//! It also never returns an empty stream. Every input produces at least
//! one event; unclassifiable input produces a `diagnostic`
//! (`docs/contracts/adapter-contract.md §Responsibilities #5`).

use std::path::{Path, PathBuf};

use freshdag_core::ir::{
    EventKind, FsRead, FsWrite, FsWriteMode, Hash, HashAlgo, IrEvent, ToolCompleted, ToolInvoked,
    ToolKind,
};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::config::{AdapterConfig, AGENT_KIND, PRODUCER, UNKNOWN_SESSION_ID};
use crate::content::{ContentSource, DiskContent, NoContent};
use crate::determinism::{Clock, IdGen, SystemClock, UuidV7Gen};
use crate::diagnostic::{payload_keys, Diagnostic, DiagnosticCode};
use crate::hook::{self, HookEvent};
use crate::identity;
use crate::paths;

/// Marker written onto every adapter-synthesized `fs.*` payload.
///
/// These events are derived from a `PreToolUse` payload — the tool's
/// *declared intent*, observed before it ran. A denied or failed tool
/// call therefore yields an `fs.*` event whose effect never happened.
/// Consumers must treat this marker as "unconfirmed"; the coverage
/// manifest says the same thing at producer granularity.
pub const OBSERVATION_PRE_EXECUTION_INTENT: &str = "pre-execution-intent";

/// Per-payload facts shared by every event compiled from it.
#[derive(Debug, Clone)]
struct HookCtx {
    session_id: String,
    computation_id: Option<String>,
    cwd: Option<PathBuf>,
}

/// Compiles Claude Code hook payloads into canonical IR events.
#[derive(Debug)]
pub struct Compiler<C: Clock, G: IdGen> {
    config: AdapterConfig,
    clock: C,
    ids: G,
    /// Where `fs.read` fingerprints come from. Defaults to
    /// [`NoContent`], so the compile path stays filesystem-free unless
    /// a caller deliberately installs a reader.
    content: Box<dyn ContentSource>,
}

impl Compiler<SystemClock, UuidV7Gen> {
    /// The production wiring: real wall clock, real `UUIDv7`s.
    #[must_use]
    pub fn production(config: AdapterConfig) -> Self {
        let max = config.max_hash_bytes;
        Self::new(config, SystemClock, UuidV7Gen).with_content(Box::new(DiskContent::new(max)))
    }
}

impl<C: Clock, G: IdGen> Compiler<C, G> {
    /// Construct with explicit time and identity sources.
    pub fn new(config: AdapterConfig, clock: C, ids: G) -> Self {
        Self {
            config,
            clock,
            ids,
            content: Box::new(NoContent),
        }
    }

    /// Install a fingerprint source for `fs.read`.
    ///
    /// Deliberately not the default: the conformance harness builds its
    /// compiler with [`Compiler::new`], and a golden that depended on
    /// files present on the running machine would be nondeterministic.
    #[must_use]
    pub fn with_content(mut self, content: Box<dyn ContentSource>) -> Self {
        self.content = content;
        self
    }

    /// The configuration in force.
    pub fn config(&self) -> &AdapterConfig {
        &self.config
    }

    // ---- entry points -------------------------------------------------

    /// Compile one hook payload, given as raw stdin bytes.
    ///
    /// Never returns an empty vector and never fails: malformed input
    /// becomes a `diagnostic`.
    pub fn compile_str(&mut self, input: &str) -> Vec<IrEvent> {
        match serde_json::from_str::<Value>(input) {
            Ok(value) => self.compile_value(&value),
            Err(err) => {
                let diag = Diagnostic::new(
                    DiagnosticCode::MalformedPayload,
                    format!("hook payload is not valid JSON: {err}"),
                );
                vec![self.orphan_diagnostic(&diag)]
            }
        }
    }

    /// Compile one already-parsed hook payload.
    pub fn compile_value(&mut self, payload: &Value) -> Vec<IrEvent> {
        let mut out = self.compile_inner(payload);
        self.apply_coverage_override(&mut out);
        out
    }

    fn compile_inner(&mut self, payload: &Value) -> Vec<IrEvent> {
        let Some(obj) = payload.as_object() else {
            let diag = Diagnostic::new(
                DiagnosticCode::MalformedPayload,
                "hook payload is valid JSON but not a JSON object",
            );
            return vec![self.orphan_diagnostic(&diag)];
        };

        let hook_name = str_field(obj, "hook_event_name");
        let session_id = str_field(obj, "session_id");

        let mut missing: Vec<&str> = Vec::new();
        if hook_name.is_none() {
            missing.push("hook_event_name");
        }
        if session_id.is_none() {
            missing.push("session_id");
        }
        if !missing.is_empty() {
            // Without a session we cannot attribute anything honestly,
            // and without a hook event name we cannot classify. Either
            // way: one diagnostic, no invented events.
            let diag = Diagnostic::new(
                DiagnosticCode::MissingRequiredField,
                format!(
                    "hook payload is missing required field(s): {}",
                    missing.join(", ")
                ),
            )
            .with("missing_fields", json!(missing))
            .with("payload_keys", payload_keys(payload));
            let diag = match hook_name {
                Some(h) => diag.with("hook_event", json!(h)),
                None => diag,
            };
            return vec![self.orphan_diagnostic(&diag)];
        }

        let hook_name = hook_name.expect("checked above");
        let session_id = session_id.expect("checked above");
        let ctx = self.context_for(session_id, obj);

        let Some(event) = HookEvent::parse(hook_name) else {
            let diag = Diagnostic::new(
                DiagnosticCode::UnknownHookEvent,
                format!(
                    "`{hook_name}` is not a hook event this adapter version recognizes; \
                     the payload was recorded as a diagnostic rather than dropped"
                ),
            )
            .with("hook_event", json!(hook_name))
            .with("payload_keys", payload_keys(payload));
            return vec![self.diagnostic(&ctx, &diag, None)];
        };

        match event {
            HookEvent::PreToolUse => self.compile_pre_tool_use(&ctx, obj, payload),
            HookEvent::PostToolUse => self.compile_post_tool_use(&ctx, obj, payload),
            HookEvent::SessionStart => self.compile_session_start(&ctx, obj),
            HookEvent::SessionEnd => self.compile_session_end(&ctx, obj),
            unmapped => vec![self.unmapped_hook_event(&ctx, unmapped)],
        }
    }

    fn context_for(&self, session_id: &str, obj: &Map<String, Value>) -> HookCtx {
        let computation_id =
            identity::computation_id_for_session(session_id, &self.config.identity_rule_version).0;
        HookCtx {
            session_id: session_id.to_string(),
            computation_id: Some(computation_id),
            cwd: str_field(obj, "cwd").map(PathBuf::from),
        }
    }

    // ---- hook event handlers ------------------------------------------

    fn compile_pre_tool_use(
        &mut self,
        ctx: &HookCtx,
        obj: &Map<String, Value>,
        payload: &Value,
    ) -> Vec<IrEvent> {
        let Some(raw_tool_name) = str_field(obj, "tool_name") else {
            let diag = Diagnostic::new(
                DiagnosticCode::MissingRequiredField,
                "PreToolUse payload has no `tool_name`; no tool.invoked was synthesized",
            )
            .with("hook_event", json!(HookEvent::PreToolUse.as_str()))
            .with("payload_keys", payload_keys(payload));
            return vec![self.diagnostic(ctx, &diag, None)];
        };

        let mut out = Vec::new();
        let tool_input = obj.get("tool_input").cloned();
        let mut pending: Vec<Diagnostic> = Vec::new();
        if tool_input.is_none() {
            pending.push(
                Diagnostic::new(
                    DiagnosticCode::MissingRequiredField,
                    "PreToolUse payload has no `tool_input`; tool.invoked carries a null input",
                )
                .with("tool_name", json!(raw_tool_name)),
            );
        }
        let tool_input = tool_input.unwrap_or(Value::Null);

        let classified = classify_tool(raw_tool_name, &tool_input);
        pending.extend(classified.diagnostics);

        let invoked = ToolInvoked {
            tool_name: classified.tool_name,
            tool_kind: classified.tool_kind,
            tool_input: tool_input.clone(),
            cwd: ctx.cwd.clone(),
        };
        let invoked_event = self.event(ctx, EventKind::ToolInvoked, to_payload(&invoked), None);
        let invoked_id = invoked_event.event_id;
        out.push(invoked_event);

        // Filesystem synthesis keys off the RUNTIME tool name, not the
        // normalized one: `mcp/.../read` is not Claude Code's `Read`.
        let (fs_payload, fs_diags) = synthesize_fs(
            raw_tool_name,
            &tool_input,
            ctx.cwd.as_deref(),
            self.content.as_ref(),
        );
        pending.extend(fs_diags);
        if let Some((kind, payload_value)) = fs_payload {
            out.push(self.event(ctx, kind, payload_value, Some(invoked_id)));
        }

        for diag in &pending {
            out.push(self.diagnostic(ctx, diag, Some(invoked_id)));
        }
        out
    }

    fn compile_post_tool_use(
        &mut self,
        ctx: &HookCtx,
        obj: &Map<String, Value>,
        payload: &Value,
    ) -> Vec<IrEvent> {
        let Some(raw_tool_name) = str_field(obj, "tool_name") else {
            let diag = Diagnostic::new(
                DiagnosticCode::MissingRequiredField,
                "PostToolUse payload has no `tool_name`; no tool.completed was synthesized",
            )
            .with("hook_event", json!(HookEvent::PostToolUse.as_str()))
            .with("payload_keys", payload_keys(payload));
            return vec![self.diagnostic(ctx, &diag, None)];
        };

        let mut pending: Vec<Diagnostic> = Vec::new();
        let tool_response = obj.get("tool_response").cloned().unwrap_or_else(|| {
            pending.push(
                Diagnostic::new(
                    DiagnosticCode::MissingRequiredField,
                    "PostToolUse payload has no `tool_response`; tool.completed carries a \
                     null output",
                )
                .with("tool_name", json!(raw_tool_name)),
            );
            Value::Null
        });

        let classified = classify_tool(raw_tool_name, &Value::Null);
        // Name-normalization diagnostics were already emitted on the
        // matching PreToolUse; re-emitting them here would double-count.
        let completed = ToolCompleted {
            tool_output: tool_response.clone(),
            is_error: derive_is_error(&tool_response),
            duration_ms: 0,
        };
        let mut value = to_payload(&completed);
        insert(&mut value, "duration_observed", json!(false));
        insert(&mut value, "tool_name", json!(classified.tool_name));

        let mut out = vec![self.event(ctx, EventKind::ToolCompleted, value, None)];
        for diag in &pending {
            let id = out[0].event_id;
            out.push(self.diagnostic(ctx, diag, Some(id)));
        }
        out
    }

    fn compile_session_start(&mut self, ctx: &HookCtx, obj: &Map<String, Value>) -> Vec<IrEvent> {
        let source = str_field(obj, "source");
        let mut started = json!({ "agent_kind": AGENT_KIND });
        if let Some(cwd) = &ctx.cwd {
            insert(&mut started, "cwd", json!(cwd));
        }
        if let Some(source) = source {
            insert(&mut started, "source", json!(source));
        }
        let session_event = self.event(ctx, EventKind::SessionStarted, started, None);
        let session_event_id = session_event.event_id;
        let mut out = vec![session_event];

        // `computation.started` must appear exactly once per
        // `computation_id` (adapter contract §Responsibilities #2). A
        // session that resumes, clears or compacts fires `SessionStart`
        // again for a session_id we have already bracketed, so only a
        // genuine `startup` opens the bracket.
        if source == Some("startup") {
            let payload = json!({
                "recipe_id": identity::synthesized_recipe_id(&ctx.session_id),
                "inputs_declared": Vec::<String>::new(),
                "identity_rule": self.config.identity_rule_version,
            });
            out.push(self.event(
                ctx,
                EventKind::ComputationStarted,
                payload,
                Some(session_event_id),
            ));
        } else {
            let diag = Diagnostic::new(
                DiagnosticCode::ComputationBracketSkipped,
                format!(
                    "SessionStart source is {}; this session_id's computation.started bracket \
                     was opened by an earlier `startup` and is not reopened",
                    source.map_or_else(|| "absent".to_string(), |s| format!("`{s}`"))
                ),
            )
            .with("source", source.map_or(Value::Null, |s| json!(s)))
            .with("identity_rule", json!(self.config.identity_rule_version));
            out.push(self.diagnostic(ctx, &diag, Some(session_event_id)));
        }
        out
    }

    fn compile_session_end(&mut self, ctx: &HookCtx, obj: &Map<String, Value>) -> Vec<IrEvent> {
        let reason = str_field(obj, "reason");
        // Unknown never becomes the optimistic value (invariant #7):
        // only a recognized clean-exit reason yields `ok`.
        let status = match reason {
            Some("clear" | "logout" | "prompt_input_exit" | "exit") => "ok",
            _ => "aborted",
        };
        let mut ended = json!({ "status": status });
        if let Some(reason) = reason {
            insert(&mut ended, "reason", json!(reason));
        }
        let comp = self.event(ctx, EventKind::ComputationEnded, ended, None);
        let comp_id = comp.event_id;

        let mut session_payload = json!({});
        if let Some(reason) = reason {
            insert(&mut session_payload, "reason", json!(reason));
        }
        vec![
            comp,
            self.event(ctx, EventKind::SessionEnded, session_payload, Some(comp_id)),
        ]
    }

    fn unmapped_hook_event(&mut self, ctx: &HookCtx, event: HookEvent) -> IrEvent {
        let diag = Diagnostic::new(
            DiagnosticCode::UnmappedHookEvent,
            format!(
                "hook event `{event}` is recognized but has no canonical IR kind in \
                 execution-ir v0.1; recorded as a diagnostic rather than dropped"
            ),
        )
        .with("hook_event", json!(event.as_str()));
        self.diagnostic(ctx, &diag, None)
    }

    // ---- coverage override --------------------------------------------

    fn apply_coverage_override(&mut self, events: &mut Vec<IrEvent>) {
        if self.config.suppressed_kinds.is_empty() {
            return;
        }
        let mut suppressed: Vec<&'static str> = Vec::new();
        let mut ctx: Option<HookCtx> = None;
        events.retain(|ev| {
            // `diagnostic` is never suppressible: suppressing the record
            // of suppression would be a silent drop.
            if matches!(ev.kind, EventKind::Diagnostic) {
                return true;
            }
            let hit = self
                .config
                .suppressed_kinds
                .iter()
                .any(|p| p.matches(ev.kind));
            if hit {
                suppressed.push(ev.kind.as_wire_str());
                ctx.get_or_insert_with(|| HookCtx {
                    session_id: ev.session_id.clone(),
                    computation_id: ev.computation_id.clone(),
                    cwd: None,
                });
            }
            !hit
        });
        if suppressed.is_empty() {
            return;
        }
        suppressed.sort_unstable();
        let count = suppressed.len();
        suppressed.dedup();
        let diag = Diagnostic::new(
            DiagnosticCode::CoverageOverrideSuppressed,
            format!("coverage override withheld {count} event(s) from this hook invocation"),
        )
        .with("suppressed_kinds", json!(suppressed))
        .with("suppressed_count", json!(count));
        let ctx = ctx.expect("suppressed is non-empty, so a context was captured");
        let ev = self.diagnostic(&ctx, &diag, None);
        events.push(ev);
    }

    // ---- envelope construction ----------------------------------------

    fn event(
        &mut self,
        ctx: &HookCtx,
        kind: EventKind,
        payload: Value,
        causal: Option<Uuid>,
    ) -> IrEvent {
        IrEvent {
            event_id: self.ids.next_id(),
            producer: PRODUCER.to_string(),
            producer_version: self.config.producer_version.clone(),
            session_id: ctx.session_id.clone(),
            computation_id: ctx.computation_id.clone(),
            parent_id: causal,
            causal_inputs: causal.map(|id| vec![id]),
            ts: self.clock.now(),
            kind,
            payload,
        }
    }

    fn diagnostic(&mut self, ctx: &HookCtx, diag: &Diagnostic, causal: Option<Uuid>) -> IrEvent {
        self.event(ctx, EventKind::Diagnostic, diag.to_payload(), causal)
    }

    /// A diagnostic for a payload so broken we could not even establish
    /// a session. `session_id` is the [`UNKNOWN_SESSION_ID`] sentinel
    /// and `computation_id` is absent — we refuse to attribute the event
    /// to a computation we cannot identify.
    fn orphan_diagnostic(&mut self, diag: &Diagnostic) -> IrEvent {
        let ctx = HookCtx {
            session_id: UNKNOWN_SESSION_ID.to_string(),
            computation_id: None,
            cwd: None,
        };
        self.diagnostic(&ctx, diag, None)
    }

    /// Build a standalone `diagnostic` event outside the compile path —
    /// used by the hook binary to record sink back-pressure drops.
    pub fn standalone_diagnostic(&mut self, session_id: &str, diag: &Diagnostic) -> IrEvent {
        let computation_id = if session_id == UNKNOWN_SESSION_ID {
            None
        } else {
            Some(
                identity::computation_id_for_session(
                    session_id,
                    &self.config.identity_rule_version,
                )
                .0,
            )
        };
        let ctx = HookCtx {
            session_id: session_id.to_string(),
            computation_id,
            cwd: None,
        };
        self.diagnostic(&ctx, diag, None)
    }
}

// ---- tool classification ----------------------------------------------

/// The normalized identity of a tool call.
#[derive(Debug, Clone)]
struct ClassifiedTool {
    tool_name: String,
    tool_kind: ToolKind,
    diagnostics: Vec<Diagnostic>,
}

/// Normalize a Claude Code tool name per
/// `docs/contracts/execution-ir.md §Tool interaction`.
fn classify_tool(raw: &str, tool_input: &Value) -> ClassifiedTool {
    if raw.starts_with(hook::MCP_TOOL_PREFIX) {
        return classify_mcp(raw);
    }
    match raw {
        hook::BASH_TOOL_NAME => ClassifiedTool {
            tool_name: "bash".to_string(),
            tool_kind: ToolKind::Bash,
            diagnostics: Vec::new(),
        },
        hook::TASK_TOOL_NAME => ClassifiedTool {
            tool_name: "task".to_string(),
            tool_kind: ToolKind::Task,
            diagnostics: Vec::new(),
        },
        hook::SKILL_TOOL_NAME => classify_skill(raw, tool_input),
        other => ClassifiedTool {
            tool_name: other.to_string(),
            tool_kind: ToolKind::Builtin,
            diagnostics: Vec::new(),
        },
    }
}

fn classify_mcp(raw: &str) -> ClassifiedTool {
    match hook::split_mcp_tool_name(raw) {
        Some((server, tool)) => ClassifiedTool {
            tool_name: format!("mcp/{server}/{tool}"),
            tool_kind: ToolKind::Mcp,
            diagnostics: Vec::new(),
        },
        None => ClassifiedTool {
            tool_name: raw.to_string(),
            tool_kind: ToolKind::Mcp,
            diagnostics: vec![Diagnostic::new(
                DiagnosticCode::ToolNameNormalizationFailed,
                format!(
                    "MCP tool name `{raw}` does not match `mcp__<server>__<tool>`; \
                     emitted the raw name rather than guessing a server/tool split"
                ),
            )
            .with("tool_name", json!(raw))],
        },
    }
}

fn classify_skill(raw: &str, tool_input: &Value) -> ClassifiedTool {
    match hook::skill_name_from_input(tool_input) {
        Some(name) => ClassifiedTool {
            tool_name: format!("skill/{name}"),
            tool_kind: ToolKind::Skill,
            diagnostics: Vec::new(),
        },
        None => ClassifiedTool {
            tool_name: raw.to_string(),
            tool_kind: ToolKind::Skill,
            diagnostics: vec![Diagnostic::new(
                DiagnosticCode::ToolNameNormalizationFailed,
                "Skill invocation carried no recognizable skill name in `tool_input`; \
                 emitted the raw tool name rather than inventing one",
            )
            .with("tool_name", json!(raw))],
        },
    }
}

// ---- filesystem synthesis ---------------------------------------------

/// Synthesize the `fs.*` event implied by a `PreToolUse` payload.
///
/// Returns `None` for tools whose filesystem effects the adapter cannot
/// see. That emphatically includes `Bash` and `Task`: their I/O happens
/// inside a subprocess or a subagent, is invisible from the hook
/// payload, and is observer territory. Emitting nothing here is what
/// lets `Certificate::check_coverage_deficit` force a non-`valid`
/// status when no fs-covering observer is present.
fn synthesize_fs(
    raw_tool_name: &str,
    tool_input: &Value,
    cwd: Option<&Path>,
    content: &dyn ContentSource,
) -> (Option<(EventKind, Value)>, Vec<Diagnostic>) {
    let is_read = hook::READ_TOOLS.contains(&raw_tool_name);
    let is_write = hook::WRITE_TOOLS.contains(&raw_tool_name);
    if !is_read && !is_write {
        return (None, Vec::new());
    }

    let key = hook::file_path_key(raw_tool_name);
    let Some(raw_path) = tool_input.get(key).and_then(Value::as_str) else {
        let diag = Diagnostic::new(
            DiagnosticCode::UnparseableToolInput,
            format!(
                "`{raw_tool_name}` tool input has no string `{key}`; no fs event could be \
                 synthesized for this invocation"
            ),
        )
        .with("tool_name", json!(raw_tool_name))
        .with("expected_key", json!(key))
        .with("tool_input_keys", payload_keys(tool_input));
        return (None, vec![diag]);
    };

    let resolved = match paths::resolve(raw_path, cwd) {
        Ok(r) => r,
        Err(err) => {
            let diag = Diagnostic::new(
                DiagnosticCode::UnparseableToolInput,
                format!("`{raw_tool_name}` tool input path could not be canonicalized: {err}"),
            )
            .with("tool_name", json!(raw_tool_name))
            .with("expected_key", json!(key));
            return (None, vec![diag]);
        }
    };

    if is_read {
        (Some(synthesize_read(&resolved, content)), Vec::new())
    } else {
        let (event, diags) = synthesize_write(raw_tool_name, tool_input, &resolved);
        (Some(event), diags)
    }
}

fn synthesize_read(
    resolved: &paths::ResolvedPath,
    content: &dyn ContentSource,
) -> (EventKind, Value) {
    // The hook payload carries only a path, so the fingerprint comes
    // from the injected content source — `None` under the conformance
    // harness, the real file in production. A read with no fingerprint
    // is dropped by the store as `NoFingerprint`, so this is what
    // decides whether the adapter produces usable dependencies at all.
    let fingerprint = content.fingerprint(&resolved.path);
    let (size, hash) = match fingerprint {
        Some((hash, size)) => (size, Some(hash)),
        None => (0, None),
    };
    let observed = hash.is_some();

    let read = FsRead {
        path: resolved.path.clone(),
        size,
        hash,
        follow_symlink_target: None,
        raw_path: resolved.raw_path.clone(),
        read_kind: freshdag_core::ir::FsReadKind::Direct,
        impure: false,
    };
    let mut value = to_payload(&read);
    insert(&mut value, "size_observed", json!(observed));
    insert(
        &mut value,
        "observation",
        json!(OBSERVATION_PRE_EXECUTION_INTENT),
    );
    (EventKind::FsRead, value)
}

fn synthesize_write(
    raw_tool_name: &str,
    tool_input: &Value,
    resolved: &paths::ResolvedPath,
) -> ((EventKind, Value), Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    // `Write` carries the full post-write contents, so size and hash are
    // genuinely derivable from the payload. `Edit`/`MultiEdit`/
    // `NotebookEdit` carry only a splice, so they are not.
    let content = if raw_tool_name == "Write" {
        let c = tool_input.get("content").and_then(Value::as_str);
        if c.is_none() {
            diagnostics.push(
                Diagnostic::new(
                    DiagnosticCode::UnparseableToolInput,
                    "`Write` tool input has no string `content`; fs.write carries no size \
                     or hash",
                )
                .with("tool_name", json!(raw_tool_name)),
            );
        }
        c
    } else {
        None
    };

    let (size, hash) = match content {
        Some(c) => (c.len() as u64, Some(blake3_hash(c.as_bytes()))),
        None => (0, None),
    };

    let write = FsWrite {
        path: resolved.path.clone(),
        size,
        hash,
        // Hook payloads do not reveal whether the target already
        // existed, so create-vs-truncate is not distinguishable;
        // `truncate` describes the observable effect of both.
        mode: FsWriteMode::Truncate,
        raw_path: resolved.raw_path.clone(),
    };
    let mut value = to_payload(&write);
    if content.is_none() {
        insert(&mut value, "size_observed", json!(false));
    }
    insert(
        &mut value,
        "observation",
        json!(OBSERVATION_PRE_EXECUTION_INTENT),
    );
    ((EventKind::FsWrite, value), diagnostics)
}

fn blake3_hash(bytes: &[u8]) -> Hash {
    let digest = blake3::hash(bytes).to_hex().to_string();
    Hash::new(HashAlgo::Blake3, digest).expect("blake3 hex is 64 lowercase hex chars")
}

// ---- small helpers ----------------------------------------------------

fn str_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

fn to_payload<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("IR payload types serialize infallibly")
}

fn insert(target: &mut Value, key: &str, value: Value) {
    if let Some(obj) = target.as_object_mut() {
        obj.insert(key.to_string(), value);
    }
}

/// Best-effort transport-level error detection from a `tool_response`.
///
/// Claude Code does not expose a uniform error flag, so this reads the
/// spellings that exist and defaults to `false`. The coverage manifest's
/// `partial` note for `tool.completed` records that `is_error: false`
/// means "no error signal in the payload", not "the tool succeeded".
fn derive_is_error(tool_response: &Value) -> bool {
    let Some(obj) = tool_response.as_object() else {
        return false;
    };
    if obj.get("success").and_then(Value::as_bool) == Some(false) {
        return true;
    }
    if obj.get("is_error").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    obj.get("error").is_some_and(|v| !v.is_null())
}
