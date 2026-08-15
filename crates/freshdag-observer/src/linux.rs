//! Linux subprocess observation via `fsatrace`.
//!
//! Approach (per observer research memo and observer contract):
//! `fsatrace` intercepts libc I/O via `LD_PRELOAD` and writes a
//! newline-delimited trace to a file. Each line has the shape
//! `<op>|<path>` where `<op>` is one of `r` (read), `w` (write),
//! `m` (modify), `d` (delete), `q` (query/stat), `t` (touch), plus
//! `R`/`W` for rename (source/target on adjacent lines). We consume the
//! trace after the wrapped process exits and translate it to canonical
//! IR events.
//!
//! For W6.1 we support the read and write kinds — the minimum needed
//! for the Wave-1 fixtures. Full trace support (renames, negative
//! deps, mmap-pessimism) lands in follow-up workstreams.
//!
//! ## Cross-platform posture
//!
//! - Linux with `fsatrace` on `$PATH` → real observation.
//! - Any other platform → [`ObserverError::NotSupportedOnPlatform`].
//! - Linux without `fsatrace` on `$PATH` →
//!   [`ObserverError::BackendMissing`].
//!
//! The coverage manifest degrades with the backend: off-platform,
//! [`FsatraceObserver::coverage`] declares `emits: []` rather than
//! `fs.read`/`fs.write`, because consumers compute coverage deficits
//! from `emits` and an off-platform `fs.*` claim would certify
//! unobserved subprocess I/O as covered (invariant #7).
//!
//! The workspace compiles on macOS; we simply cannot run
//! `FsatraceObserver::observe` there. Tests that need to exercise the
//! observer trait use [`crate::replay::ScriptedObserver`], which is
//! platform-independent.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use freshdag_core::ir::{CoverageManifest, EventKind, EventKindPattern, ProducerRole};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::observer::{CommandInvocation, ObservationRun, Observer, ObserverError};

/// Observer that wraps a subprocess using the external `fsatrace`
/// binary.
#[derive(Debug, Clone)]
pub struct FsatraceObserver {
    fsatrace_path: PathBuf,
    session_id: String,
    version: String,
}

