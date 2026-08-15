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
//! freshdag-claude-hook [--sink PATH] [--suppress PATTERN[,PATTERN...]]
//!                      [--max-bytes N] [--stdout]
//!
//!   --sink PATH       JSONL sink. Env: FRESHDAG_SINK / FRESHDAG_CLAUDE_SINK
//!   --suppress PATS   Coverage override; comma-separated event-kind
//!                     patterns (e.g. `fs.*,tool.completed`).
//!                     Env: FRESHDAG_CLAUDE_SUPPRESS
//!   --max-bytes N     Sink byte cap. Env: FRESHDAG_CLAUDE_MAX_SINK_BYTES
//!   --stdout          Also print events to stdout. DEBUGGING ONLY —
//!                     never use while registered as a live hook.
//! ```

use std::io::Read;
use std::process::ExitCode;

use freshdag_adapter_claude::compile::Compiler;
use freshdag_adapter_claude::config::{AdapterConfig, UNKNOWN_SESSION_ID};
use freshdag_adapter_claude::diagnostic::{Diagnostic, DiagnosticCode};
use freshdag_adapter_claude::sink::JsonlSink;
use freshdag_core::ir::EventKindPattern;

const HELP: &str = "\
freshdag-claude-hook — compile one Claude Code hook payload into canonical IR

USAGE:
    freshdag-claude-hook [--sink PATH] [--suppress PATTERNS] [--max-bytes N] [--stdout]

Reads one JSON hook payload on stdin. Always exits 0.
";

#[derive(Debug, Default)]
struct Args {
    sink: Option<String>,
    suppress: Vec<String>,
    max_bytes: Option<u64>,
    stdout: bool,
    help: bool,
}

fn parse_args<I: Iterator<Item = String>>(mut it: I) -> Result<Args, String> {
    let mut args = Args::default();
    while let Some(arg) = it.next() {
        match arg.as_str() {
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

    let Some(sink_path) = args
        .sink
        .or_else(|| std::env::var("FRESHDAG_SINK").ok())
        .or_else(|| std::env::var("FRESHDAG_CLAUDE_SINK").ok())
    else {
        if !args.stdout {
            eprintln!(
                "freshdag-claude-hook: no sink configured (--sink or $FRESHDAG_SINK); \
                 {} event(s) discarded",
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
