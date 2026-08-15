//! Systems-level observation.
//!
//! Below-tool-layer observers (e.g., subprocess syscall tracing) that
//! surface path-reads, path-writes, and subprocess spawns as canonical
//! observation events. Platform-specific implementations sit behind
//! interfaces — see `docs/contracts/observer-contract.md`.

#![warn(missing_docs)]
