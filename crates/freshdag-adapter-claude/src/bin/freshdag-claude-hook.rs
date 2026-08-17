//! `freshdag-claude-hook` — the Claude Code hook binary.
//!
//! Claude Code's hook system runs a fresh process per event and feeds it
//! one JSON payload on stdin. This binary reads that payload, compiles
//! it to canonical IR, and appends the events to a JSONL sink.
//!
//! # Registration
//!
//! Register for `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `Stop`,
//! `SubagentStop`, `SessionStart`, `SessionEnd`, `PreCompact` and
//! `Notification` with matcher `.*`.
//!
//! # It must never block the runtime
//!
//! Per `docs/contracts/adapter-contract.md §Errors and Backpressure`
//! this process **always exits 0** and never writes to stdout unless
//! explicitly asked to. Claude Code interprets a hook's stdout as
//! control output; emitting IR there would corrupt the session. All
//! problems go to stderr.
//!
//! # Usage
//!
//! ```text
//! freshdag-claude-hook [--store DIR] [--sink PATH] [--no-store]
//!                      [--suppress PATTERN[,PATTERN...]]
//!                      [--max-bytes N] [--stdout]
//!
//!   --store DIR       Store root. Events go to DIR/events.jsonl and this
//!                     adapter's coverage manifest is published to
//!                     DIR/coverage.jsonl. Default `.freshdag`.
//!                     Env: FRESHDAG_STORE
//!   --sink PATH       Write events to PATH instead of DIR/events.jsonl.
//!                     The manifest still goes to the store.
//!                     Env: FRESHDAG_SINK / FRESHDAG_CLAUDE_SINK
//!   --no-store        Publish no manifest, and default no sink. Events
//!                     are discarded unless --sink or --stdout is given.
//!                     For exercising the compiler in isolation.
//!   --suppress PATS   Coverage override; comma-separated event-kind
//!                     patterns (e.g. `fs.*,tool.completed`). Narrows the
//!                     published manifest to match.
//!                     Env: FRESHDAG_CLAUDE_SUPPRESS
//!   --max-bytes N     Sink byte cap. Env: FRESHDAG_CLAUDE_MAX_SINK_BYTES
//!   --stdout          Also print events to stdout. DEBUGGING ONLY —
//!                     never use while registered as a live hook.
//! ```
//!
//! # Why a store and not a bare file
//!
//! An IR log whose producer has no coverage manifest evaluates to
//! `unknown` on every artifact — `producer-missing-from-coverage` — so
//! events written without a manifest look like evidence and decide
//! nothing. Publishing the manifest is adapter-contract §Required
//! Behavior 4, and pointing the default at the same `.freshdag` root
//! `freshdag check` reads is what lets the two halves of the loop meet
//! without configuration.

use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

use freshdag_adapter_claude::compile::Compiler;
use freshdag_adapter_claude::config::{AdapterConfig, UNKNOWN_SESSION_ID};
use freshdag_adapter_claude::coverage::coverage_manifest_for;
use freshdag_adapter_claude::diagnostic::{Diagnostic, DiagnosticCode};
use freshdag_adapter_claude::sink::JsonlSink;
use freshdag_core::ir::EventKindPattern;
use freshdag_store::{CoverageRegistry, ProducerKey, COVERAGE_FILE_NAME, LOG_FILE_NAME};

/// Store root used when neither `--store` nor `$FRESHDAG_STORE` is set.
/// Matches `freshdag check`'s own default so the two halves of the loop
/// meet without configuration.
const DEFAULT_STORE: &str = ".freshdag";

const HELP: &str = "\
freshdag-claude-hook — compile one Claude Code hook payload into canonical IR

