# ADR 0004: Trust-classified validity (exact / versioned / heuristic / volatile)

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** architect
- **Consulted:** build-systems memo (Workstream B), product/eval memo (Workstream E)

## Context

Existing build systems assume dependencies are either present-and-exact
(`hash(inputs) == cached`) or absent-and-recompute. Agent computations
have dependencies that don't fit that binary:

- A local file: fully hashable — `exact`.
- An Attio record: has a version token — `versioned`.
- A rendered pricing page: content hash is possible but expensive, ETag
  is the trustworthy signal — `versioned` if ETag present, `heuristic`
  otherwise.
- `web.search(...)` — no repeatable signal — `volatile`.

Treating all of these as if they were `exact` is the primary way
FreshDAG could silently report `fresh` on stale artifacts. Treating
them all as `volatile` reduces FreshDAG to "always rerun."

## Decision

Every fingerprint carries a **trust class** — one of
`exact | versioned | heuristic | volatile` — and the class is preserved
through the observation → store → engine → certificate pipeline.

Rules:

- A dependency's trust class cannot be silently promoted (heuristic
  never becomes exact).
- The artifact's status is the strict aggregation of its edges:
  `heuristic` edges cap the artifact's status at `likely-valid`.
- Probes may escalate a dependency's trust class over time by
  discovering a better signal (e.g., an endpoint gains an ETag), but
  never demote.

## Consequences

- Certificates carry more information than a naive hash. Consumers
  must handle the four classes.
- The engine has more branches than a monoclassed system, but the
  branches are small and well-tested (per-class rules in one file).
- The correctness story is defensible: no path in the code produces
  `valid` from `unknown`.

## Rejected Alternatives

- **Single hash per dependency.** Cannot represent the
  heuristic/volatile distinction. Would push FreshDAG toward silently
  wrong results, which is the anti-goal.
- **Trust class as a soft comment.** Rejected — must be
  machine-enforced (see `docs/contracts/certificate-contract.md`).
