# ADR 0005: Append-only canonical observations; derived state is disposable

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** architect
- **Consulted:** build-systems memo (Workstream B)

## Context

FreshDAG's dependency graph and artifact statuses are derived views.
The primary evidence for any claim FreshDAG makes is a stream of
observation events emitted by adapters and observers. If that stream is
lossy or reorderable, we cannot trust any downstream decision.

## Decision

Canonical observations are:

- Append-only per producer.
- Never rewritten, never merged, never deleted (except via explicit
  retention policy at long time horizons; retention is a separate ADR
  when it becomes relevant).
- Sufficient by themselves to reconstruct any derived state.

Derived state (dependency graph, artifact certificates, indices) is
disposable and can be rebuilt from the canonical log at any time.

## Consequences

- Storage grows monotonically until retention is added. Acceptable for
  v0 (JSONL on disk, single machine).
- Migrations for derived-state layout become trivial: drop and
  rebuild. No schema migration for the derived layer.
- Debugging is bounded by the log — every "why is this stale" question
  has a trail.
- Producers must handle backpressure by buffering to disk locally;
  never by dropping observations silently.

## Rejected Alternatives

- **Mutable graph as source of truth.** Rejected — invariant #5 exists
  precisely to prevent this. Mutable derived state that has no
  primary log means bugs in the engine cannot be diagnosed after the
  fact.
- **Deterministic replay from a compact snapshot.** Interesting future
  optimization; not v0.
