//! Claude Code adapter.
//!
//! Claude Code is adapter #1, not the architecture. This crate is
//! responsible for translating Claude Code hook payloads
//! (`PreToolUse`, `PostToolUse`, …) into FreshDAG's runtime-agnostic
//! canonical IR. See `docs/contracts/adapter-contract.md` and
//! `docs/contracts/execution-ir.md`.
//!
//! # Boundary
//!
//! Invariants #1, #2 and #14: **no Claude Code concept leaves this
//! crate.** Hook event names, `tool_use_id`, `transcript_path` and hook
//! payload shapes live in [`hook`] and [`compile`]; what crosses the
//! boundary is [`freshdag_core::ir::IrEvent`] and nothing else.
//!
//! # Determinism
//!
//! [`compile::Compiler::compile_str`] is a pure function of
//! `(payload, clock, id generator, config)`. Wall-clock time and UUIDv7
//! minting are injected via [`determinism::Clock`] and
//! [`determinism::IdGen`] so the golden-file conformance harness under
//! `fixtures/adapter-conformance/claude/` is byte-stable.
//!
//! # Never silent
//!
//! Every input produces at least one event. A payload the adapter
//! cannot classify becomes a `diagnostic`
//! (`docs/contracts/adapter-contract.md §Responsibilities #5`); silent
//! drops corrupt the dependency graph.
//!
//! ```
//! use freshdag_adapter_claude::{
//!     compile::Compiler,
//!     config::AdapterConfig,
//!     determinism::{FixedClock, SeededIdGen},
//! };
//! use freshdag_core::ir::EventKind;
//!
//! let mut compiler = Compiler::new(
//!     AdapterConfig::new(),
//!     FixedClock::conformance(),
//!     SeededIdGen::conformance(),
//! );
//! let events = compiler.compile_str(r#"{"hook_event_name":"Frobnicate","session_id":"s1"}"#);
//! assert_eq!(events.len(), 1);
//! assert_eq!(events[0].kind, EventKind::Diagnostic);
//! ```

#![warn(missing_docs)]

pub mod compile;
pub mod config;
pub mod coverage;
pub mod determinism;
pub mod diagnostic;
pub mod hook;
pub mod identity;
pub mod paths;
pub mod sink;

pub use compile::Compiler;
pub use config::{AdapterConfig, PRODUCER};
pub use coverage::coverage_manifest;
pub use diagnostic::{Diagnostic, DiagnosticCode, Severity};
pub use hook::HookEvent;
pub use sink::{JsonlSink, SinkOutcome};

#[cfg(test)]
mod tests;
