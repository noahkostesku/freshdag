//! This adapter's coverage manifest.
//!
//! `docs/contracts/execution-ir.md §Coverage Declarations`: *"Consumers
//! use this manifest to know what 'no event' means: silent because the
//! producer does not cover this signal is different from silent because
//! nothing happened."*
//!
//! The Rust constructor here and `coverage.json` at the crate root are
//! two spellings of one fact, and a test asserts they agree. A drifting
//! manifest is precisely the failure the certificate contract's
//! coverage-deficit rule cannot tolerate.

use std::collections::BTreeMap;

use freshdag_core::ir::{
    CoverageManifest, EventKindPattern, PartialCoverage, PartialReason, ProducerRole,
};
use serde_json::{json, Value};

use crate::config::{AdapterConfig, PRODUCER};
use crate::hook::HookEvent;
use crate::identity::SESSION_AS_COMPUTATION_V1;

/// The manifest as this build of the adapter behaves.
#[must_use]
pub fn coverage_manifest() -> CoverageManifest {
    manifest_with_version(env!("CARGO_PKG_VERSION"), SESSION_AS_COMPUTATION_V1)
}

/// The manifest for a given adapter configuration.
///
/// **Suppression narrows the manifest.** `AdapterConfig::suppressed_kinds`
/// stops events reaching the sink, so a manifest that still declared
/// those kinds would tell a consumer "I cover this" about a signal the
/// consumer will never see — and silence under a covered kind reads as
/// `ObservedAbsent`, not `Unobserved`. That is the fail-*open* direction:
/// it would let a suppressed adapter's silence look like evidence that
/// nothing happened.
///
/// The narrowing is deliberately **coarse**: any declared pattern that
/// *intersects* a suppression pattern is dropped whole, even when the
/// suppression covers only part of it. Dropping too much under-claims
/// coverage, which fails safe — an unclaimed kind's silence is
/// `Unobserved`, which caps the artifact at `unknown` rather than
/// licensing `valid`.
#[must_use]
pub fn coverage_manifest_for(config: &AdapterConfig) -> CoverageManifest {
    let mut manifest =
        manifest_with_version(&config.producer_version, &config.identity_rule_version);
    if config.suppressed_kinds.is_empty() {
        return manifest;
    }

    manifest.emits.retain(|declared| {
        !config
            .suppressed_kinds
            .iter()
            .any(|s| patterns_intersect(declared, s))
    });
    // A partial-coverage note about a kind this build no longer emits
    // describes nothing. Drop those with the kinds they annotate.
    manifest.partial.retain(|kind, _| {
        let declared = EventKindPattern::new(kind.clone());
        !config
            .suppressed_kinds
            .iter()
            .any(|s| patterns_intersect(&declared, s))
    });
    manifest
}

/// Could these two patterns ever match the same concrete event kind?
///
/// Both forms are either an exact wire string or a `prefix.*` glob, so
/// this is decidable without enumerating `EventKind` — which matters
/// because enumerating it would mean adding an `ALL` to a core IR enum,
/// and those are contract-change surface.
fn patterns_intersect(a: &EventKindPattern, b: &EventKindPattern) -> bool {
    match (a.as_str().strip_suffix(".*"), b.as_str().strip_suffix(".*")) {
        // Two globs overlap when either prefix contains the other.
        (Some(pa), Some(pb)) => pa.starts_with(pb) || pb.starts_with(pa),
        // A glob and an exact kind overlap when the kind is under it.
        (Some(pa), None) => b.as_str().starts_with(pa) && b.as_str()[pa.len()..].starts_with('.'),
        (None, Some(pb)) => a.as_str().starts_with(pb) && a.as_str()[pb.len()..].starts_with('.'),
        // Two exact kinds overlap only when equal.
        (None, None) => a.as_str() == b.as_str(),
    }
}

