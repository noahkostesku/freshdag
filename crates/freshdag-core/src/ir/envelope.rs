//! The event envelope — every canonical IR event carries this shape.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use super::kind::EventKind;
use super::payload::TypedPayload;

/// A canonical execution IR event.
///
/// Matches `schemas/execution-ir/v0.1.json`. Payload is stored as
/// `serde_json::Value` so that events with kinds not yet typed round-trip
/// losslessly; use [`Self::decode_payload`] to get a strongly-typed
/// payload for the S0 subset.
///
/// `computation_id` is documented in `docs/contracts/execution-ir.md §Event
/// Envelope` as a deterministic function of
/// `(recipe_id_or_hash, canonicalized_declared_inputs,
/// adapter_identity_rule_version)`. The derivation function lives in a
/// follow-up; here we accept the caller's opaque string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrEvent {
    /// UUIDv7 (monotonic per producer).
    pub event_id: Uuid,
    /// Producer identity (e.g., `freshdag-adapter-claude`).
    pub producer: String,
    /// Producer semver.
    pub producer_version: String,
    /// Opaque per-execution session identifier defined by the producer.
    pub session_id: String,
    /// Computation this event contributes to; `None` for infrastructural events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computation_id: Option<String>,
    /// Single causal parent (retained for backwards-compatibility with
    /// early emitters; prefer `causal_inputs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    /// Causal predecessors — the events whose outputs this event consumed.
    ///
    /// Task-subagent joins and other DAG-shaped causality require a list,
    /// not a single parent. `None` and empty are semantically identical
    /// ("no known causal predecessor") — never "no causal predecessor
    /// exists."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causal_inputs: Option<Vec<Uuid>>,
    /// Event timestamp (RFC 3339, nanosecond precision).
    #[serde(with = "time::serde::rfc3339")]
    pub ts: OffsetDateTime,
    /// Event kind (wire form is the dotted lowercase string, e.g., `fs.read`).
    pub kind: EventKind,
    /// Kind-specific payload. Producers of unknown-kind events may
    /// populate this arbitrarily; use `decode_payload` for typed access
    /// to S0 kinds.
    pub payload: serde_json::Value,
}

impl IrEvent {
    /// Attempt to project `payload` into a strongly-typed [`TypedPayload`]
    /// for the S0 event kinds. Returns [`DecodeError::Unsupported`] for
    /// kinds whose typed variants have not yet landed.
    ///
    /// # Errors
    ///
    /// - [`DecodeError::Unsupported`] if `self.kind` is not one of the S0 kinds.
    /// - [`DecodeError::Malformed`] if the raw payload does not match the
    ///   expected shape for its kind.
    pub fn decode_payload(&self) -> Result<TypedPayload, DecodeError> {
        use super::payload::{FsRead, FsWrite, ToolCompleted, ToolInvoked};

        match self.kind {
            EventKind::FsRead => serde_json::from_value::<FsRead>(self.payload.clone())
                .map(TypedPayload::FsRead)
                .map_err(|e| DecodeError::Malformed(e.to_string())),
            EventKind::FsWrite => serde_json::from_value::<FsWrite>(self.payload.clone())
                .map(TypedPayload::FsWrite)
                .map_err(|e| DecodeError::Malformed(e.to_string())),
            EventKind::ToolInvoked => serde_json::from_value::<ToolInvoked>(self.payload.clone())
                .map(TypedPayload::ToolInvoked)
                .map_err(|e| DecodeError::Malformed(e.to_string())),
            EventKind::ToolCompleted => {
                serde_json::from_value::<ToolCompleted>(self.payload.clone())
                    .map(TypedPayload::ToolCompleted)
                    .map_err(|e| DecodeError::Malformed(e.to_string()))
            }
            other => Err(DecodeError::Unsupported(other)),
        }
    }
}

/// Errors from [`IrEvent::decode_payload`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
    /// Kind has no typed payload variant in this crate version.
    #[error("no typed payload for event kind `{0}` in this crate version")]
    Unsupported(EventKind),
    /// Raw payload did not match the expected shape for its kind.
    #[error("malformed payload for kind: {0}")]
    Malformed(String),
}