USAGE:
    freshdag-claude-hook [--store DIR] [--sink PATH] [--no-store]
                         [--suppress PATTERNS] [--max-bytes N] [--stdout]

    --store DIR   Store root (default `.freshdag`, env FRESHDAG_STORE).
                  Events -> DIR/events.jsonl, manifest -> DIR/coverage.jsonl.
    --sink PATH   Write events to PATH instead (env FRESHDAG_SINK).
    --no-store    Publish no manifest and default no sink.
    --suppress    Comma-separated event-kind patterns to suppress.
    --max-bytes   Sink byte cap.
    --stdout      Also print events to stdout. Debugging only.

Reads one JSON hook payload on stdin. Always exits 0.
";

#[derive(Debug, Default)]
struct Args {
    store: Option<String>,
    sink: Option<String>,
    suppress: Vec<String>,
    max_bytes: Option<u64>,
    stdout: bool,
    no_store: bool,
    help: bool,
}

fn parse_args<I: Iterator<Item = String>>(mut it: I) -> Result<Args, String> {
    let mut args = Args::default();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--store" => args.store = Some(it.next().ok_or("--store requires a path")?),
            "--no-store" => args.no_store = true,
            "--sink" => args.sink = Some(it.next().ok_or("--sink requires a path")?),
            "--suppress" => {
                let raw = it.next().ok_or("--suppress requires a pattern list")?;
                args.suppress.extend(split_patterns(&raw));
            }
            "--max-bytes" => {
                let raw = it.next().ok_or("--max-bytes requires a number")?;
                args.max_bytes = Some(raw.parse().map_err(|_| "--max-bytes must be an integer")?);
            }
            "--stdout" => args.stdout = true,
            "-h" | "--help" => args.help = true,
            other => return Err(format!("unrecognized argument `{other}`")),
        }
    }
    Ok(args)
}

fn split_patterns(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn main() -> ExitCode {
    // Every failure path below still returns SUCCESS: this process is in
    // the runtime's critical path and must never fail a tool call.
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(err) => {
            eprintln!("freshdag-claude-hook: {err}\n\n{HELP}");
            return ExitCode::SUCCESS;
        }
    };
    if args.help {
        eprintln!("{HELP}");
        return ExitCode::SUCCESS;
    }

    let mut input = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("freshdag-claude-hook: could not read stdin: {err}");
        return ExitCode::SUCCESS;
    }

    let mut config = AdapterConfig::new();
    let mut patterns = args.suppress;
    if let Ok(env) = std::env::var("FRESHDAG_CLAUDE_SUPPRESS") {
        patterns.extend(split_patterns(&env));
    }
    config =
        config.with_suppressed_kinds(patterns.into_iter().map(EventKindPattern::new).collect());

    // The manifest must describe the configuration the events were
    // actually compiled under — `--suppress` narrows what this adapter
    // emits, and a manifest claiming the un-suppressed set would
    // overstate its coverage.
    let compiler_config = config.clone();
    let mut compiler = Compiler::production(config);
    let events = compiler.compile_str(&input);

    if args.stdout {
        for event in &events {
            match serde_json::to_string(event) {
                Ok(line) => println!("{line}"),
                Err(err) => eprintln!("freshdag-claude-hook: serialize failed: {err}"),
            }
        }
    }

    // Resolve the store first: it decides where the coverage manifest
    // goes and, unless `--sink` overrides it, where events go too.
    let store_root = if args.no_store {
        None
    } else {
        Some(
            args.store
                .or_else(|| std::env::var("FRESHDAG_STORE").ok())
                .unwrap_or_else(|| DEFAULT_STORE.to_string()),
        )
    };

    // Publish the coverage manifest before writing any events. A log
    // whose producer has no manifest evaluates to `unknown` on every
    // artifact (`producer-missing-from-coverage`), so events without a
    // manifest are worse than useless — they look like evidence and
    // decide nothing.
    if let Some(root) = &store_root {
        publish_manifest(Path::new(root), &compiler_config);
    }

    let sink_path = args
        .sink
        .or_else(|| std::env::var("FRESHDAG_SINK").ok())
        .or_else(|| std::env::var("FRESHDAG_CLAUDE_SINK").ok())
        .or_else(|| {
            store_root
                .as_ref()
                .map(|r| Path::new(r).join(LOG_FILE_NAME).display().to_string())
        });

    let Some(sink_path) = sink_path else {
        if !args.stdout {
            eprintln!(
                "freshdag-claude-hook: no sink configured (--store, --sink, or \
                 $FRESHDAG_SINK); {} event(s) discarded",
                events.len()
            );
        }
        return ExitCode::SUCCESS;
    };

    let mut sink = JsonlSink::new(sink_path);
    let max_bytes = args
        .max_bytes
        .or_else(|| env_u64("FRESHDAG_CLAUDE_MAX_SINK_BYTES"));
    if let Some(max) = max_bytes {
        sink = sink.with_max_bytes(max);
    }

    let outcome = sink.write_all(&events);
    for err in &outcome.errors {
        eprintln!("freshdag-claude-hook: {err}");
    }
    if let Some(buffer) = &outcome.buffered_to {
        eprintln!(
            "freshdag-claude-hook: buffered to {} because the sink was unavailable",
            buffer.display()
        );
    }
    if outcome.dropped > 0 {
        report_drop(&mut compiler, &sink, &events, outcome.dropped);
    }
    ExitCode::SUCCESS
}

