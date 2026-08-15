# Build Plan

This document is the build DAG. It tells contributors (human or agent)
what must happen serially before broad parallel implementation begins,
and what can safely run in parallel after that.

Ownership per crate/area is in `docs/OWNERSHIP.md`.

---

## 1. Principles

- **Contracts before implementations.** Every parallel workstream in §3
  depends on the contract for its interface being stable.
- **One editor per contract file.** Multiple agents may implement
  against a contract simultaneously; only one edits the contract at a
  time.
- **Independent useful work over maximum concurrency.** If two tasks
  need the same file, they are one task.

---

## 2. Serial Spine (already done or in-flight)

Everything here must exist and be stable before §3 can begin at scale.

```
1. Novelty thesis (docs/NOVELTY.md)                              [DONE]
2. Canonical vocabulary (ARCHITECTURE.md §2)                      [DONE]
3. Execution IR contract (docs/contracts/execution-ir.md)         [DONE, provisional]
4. Adapter, observer, probe, comparator, certificate contracts    [DONE, provisional]
5. Core interfaces skeleton (freshdag-core::*)                    [SKELETON — types land next]
6. Fixture format (docs/EVALUATION.md §2)                         [SPEC — one fixture-runner needed]
```

Next serial task before broad parallelization:

- **S0. Land IR event enums** in `freshdag-core::ir` — the minimum set
  W5 and W6 need to compile: `IrEvent`, `EventKind`, `FsRead`,
  `FsWrite`, `ToolInvoked`, `ToolCompleted`, plus the coverage
  manifest struct. This is a sub-step of S1, extracted so
  observers/probes can start in parallel with the rest of S1.
  Owner: `core-engineer`. Blocks: W5, W6.
- **S1. Land the remaining `freshdag-core` types** — `Dependency`,
  `Fingerprint` (with trust class), `Validity`, `Artifact`,
  `Computation`, `Comparator` (interface only for v0; implementations
  deferred), `Certificate`. Owner: `core-engineer`. Blocks: W1, W2,
  W3, W4, W7.

**Claiming S0/S1.** The `release-manager` claims S0 first and either
implements or hands off to `core-engineer` with an explicit written
handoff. Only ONE agent may hold either task at a time. The task list
(TaskCreate/TaskList) is the lock — set `owner` on the task before
starting.

## 3. Parallel Workstreams (unlocked after S1)

Each workstream owns a well-bounded set of files. Cross-workstream
changes go through the owner or through a contract change.

### W1. Claude Code adapter
- Crate: `freshdag-adapter-claude`
- Owner: `claude-adapter`
- Depends on: S1, execution-ir contract, adapter contract.
- First deliverable: hook binary consumes PreToolUse/PostToolUse and
  emits `tool.invoked`/`tool.completed` to a local sink; coverage
  manifest.

### W2. Store — append-only log
- Crate: `freshdag-store`
- Owner: `store-engineer`
- Depends on: S1, execution-ir contract.
- First deliverable: append-only JSONL sink and a reader that
  reconstructs the event stream.

### W3. Store — derived graph
- Crate: `freshdag-store`
- Owner: `store-engineer`
- Depends on: W2.
- First deliverable: dependency-graph materializer from canonical events.

### W4. Engine — validity evaluation
- Crate: `freshdag-engine`
- Owner: `graph-engineer`
- Depends on: S1, probe contract, store W3.
- First deliverable: given a graph and a set of probe results, produce
  a status per artifact.

### W5. Probes — file, http
- Crate: `freshdag-probes`
- Owner: `probe-engineer`
- Depends on: S1, probe contract.
- First deliverable: `file://` (content hash) and `https://` (ETag /
  Last-Modified / conditional GET) probes.

### W6. Observer — Linux subprocess (fsatrace)
- Crate: `freshdag-observer`
- Owner: `observer-engineer`
- Depends on: S1, observer contract, execution-ir contract.
- First deliverable: fsatrace-based observer emitting `fs.read`,
  `fs.write` for a wrapped subprocess.

### W7. CLI
- Crate: `freshdag-cli`
- Owner: `integration-engineer`
- Depends on: engine W4, store W2.
- First deliverable: `freshdag check` end-to-end; exit codes wired.

### W8. Fixtures + eval harness
- Directory: `fixtures/`
- Owner: `eval-engineer`
- Depends on: engine W4, CLI W7 for driver.
- First deliverable: fixtures 1, 2, 3, 5, 6 from `EVALUATION.md`.

