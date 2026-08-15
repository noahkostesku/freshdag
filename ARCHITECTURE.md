# FreshDAG Architecture

This document is the canonical architectural reference for FreshDAG. It
defines the concepts, the flow of data through the system, and the
invariants that all subsystems must honor.

Complementary documents:

- `docs/NOVELTY.md` — the novelty firewall.
- `docs/contracts/` — stable interfaces subsystems implement against.
- `docs/adr/` — decisions and their reasoning.
- `docs/EVALUATION.md` — how correctness is measured.
- `docs/BUILD_PLAN.md` — build DAG and staged rollout.
- `CLAUDE.md` — day-to-day rules of engagement for humans and agents.

---

## 1. Product Model

FreshDAG observes agent computations, dynamically discovers what they
depend on, fingerprints those dependencies, attaches validity information
to produced artifacts, and determines when those artifacts must be
reconsidered.

The product surfaces its work through:

- **Validity certificates** attached to each artifact.
- **CLI commands**: `freshdag run`, `freshdag check`, `freshdag why`,
  `freshdag cert`, `freshdag graph`, `freshdag watch`.
- **A future graph UI** (see §11) that is a view over the system, not the
  system.

Every artifact must eventually be able to answer:

- Where did I come from?
- What did I depend on?
- Are those dependencies still valid?
- Why am I stale?
- What downstream artifacts would be affected?

---

## 2. Core Vocabulary

These terms appear throughout the codebase and are load-bearing. They are
defined in `freshdag-core::dependency`, `::artifact`, and `::computation`.

| Term | Meaning |
| --- | --- |
| **Dependency** | Something a computation observed and whose state may affect its output. Files, URLs, database records, MCP results, tool results, environment values, subprocess-observed files, other agent artifacts, skills, prompts, model/tool configuration. |
| **Fingerprint** | A representation of the observed state of a dependency. Not necessarily a full content hash; may be a version token, ETag, mtime, or heuristic digest depending on trust class. |
| **Validity** | Whether the dependency is still in a state that permits reuse of a previously produced artifact. Carries a **trust class**: `exact`, `versioned`, `heuristic`, `volatile`. |
| **Artifact** | Something produced by an agent computation. Content-addressed. |
| **Computation** | The transformation that observed dependencies and produced artifacts. Identified by a stable key derived from adapter, agent identity, and inputs where possible. |
| **Equivalence** | After recomputation, whether a new output is materially equivalent to the previous one. Distinct from validity. Configurable via a **comparator** (`exact`, `json-structural`, `set`, `numeric(tol)`, `judge(rubric)`, `custom`). |
| **Validity Certificate** | A portable, human-readable manifest tying an artifact to its dependencies, their fingerprints, and their trust classes. |

**Validity vs. equivalence.** Validity decides whether recomputation is
necessary. Equivalence decides whether the *result* of recomputation
propagates downstream. They must never be conflated.