/// Publish this adapter's coverage manifest into `<store>/coverage.jsonl`,
/// once per `(producer, version)`.
///
/// Registration is skipped when a manifest for this exact
/// `(producer, version)` is already on file, because this binary runs
/// once per hook event and the registry appends unconditionally. Two
/// hook processes racing may still append the same record twice; that is
/// harmless, since the registry keys manifests by `(producer, version)`
/// on load.
///
/// Every failure here is reported to stderr and otherwise ignored. This
/// process is in the runtime's critical path (adapter-contract §Errors
/// and Backpressure) and must not fail a tool call over bookkeeping.
fn publish_manifest(store_root: &Path, config: &AdapterConfig) {
    let manifest = coverage_manifest_for(config);
    let path = store_root.join(COVERAGE_FILE_NAME);

    let mut registry = match CoverageRegistry::open(&path) {
        Ok(registry) => registry,
        Err(err) => {
            eprintln!(
                "freshdag-claude-hook: could not open {}: {err}",
                path.display()
            );
            return;
        }
    };

    if registry
        .manifest(&ProducerKey::of_manifest(&manifest))
        .is_some()
    {
        return;
    }

    if let Err(err) = registry.register(manifest) {
        eprintln!(
            "freshdag-claude-hook: could not publish coverage manifest to {}: {err}",
            path.display()
        );
    }
}

/// Record a back-pressure drop as a `diagnostic`. The record of a drop
/// must never itself be silently dropped.
fn report_drop(
    compiler: &mut Compiler<
        freshdag_adapter_claude::determinism::SystemClock,
        freshdag_adapter_claude::determinism::UuidV7Gen,
    >,
    sink: &JsonlSink,
    events: &[freshdag_core::ir::IrEvent],
    dropped: usize,
) {
    let session_id = events
        .first()
        .map_or(UNKNOWN_SESSION_ID, |e| e.session_id.as_str())
        .to_string();
    let diag = Diagnostic::new(
        DiagnosticCode::SinkBackpressureDrop,
        format!(
            "the sink could not accept {dropped} event(s) from this hook invocation (byte cap \
             reached, or neither the sink nor any buffer was writable). The NEWEST were \
             dropped; already-written events were preserved (invariant #4)."
        ),
    )
    .with("dropped_count", serde_json::json!(dropped))
    .with("sink", serde_json::json!(sink.path().display().to_string()));
    let event = compiler.standalone_diagnostic(&session_id, &diag);
    let outcome = sink.force_write(&event);
    for err in &outcome.errors {
        eprintln!("freshdag-claude-hook: {err}");
    }
    eprintln!("freshdag-claude-hook: dropped {dropped} newest event(s); older events preserved");
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.parse().ok()
}