### W9. UI (deferred)
- Directory: `apps/web`
- Owner: `ui-engineer`
- Depends on: store W3 exports a readable graph JSON.
- First deliverable: not v0.

## 4. Dependency DAG (visual)

```
     [S1: freshdag-core types]
             │
   ┌─────────┼──────────┬─────────────┬──────────────┐
   │         │          │             │              │
   ▼         ▼          ▼             ▼              ▼
 [W1 Claude][W2 Store][W5 Probes] [W6 Observer]  [fixture format]
   adapter   append     file/http   fsatrace           │
             log                                        │
             │                                          │
             ▼                                          │
        [W3 Store                                       │
         derived graph]                                 │
             │                                          │
             ▼                                          │
        [W4 Engine                                      │
         validity]                                      │
             │                                          │
             ▼                                          │
        [W7 CLI]                                        │
             │                                          │
             └──────────────────────────────────────────┼──▶ [W8 Eval]
                                                        │
                                                        │
                                                        ▼
                                                   [nightly dogfood]
```

## 5. Rules For Parallelism

- Every workstream MUST branch from `main` and rebase before merging.
- Contract changes are ONLY made from the workstream that owns the
  contract, via a PR labeled `contract-change`. Other workstreams
  requesting a change file an issue and wait.
- Cross-workstream refactors go through the `integration-engineer`.

## 6. Immediate Next Steps

If you are starting a session right now, the first four actions are:

1. **S0** — land the IR event enums in `freshdag-core::ir`. Small,
   focused, unblocks W5/W6.
2. Once S0 lands, start W5 (file probe) and W6 (Linux observer
   fsatrace) in parallel — they now have the enums they need to
   compile.
3. **S1** — land the remaining `freshdag-core` types
   (`Dependency`, `Fingerprint`, `Validity`, `Artifact`,
   `Computation`, `Certificate`).
4. Once S1 lands, unlock W1 (Claude adapter), W2 (append-only store)
   in parallel. W3 (derived graph) follows W2. W4 (engine) follows W3.
   W7 (CLI) follows W4. W8 (fixtures) follows W7 for the driver but
   the fixture *content* can be authored in parallel starting after
   S1.

The `release-manager` assigns owners to S0 and S1. Every other
workstream is claimed by its subsystem owner per
`docs/OWNERSHIP.md`.

## 6.1. Provisional-to-Stable Contract Transitions

Contracts in `docs/contracts/` currently carry `Status: provisional`.
A contract transitions to `Status: stable` when ALL of:

1. At least one implementation consumes the contract end-to-end (an
   emitter and a reader, both compiling against it, both passing
   their conformance fixture set).
2. The relevant `fixtures/*-conformance/` suite is green on the CI
   matrix.
3. The contract owner explicitly requests the transition in a PR
   labeled `contract-change` with `[status: stable]` in the title.
4. `architect` and `release-manager` both sign off.

After stabilization, further changes require the full contract-change
process (`.claude/rules/architecture.md`) — no more provisional
edits.

## 7. Deferrals (explicit, not forgotten)

- Windows observer (W6-win).
- macOS Linux-VM tunnel for observation.
- Additional adapters (LangGraph, OpenAI Agents SDK).
- Remote store.
- Recomputation orchestration and comparator implementations. The
  interfaces live in `freshdag-core` for shape; no implementations
  ship in v0. Comparator ADR lands with the first recomputation
  workstream.
- **`refresh-on-stale` closed-loop mode.** The product-adversary
  review argued this is what turns detection into a value prop. v0
  ships detection; the follow-up ships refresh. Track separately.
- UI (`apps/web`).

Every deferred item is a candidate parallel workstream after v0
detection ships.

## 8. Existential Watch (platform-owner risk)

The novelty-adversary review escalated one risk from "novelty" to
"existential product": **Anthropic can ship freshness/staleness
tooling inside Claude Code in a two-week feature.** This is not
theoretical; hooks + transcripts + content hashing is a small delta
from their current shape.

Ongoing:

- `novelty-reviewer` polls the Claude Code changelog on a regular
  cadence (weekly during v0). Any change touching hook payload shapes,
  transcript persistence, or MCP result caching gets a triage note in
  `docs/NOVELTY.md §5`.
- If Anthropic ships a freshness feature, FreshDAG's differentiator
  becomes (a) adapter-agnostic — LangGraph, OpenAI Agents SDK,
  MCP-native runtimes; (b) trust-class-typed aggregation with
  machine-enforced anti-fresh-on-unknown; (c) cross-session probing
  of external mutable state (Claude Code's in-session focus is
  narrower). The wedge survives; the pitch has to shift.
