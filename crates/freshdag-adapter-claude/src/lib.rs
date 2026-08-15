//! Claude Code adapter.
//!
//! Claude Code is adapter #1, not the architecture. This crate is responsible
//! for translating Claude Code hook payloads (PreToolUse, PostToolUse, etc.)
//! into FreshDAG's runtime-agnostic canonical IR. See
//! `docs/contracts/adapter-contract.md`.

#![warn(missing_docs)]