**Trust classes.** Never represent a heuristic-freshness signal as an
exact-freshness signal. Unknown must default toward stale, never toward
silently fresh (invariant #7 in §5).

---

## 3. Execution / Data Model

FreshDAG has a strict separation of stages. Every subsystem lives at
exactly one stage.

```
Observation
    ↓
Canonical IR
    ↓
Dependency Graph
    ↓
Validity Evaluation
    ↓
Invalidation
    ↓
Optional Recomputation
    ↓
Equivalence
    ↓
Propagation / Early Cutoff
```

**Observation.** External runtime adapters (Claude Code, future
adapters) and systems-level observers (subprocess syscall traces,
filesystem watchers) emit raw observations.

**Canonical IR.** Adapters compile observations into a stable,
runtime-agnostic event vocabulary before anything else touches them.
This is the boundary that protects the core from any single agent
framework. Defined in `docs/contracts/execution-ir.md`.

**Dependency graph.** The engine consumes the canonical IR and
materializes computations, artifacts, and the edges between them.

**Validity evaluation.** For each dependency edge, the engine checks
whether its fingerprint still matches the world using **probes** (see §7).

**Invalidation.** Edges that cannot be proven valid mark their downstream
artifacts as `stale` (or `unknown`, never silently `valid`).

**Optional recomputation.** The engine may or may not choose to
recompute; that decision is orthogonal to invalidation. In v0 we do
detection only.

**Equivalence.** If recomputation happens, the new output is compared to
the prior output using a **comparator** appropriate to the artifact type.

**Propagation / early cutoff.** If the new output is materially
equivalent to the prior output, propagation to downstream computations is
stopped — even though the immediate node was recomputed.

---

## 4. Layers and Boundaries

```
┌─────────────────────────────────────────────────────────────────────┐
│  freshdag-cli   (freshdag run / check / why / cert / graph / watch) │
└───────────────────────────────┬─────────────────────────────────────┘
                                │
┌───────────────────────────────▼─────────────────────────────────────┐
│  freshdag-engine                                                    │
│    validity evaluation • invalidation • equivalence • early cutoff  │
└───┬────────────────────────────┬────────────────────────────┬───────┘
    │                            │                            │
┌───▼──────────┐          ┌──────▼────────┐          ┌────────▼──────┐
│ freshdag-    │          │ freshdag-     │          │ freshdag-     │
│ store        │          │ probes        │          │ observer      │
│              │          │               │          │               │
│ append-only  │          │ freshness     │          │ subprocess /  │
│ observation  │          │ queries       │          │ filesystem    │
│ log + derived│          │ against       │          │ syscall       │
│ state        │          │ external      │          │ tracing       │
│              │          │ mutable state │          │               │
└──────────────┘          └───────────────┘          └───────────────┘
        ▲                          ▲                          ▲
        │                          │                          │
        └──────── canonical IR events ─────────────────────────┘
                                ▲
                                │
                    ┌───────────┴────────────┐
                    │ freshdag-adapter-claude│  (adapter #1)
                    │  (future adapters here)│
                    └────────────────────────┘
                                ▲
                                │
                    ┌───────────┴────────────┐
                    │  agent runtimes        │
                    │  (Claude Code, etc.)   │
                    └────────────────────────┘

               freshdag-core (domain vocabulary)
               ─────────────────────────────────
               depended on by every crate above;
               depends on nothing.
```

**`freshdag-core`** is the domain-model crate. It defines
`Dependency`, `Fingerprint`, `Validity`, `Artifact`, `Computation`,
comparators, and the canonical IR event types. It has no I/O and no
dependency on any runtime.

**Adapters** (`freshdag-adapter-claude`, future) translate a specific
runtime's telemetry into the canonical IR. Adapters may NOT leak
runtime-specific concepts into the core.

**Observers** (`freshdag-observer`) provide sub-agent-layer observations
(subprocess syscalls, filesystem effects) and also emit canonical IR
events. Their contract is defined in `docs/contracts/observer-contract.md`.

**Probes** (`freshdag-probes`) answer "is this external dependency still
what it was?" cheaply. They are how validity evaluation avoids
recomputing agents just to notice nothing changed. Contract:
`docs/contracts/probe-contract.md`.

**Store** (`freshdag-store`) owns the append-only canonical log and any
derived materializations (dependency graph, indices). Derived state MUST
be reconstructable from the canonical log (invariant #5).

**Engine** (`freshdag-engine`) consumes store state and probe results,
runs validity evaluation, and drives invalidation. Recomputation
scheduling and equivalence live here too.

**CLI** (`freshdag-cli`) is the primary v0 surface.

---

## 5. Architectural Invariants

Every PR must honor these. Deviations require an ADR.

1. FreshDAG core must not depend on Claude Code.
2. Claude Code is adapter #1, not the architecture.
3. External runtimes compile their observations into a stable FreshDAG
   execution IR.
4. Raw observations are append-only where practical.
5. Derived graph state is reconstructable from canonical observations.
6. Every skip/reuse decision is explainable to a user.
7. Unknown dependency state must never silently become "fresh."
8. Heuristic validity is distinguishable from exact validity.
9. Every artifact is traceable to the computation that produced it.
10. Platform-specific observation belongs behind interfaces.
11. Tracing and provenance alone are not the product.
12. A visual graph is a view over the system, not the source of truth.
13. Public contracts are versionable and testable.
14. Agent integrations must not leak runtime-specific concepts into the
    core domain model unless unavoidable (and then an ADR is required).
15. Correctness beats cache hit rate.
16. FreshDAG is injectable into existing agent workflows rather than
    requiring users to rewrite in a new orchestration DSL.

---

## 6. Fingerprinting

Fingerprints are typed by trust class. The type must be preserved from
observation through certificate emission.

| Trust class | Meaning | Example fingerprint payload |
| --- | --- | --- |
| `exact` | Content-addressed. Two dependencies with the same fingerprint are byte-identical. | BLAKE3 or SHA-256 of canonicalized content. |
| `versioned` | A trustworthy monotonic identifier is available from the source. Two dependencies with the same version token are asserted equal by the source. | Attio `record.version`, HTTP `ETag`, Postgres `xmin`, MCP resource version. |
| `heuristic` | A cheap signal that usually implies equality but can be wrong. | File mtime + size, HTTP `Last-Modified`, page hash of a rendered URL. |
| `volatile` | The source has no trustworthy freshness signal. Freshness is only asserted for the duration of a declared TTL. | `web.search(...)`, `time.now()`, random. |

Trust classes drive validity evaluation (§7). Higher trust classes are
preferred; probes are permitted to *escalate* trust (e.g., a heuristic
probe that turns out to hit a versioned endpoint) but never to *demote*
it silently.

Byte-equality is insufficient for LLM outputs. Artifacts produced by
non-deterministic computations use content addressing over their bytes
for identity but rely on comparators (§8) for downstream propagation.

---

## 7. Validity Evaluation and Probes

Given an artifact `A` with dependency set `{d₁ … dₙ}`, each `dᵢ` has a
recorded fingerprint `fᵢ` and trust class `tᵢ`. Validity evaluation
proceeds edge-by-edge:

1. Select a probe capable of answering "is `dᵢ` still at fingerprint
   `fᵢ`?" at the recorded trust class.
2. Run the probe. Result is one of:
   - `Match` — recorded fingerprint still holds.
   - `Drift` — a new fingerprint is observed.
   - `Unknown` — probe failed, degraded, or is not implemented.
3. Downgrade the edge's contribution to the artifact's validity per the
   trust class:
   - `exact` + `Match` → `Valid`.
   - `versioned` + `Match` → `Valid`.
   - `heuristic` + `Match` → `Likely Valid` (never a bare `Valid`).
   - `volatile` inside TTL → `Likely Valid`; outside TTL → `Unknown`.
   - Any trust class + `Unknown` → `Unknown`.

The artifact's overall validity is the strict aggregation of its edges:

- All `Valid` → `Valid`.
- Any `Drift` → `Stale`.
- No `Drift`, some `Unknown` → `Unknown`.
- No `Drift`, no `Unknown`, some `Likely Valid` → `Likely Valid`.

Invariant #7 forbids any code path that promotes `Unknown` to `Valid`
without a probe result that justifies it.

Probe implementations live in `freshdag-probes` and register against
dependency schemes (`file://`, `https://`, `attio://`, `mcp://`, …).
Contract: `docs/contracts/probe-contract.md`.

---

## 8. Equivalence and Comparators

Equivalence is a property of *outputs*, not of dependencies. When
recomputation happens (which is out of scope for v0 but shapes the
architecture) a comparator decides whether the new output is materially
equivalent to the prior output.

Comparators are pluggable per artifact type:

- `exact` — byte equality after canonicalization.
- `json-structural` — order-insensitive JSON tree equality.
- `set` — set-equality of enumerated members.
- `numeric(tolerance)` — absolute or relative numeric tolerance.
- `judge(rubric)` — LLM-as-judge with an explicit rubric.
- `custom` — user-supplied function.

If a comparator reports equivalence, propagation to downstream
computations stops — this is Salsa-style early cutoff. If not,
propagation continues.

Comparators may be non-deterministic (LLM judges especially). We
must record the comparator identity and result on the certificate so
disagreements are auditable.

Contract: `docs/contracts/comparator-contract.md`.

---

## 9. Storage

The store is append-only for canonical observations. Derived state
(graph, indices, artifact certificates) can be dropped and rebuilt from
the canonical log at any time. This gives us:

- Determinism of graph reconstruction (invariant #5).
- A single source of truth to audit against.
- Freedom to change derived layouts without migrations.

v0 storage will be a simple on-disk directory with per-run JSONL. Real
storage (embedded database, remote store) lands after the interfaces
prove out.

Contract: initial sketch lives inside
`docs/contracts/execution-ir.md`; a dedicated store contract lands when
the engine is real.

---

## 10. External-State Probes

External sources (websites, CRMs, databases, MCP endpoints) have wildly
varying freshness signals. Probes normalize these into the trust classes
in §6.

Guidance:

- Prefer probes that return `versioned` results (native version tokens,
  ETag, `If-None-Match`) over probes that re-fetch and hash content.
- Any probe that returns `exact` must justify it by content-hashing the
  full response — no shortcuts.
- A probe that cannot execute (network failure, permission error) must
  return `Unknown`, never a stale success.

---

## 11. CLI

v0 commands (from the founding brief):

- `freshdag run <recipe>` — execute an agent under FreshDAG, emit the
  artifact and its certificate.
- `freshdag check <artifact>` — probe dependencies; report
  `fresh|stale|unknown` with an explanation.
- `freshdag why <artifact>` — human-readable reason for the last status
  decision.
- `freshdag cert <artifact>` — print the certificate.
- `freshdag graph` — emit the dependency graph (initially text/JSON).
- `freshdag watch` — long-running invalidation daemon.

Exit codes for `check` are load-bearing (used from CI/cron):
`0 = fresh`, `1 = stale`, `2 = unknown`, `>2 = tool error`.

---

## 12. Future Graph UI

Sits in `apps/web/`. Not part of v0. Will render:

- sources → computations → artifacts
- states: `valid`, `stale`, `unknown`, `likely valid`, `running`, `skipped`
- affordances: `why stale?`, blast radius, dependency provenance, last
  computation, cost, latency, equivalence cutoff, certificate.

Invariant #12: the graph UI is a view over the store. The store never
depends on the UI.

---

## 13. Platform Boundaries and Adapter Model

Adapters are pluggable. `freshdag-adapter-claude` is the first. The
adapter contract (`docs/contracts/adapter-contract.md`) commits an
adapter to:

- Emit canonical IR events with stable identities.
- Not leak runtime-specific concepts into the core.
- Declare its observation coverage (what it can and cannot see).

Observers are also pluggable. `freshdag-observer` will host at least
`fsatrace`-based subprocess tracing on Linux; on macOS FreshDAG
explicitly does not perform syscall-level observation and instead relies
on adapter-declared inputs plus explicit user declarations. See
`docs/contracts/observer-contract.md`.

---

## 14. Trust and Correctness Model

**Correctness beats cache hit rate.** A wrong `fresh` is worse than a
cautious `stale`. This principle drives:

- The `Unknown → not Valid` rule.
- Explicit trust classes on every fingerprint.
- Probes that fail closed.
- Comparators that are recorded, not implicit.

**Verification-first, observation-second.** For v0, agent-declared
inputs are the source of truth; observers *catch drift* from
declarations. We do not depend on syscall-level observation being
present or correct.

**Non-determinism is a first-class citizen.** LLMs are non-deterministic
at temperature > 0. FreshDAG expects outputs to differ across replays
and uses comparator-based equivalence to decide whether that matters.

---

## 15. Extension Points

Anticipated but not built in v0:

- Additional adapters (LangGraph, OpenAI Agents SDK, Anthropic Agent
  SDK, MCP-native runtimes).
- Additional observers (eBPF LSM on Linux, Detours on Windows, gVisor
  sandbox).
- Additional probes (Postgres logical replication, Salesforce change
  events, Notion API version tokens).
- Additional comparators (semantic embedding cosine, schema-typed diff).
- A remote store for team workflows.
- A visual graph UI (`apps/web`).

Each is expected to slot in behind the existing contracts without
architectural change. If a proposed extension requires a contract
change, follow the contract-change policy in `.claude/rules/architecture.md`.