fn manifest_with_version(version: &str, identity_rule: &str) -> CoverageManifest {
    // `role` is what stops this adapter's `fs.read`/`fs.write`
    // declaration from discharging the observation obligation that
    // `Certificate::check_coverage_deficit` exists to enforce. This
    // adapter genuinely synthesizes fs events from Read/Write/Edit tool
    // inputs, but it is blind inside `bash` and `task` subprocesses, so
    // only a `ProducerRole::Observer` may discharge that obligation —
    // see `known_limitations()` entry 2 and certificate-contract
    // §Coverage-Deficit Rule.
    CoverageManifest {
        role: ProducerRole::Adapter,
        producer: PRODUCER.to_string(),
        version: version.to_string(),
        platforms: vec!["any".to_string()],
        emits: vec![
            EventKindPattern::from("session.*"),
            EventKindPattern::from("computation.*"),
            EventKindPattern::from("tool.*"),
            EventKindPattern::from("fs.read"),
            EventKindPattern::from("fs.write"),
            EventKindPattern::from("diagnostic"),
        ],
        partial: partial_notes(),
        capabilities: capabilities(identity_rule),
        known_limitations: known_limitations(),
    }
}

/// Every entry's `reason` is read off that entry's own note — what this
/// adapter does and does not emit — and not off ADR 0011, which has no
/// standing to classify a producer (ADR 0011, Amendment, Correction 1).
///
/// Where a note describes error in *both* directions, the reason is the
/// fail-unsafe one. `fs.read` both over-reports (a denied Read still
/// emits) and under-reports (reads by any other means are invisible);
/// the vocabulary asks whether real events can be missed, and here they
/// can, so it is `under-approximates`. Over-reporting costs staleness,
/// which invariant #15 prefers; a missed read costs a spurious `valid`,
/// which invariant #7 forbids. A manifest may not net the two out.
///
/// None of these classifications changes this adapter's behaviour
/// today: `role: Adapter` already bars it from discharging an
/// observation obligation, whatever its `partial` map says. They matter
/// because the manifest reaches the certificate, where a third-party
/// rechecker reads the direction rather than the prose.
fn partial_notes() -> BTreeMap<String, PartialCoverage> {
    let mut m = BTreeMap::new();
    // "Reads performed by any other means are invisible here."
    m.insert(
        "fs.read".to_string(),
        PartialCoverage::new(
            PartialReason::UnderApproximates,
            "synthesized ONLY from the `Read` tool's `file_path` input on PreToolUse. \
             Pre-execution intent, not a confirmed effect: a denied or failed Read still \
             produces this event. `size`/`hash` are taken by reading the file at hook time, \
             so they describe it as it stood just BEFORE the tool ran, not what the tool \
             received; both are absent (`size_observed: false`) when the file is missing, \
             unreadable, or above the adapter's inline-hash byte cap. Reads performed by any \
             other means are invisible here.",
        ),
    );
    // "synthesized ONLY from Write/Edit/MultiEdit/NotebookEdit inputs" —
    // a write by any other route is not emitted.
    m.insert(
        "fs.write".to_string(),
        PartialCoverage::new(
            PartialReason::UnderApproximates,
            "synthesized ONLY from `Write`/`Edit`/`MultiEdit`/`NotebookEdit` `file_path` \
             (`notebook_path` for NotebookEdit) inputs on PreToolUse. Pre-execution intent, \
             not a confirmed effect. `mode` is always `truncate` because hook payloads do not \
             reveal prior existence. `size`/`hash` are real only for `Write` (whose input \
             carries the full contents); the edit tools carry `size_observed: false` and no \
             hash.",
        ),
    );
    // Structural blindness confined to a scope — subprocesses — which is
    // exactly `blind-in-scope`. This is the broad admission that ADR
    // 0011's Correction 4 uses as its worked example: it must not be
    // annotated away by the narrower `fs.read`/`fs.write` entries above.
    m.insert(
        "fs.*".to_string(),
        PartialCoverage::new(
            PartialReason::BlindInScope,
            "FILESYSTEM EFFECTS INSIDE `bash` AND `task` INVOCATIONS ARE INVISIBLE TO THIS \
             ADAPTER AND ARE OBSERVER TERRITORY. A hook payload exposes a Bash command line \
             and a Task prompt, never the syscalls they perform. This adapter emits NO fs \
             events for tool_kind `bash` or `task`, which is what lets \
             `Certificate::check_coverage_deficit` force a non-`valid` status when no \
             fs-covering observer is present.",
        ),
    );
    // Real completions go unemitted, so this is `under-approximates`.
    //
    // It was briefly `over-approximates` — the only discharging reason
    // in this manifest — on the claim that every event is still emitted,
    // merely coarser. That claim is false. `tool.completed` is emitted
    // from exactly one site, reached only from `HookEvent::PostToolUse`,
    // and `PostToolUse` fires only when a tool *succeeds*. Failures
    // arrive as `PostToolUseFailure`, which `HookEvent::parse` does not
    // recognize, so every failed tool call in this adapter's stream is a
    // `tool.invoked` with no `tool.completed`. That is the error path,
    // not a corner case. Denied calls, a `PostToolUse` carrying no
    // `tool_name`, and hook timeouts lose it too.
    //
    // Nothing compensates in the other direction: `PostToolUse` cannot
    // fire for a completion that did not happen. The degraded fields the
    // note describes are an under-report of detail, and the absent
    // `causal_inputs` is a missing edge — under-approximation in the
    // most literal sense. The entry is not mixed-direction at all, so
    // the fail-unsafe tie-break above is not even needed to decide it.
    m.insert(
        "tool.completed".to_string(),
        PartialCoverage::new(
            PartialReason::UnderApproximates,
            "emitted ONLY from `PostToolUse`, which fires only when a tool succeeds. A failed \
             tool call arrives as `PostToolUseFailure`, which this adapter does not yet \
             recognize, so it produces a `tool.invoked` with NO matching `tool.completed`; \
             denied calls, a `PostToolUse` carrying no `tool_name`, and hook timeouts lose it \
             the same way. Where the event IS emitted its fields are degraded: `duration_ms` \
             is always 0 with `duration_observed: false` because hook payloads carry no \
             timing, `is_error: false` means `no error signal was present in tool_response` \
             rather than `the tool succeeded`, and no `causal_inputs` links it to its \
             `tool.invoked` — each hook fires in its own process and this adapter keeps no \
             cross-invocation state, though the normalized `tool_name` is on the payload so a \
             consumer can correlate.",
        ),
    );
    // A resumed, cleared or compacted session emits no `computation.started`
    // at all — a missing event, not a coarse one. The bracket is also
    // asymmetric: `compile_session_end` emits `computation.ended`
    // unconditionally while `compile_session_start` guards on
    // `source == "startup"`, so an unmatched `computation.ended` is
    // reachable. That errs in the other direction and is declared rather
    // than netted out.
    m.insert(
        "computation.*".to_string(),
        PartialCoverage::new(
            PartialReason::UnderApproximates,
            "one Claude Code session is one computation. `computation.started` is emitted \
             only on `SessionStart` with `source: startup`; resume/clear/compact starts emit \
             a `computation-bracket-skipped` diagnostic instead of reopening the bracket, and \
             a hook installed mid-session never produces one at all. The bracket is \
             ASYMMETRIC: `computation.ended` is emitted on every `SessionEnd` with no matching \
             guard, so a resumed session can yield a `computation.ended` with no \
             `computation.started` — adapter-contract §Responsibilities #2 asks for exactly \
             one of each. `Task` subagents are NOT sliced into sub-computations under this \
             identity rule.",
        ),
    );
    m
}

