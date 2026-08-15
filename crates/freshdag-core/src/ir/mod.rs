//! Canonical execution intermediate representation.
//!
//! The stable, runtime-agnostic vocabulary that FreshDAG reasons about.
//! Runtime-specific concepts (Claude Code hook payloads, fsatrace event
//! shapes, `OpenTelemetry` spans) must be compiled into this IR at the
//! adapter/observer boundary. No downstream subsystem branches on
//! runtime identity.
//!
//! Contract: `docs/contracts/execution-ir.md`.
//! Schema:   `schemas/execution-ir/v0.1.json`.
//!
//! ## Minimum surface (S0)
//!
//! S0 lands only the subset needed to unblock the first probe (W5.1) and
//! observer (W6.1) implementations:
//!
//! - [`IrEvent`] — the event envelope.
//! - [`EventKind`] — the enumerated event kinds.
//! - Typed payloads for [`EventKind::FsRead`], [`EventKind::FsWrite`],
//!   [`EventKind::ToolInvoked`], [`EventKind::ToolCompleted`] via
//!   [`TypedPayload`].
//! - [`Hash`] — trust-class-tagged content hash.
//! - [`CoverageManifest`] — producer coverage declaration.
//!
//! The remaining event kinds are enumerated in [`EventKind`] but their
//! typed payload variants land in follow-up workstreams alongside the
//! producers that emit them. Untyped access via [`IrEvent::payload`]
//! (as `serde_json::Value`) works today for every kind.

mod coverage;
mod envelope;
mod hash;
mod kind;
mod payload;

pub use coverage::{CoverageManifest, EventKindPattern};
pub use envelope::IrEvent;
pub use hash::{Hash, HashAlgo, HashParseError};
pub use kind::EventKind;
pub use payload::{
    FsRead, FsReadKind, FsWrite, FsWriteMode, ToolCompleted, ToolInvoked, ToolKind, TypedPayload,
};

#[cfg(test)]
mod tests;