impl FsatraceObserver {
    /// Construct with an explicit path to the `fsatrace` binary.
    #[must_use]
    pub fn with_binary(fsatrace_path: impl Into<PathBuf>, session_id: impl Into<String>) -> Self {
        Self {
            fsatrace_path: fsatrace_path.into(),
            session_id: session_id.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Construct by discovering `fsatrace` on `$PATH`.
    ///
    /// # Errors
    ///
    /// - [`ObserverError::NotSupportedOnPlatform`] on non-Linux.
    /// - [`ObserverError::BackendMissing`] if `fsatrace` is not on
    ///   `$PATH`.
    pub fn discover(session_id: impl Into<String>) -> Result<Self, ObserverError> {
        if !cfg!(target_os = "linux") {
            return Err(ObserverError::NotSupportedOnPlatform(format!(
                "fsatrace is Linux-only; current target is {}",
                std::env::consts::OS
            )));
        }
        let path = which("fsatrace").ok_or_else(|| {
            ObserverError::BackendMissing(
                "fsatrace binary not found on $PATH; install fsatrace or use with_binary()"
                    .to_string(),
            )
        })?;
        Ok(Self::with_binary(path, session_id))
    }
}

impl Observer for FsatraceObserver {
    /// Coverage this backend delivers **on the platform it is running
    /// on**.
    ///
    /// On non-Linux targets [`Observer::observe`] always returns
    /// [`ObserverError::NotSupportedOnPlatform`], so this backend emits
    /// nothing there. Declaring `fs.read`/`fs.write` anyway would be
    /// fabricated coverage: consumers read `emits` (not `platforms`)
    /// when they compute the certificate contract's coverage deficit,
    /// so an off-platform manifest that still claims `fs.*` would let a
    /// `bash` invocation certify as `valid` with zero observation
    /// behind it. That is precisely the invariant #7 violation the
    /// coverage manifest exists to prevent, so the manifest degrades
    /// with the backend.
    fn coverage(&self) -> CoverageManifest {
        let mut capabilities: std::collections::BTreeMap<String, serde_json::Value> =
            std::collections::BTreeMap::new();
        let mut partial: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();

        if !cfg!(target_os = "linux") {
            for k in [
                "fs.read",
                "fs.write",
                "fs.rename",
                "fs.stat",
                "fs.dirlist",
                "proc.spawn",
                "net.connect",
            ] {
                capabilities.insert(k.to_string(), json!(false));
            }
            return CoverageManifest {
                producer: "freshdag-observer-fsatrace".to_string(),
                version: self.version.clone(),
                platforms: vec!["linux-x86_64".to_string(), "linux-arm64".to_string()],
                emits: vec![],
                partial,
                capabilities,
                known_limitations: vec![format!(
                    "fsatrace is Linux-only; current target is {} — this backend \
                     emits nothing here and declares no coverage",
                    std::env::consts::OS
                )],
            };
        }

        capabilities.insert("fs.read".to_string(), json!(true));
        capabilities.insert("fs.write".to_string(), json!(true));
        capabilities.insert("fs.rename".to_string(), json!(false));
        capabilities.insert("fs.stat".to_string(), json!(false));
        capabilities.insert("fs.dirlist".to_string(), json!(false));
        capabilities.insert("proc.spawn".to_string(), json!(false));
        capabilities.insert("net.connect".to_string(), json!(false));
        capabilities.insert(
            "symlink_resolution".to_string(),
            json!("at-observation-time-not-yet-implemented"),
        );
        capabilities.insert("mmap_reads".to_string(), json!("not-yet-covered"));

        // `emits` says "this kind can appear"; `partial` says "and its
        // absence still does not mean nothing happened." Both fs kinds
        // are genuinely partial today (observer-contract §Correctness
        // Pitfalls), so declaring them without these notes would
        // overclaim.
        partial.insert(
            "fs.read".to_string(),
            "mmap reads bypass LD_PRELOAD interception and are not emitted \
             (observer-contract §Correctness Pitfalls #2); statically linked or \
             raw-syscall processes are invisible"
                .to_string(),
        );
        partial.insert(
            "fs.write".to_string(),
            "rename-atomic writes are emitted against the temporary path only; the \
             synthetic fs.write at the rename target required by observer-contract \
             §Required Behavior #3 is not yet implemented"
                .to_string(),
        );

        CoverageManifest {
            producer: "freshdag-observer-fsatrace".to_string(),
            version: self.version.clone(),
            role: ProducerRole::Observer,
            platforms: vec!["linux-x86_64".to_string(), "linux-arm64".to_string()],
            emits: vec![
                EventKindPattern::from("fs.read"),
                EventKindPattern::from("fs.write"),
            ],
            partial,
            capabilities,
            known_limitations: vec![
                "glibc only; musl targets require strace fallback".to_string(),
                "W6.1 only emits fs.read and fs.write; renames/mmap/stat land later".to_string(),
                "cannot observe processes that fork before LD_PRELOAD attaches".to_string(),
                "fsatrace delete (`d`) and query (`q`) trace ops are parsed but dropped; \
                 no fs.unlink / fs.stat events are emitted"
                    .to_string(),
            ],
        }
    }

    fn observe(&self, invocation: &CommandInvocation) -> Result<ObservationRun, ObserverError> {
        if !cfg!(target_os = "linux") {
            return Err(ObserverError::NotSupportedOnPlatform(format!(
                "fsatrace is Linux-only; current target is {}",
                std::env::consts::OS
            )));
        }

        // Trace file is a tempfile alongside the working directory.
        let trace_path = invocation
            .cwd
            .join(format!(".freshdag-fsatrace.{}.log", Uuid::new_v4()));

        // Assemble: fsatrace <ops> <trace_file> -- <program> <args...>
        let mut cmd = Command::new(&self.fsatrace_path);
        cmd.arg("rwmdt")
            .arg(&trace_path)
            .arg("--")
            .arg(&invocation.program)
            .args(&invocation.args)
            .current_dir(&invocation.cwd)
            .envs(invocation.env.iter().map(|(k, v)| (k.clone(), v.clone())))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd.output()?;

        let combined_output = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let trace = std::fs::read_to_string(&trace_path).map_err(|e| {
            ObserverError::Malformed(format!(
                "could not read fsatrace log {}: {e}",
                trace_path.display()
            ))
        })?;
        let _ = std::fs::remove_file(&trace_path);

        let events = parse_fsatrace_lines(&trace, &self.session_id, &self.version);

        Ok(ObservationRun {
            exit_code: output.status.code(),
            combined_output,
            events,
        })
    }
}

/// Parse fsatrace's newline-delimited trace into canonical IR events.
///
/// Public so tests (and downstream tooling that just wants the parser)
/// can exercise it without spawning a subprocess. See fsatrace's
/// documentation for the wire format.
#[must_use]
pub fn parse_fsatrace_lines(
    trace: &str,
    session_id: &str,
    version: &str,
) -> Vec<freshdag_core::ir::IrEvent> {
    let mut events = Vec::new();
    for line in trace.lines() {
        let Some((op, path)) = line.split_once('|') else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        match op {
            "r" => events.push(make_event(
                session_id,
                version,
                EventKind::FsRead,
                json!({
                    "path": path,
                    "size": 0,
                    "raw_path": path,
                    "read_kind": "direct",
                    "impure": false
                }),
            )),
            "w" | "m" | "t" => events.push(make_event(
                session_id,
                version,
                EventKind::FsWrite,
                json!({
                    "path": path,
                    "size": 0,
                    "mode": mode_for_op(op),
                }),
            )),
            _ => {
                // rename source/target ("R"/"W" with adjacent lines),
                // deletes, queries, and any future op are not covered
                // by W6.1 and are silently skipped. Consumers see this
                // as absent-because-uncovered via the coverage manifest,
                // which is honest silence per invariant #7.
            }
        }
    }
    events
}

fn mode_for_op(op: &str) -> &'static str {
    match op {
        "m" => "append",
        "t" => "create",
        _ => "truncate", // "w" and any fallback
    }
}

fn make_event(
    session_id: &str,
    version: &str,
    kind: EventKind,
    payload: serde_json::Value,
) -> freshdag_core::ir::IrEvent {
    freshdag_core::ir::IrEvent {
        event_id: Uuid::new_v4(),
        producer: "freshdag-observer-fsatrace".to_string(),
        producer_version: version.to_string(),
        session_id: session_id.to_string(),
        computation_id: None,
        parent_id: None,
        causal_inputs: None,
        ts: OffsetDateTime::now_utc(),
        kind,
        payload,
    }
}

/// Look up an executable on `$PATH`. Small stand-in for the `which`
/// crate to avoid pulling a new workspace dep for one function.
fn which(bin: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let full = dir.join(bin);
        if full.is_file() {
            return Some(full);
        }
    }
    None
}