fn capabilities(identity_rule: &str) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert("identity_rule".to_string(), json!(identity_rule));
    m.insert(
        "identity_rule_description".to_string(),
        json!(
            "one Claude Code session = one computation; \
             recipe_id_or_hash = \"claude-code-session:<session_id>\", \
             canonicalized_declared_inputs = \"\", \
             adapter_identity_rule_version = the identity_rule string"
        ),
    );
    m.insert(
        "computation_id_derivation".to_string(),
        json!("freshdag_core::computation::ComputationId::derive"),
    );
    m.insert(
        "hook_events_supported".to_string(),
        json!(HookEvent::ALL.map(HookEvent::as_str)),
    );
    m.insert(
        "hook_events_with_ir_mapping".to_string(),
        json!(HookEvent::ALL
            .iter()
            .filter(|e| e.has_ir_mapping())
            .map(|e| e.as_str())
            .collect::<Vec<_>>()),
    );
    m.insert("hook_matcher".to_string(), json!(".*"));
    m.insert("transcript_tailing".to_string(), json!(false));
    m.insert(
        "fs_coverage_scope".to_string(),
        json!("tool-input-derived-only"),
    );
    m.insert("fs_events_are_confirmed_effects".to_string(), json!(false));
    m.insert("symlink_resolution".to_string(), json!(false));
    m.insert("path_canonicalization".to_string(), json!("lexical-only"));
    m.insert("proc.spawn".to_string(), json!(false));
    m.insert("net.connect".to_string(), json!(false));
    m.insert("net.fetch".to_string(), json!(false));
    m
}

fn known_limitations() -> Vec<String> {
    vec![
        "BASH/TASK BLINDNESS: filesystem, process and network effects inside `Bash` \
         subprocesses and `Task` subagents are invisible to this adapter. It emits \
         tool.invoked with tool_kind `bash`/`task` and NO fs events, so the certificate \
         contract's coverage-deficit rule can see the gap."
            .to_string(),
        "THIS ADAPTER IS NOT AN OBSERVER. Its `fs.read`/`fs.write` coverage is derived \
         from tool inputs only. The engine MUST NOT treat this producer as satisfying the \
         fs-coverage requirement of `Certificate::check_coverage_deficit`; a systems \
         observer is still required before a computation that invoked bash/task may be \
         `valid`."
            .to_string(),
        "COMPUTATION IDENTITY GAP: Claude Code exposes no recipe, so \
         `recipe_id_or_hash` is synthesized as `claude-code-session:<session_id>` per the \
         execution-IR contract's fallback clause. `computation.started` carries no \
         `inputs_declared`, and `Computation::recipe_hash` cannot be populated from hook \
         payloads — which by invariant #9 caps such computations below `valid`."
            .to_string(),
        "fs.* events are PRE-EXECUTION INTENT (`observation: pre-execution-intent`), \
         derived from PreToolUse. A tool call denied by permissions or failing at runtime \
         still produces one."
            .to_string(),
        "NO TRANSCRIPT TAILING: `transcript_path` is not read, so subagent parenthood is \
         not reconstructed and `tool.completed` carries no `causal_inputs` to its \
         `tool.invoked`. Causal links exist only within a single hook invocation."
            .to_string(),
        "`tool.completed.duration_ms` is always 0 (`duration_observed: false`); hook \
         payloads carry no timing."
            .to_string(),
        "Paths are canonicalized LEXICALLY against the payload `cwd`. Symlinks are not \
         resolved and `follow_symlink_target` is always absent."
            .to_string(),
        "`UserPromptSubmit`, `Stop`, `SubagentStop`, `PreCompact` and `Notification` are \
         recognized but have no canonical IR kind in execution-ir v0.1; each produces an \
         info-severity `unmapped-hook-event` diagnostic rather than a silent drop."
            .to_string(),
        "The coverage override is a list of event-kind patterns supplied on the command \
         line or via the environment, not the override *file* named by the adapter \
         contract's §Configuration."
            .to_string(),
        "FAILED AND DENIED TOOL CALLS PRODUCE NO `tool.completed`. `PostToolUseFailure` and \
         `PermissionDenied` are not recognized by `HookEvent::parse`; they fall to an \
         info-severity `unknown-hook-event` diagnostic. Nothing is silently dropped, but the \
         completion event itself is absent, which is why `tool.completed` declares \
         `under-approximates`. Mapping them is follow-up work."
            .to_string(),
    ]
}
