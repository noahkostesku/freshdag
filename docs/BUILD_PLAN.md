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

## 6. Current wave

Wave 1 landed on `main` (commits `f564f4c` S0, `b1e1efe` S1,
`26afd70` Phase B, `5cb280a` adversarial-review fixes; 103 test
results green): canonical IR + domain model, file probe, Linux
subprocess observer + macOS stub, certificate conformance fixtures,
scenario harness skeleton, HTTP probe research memo.

**Wave 2 is the next implementation wave.** The full execution plan
lives at `docs/prompts/wave-2.md` — it is a self-contained release-
manager prompt covering Phase A (`ReasonCode` enum), then the
parallel Phase B workstreams (W1 Claude adapter, W2 append-only
store, W5.2 HTTPS probe, W6.2 coverage-manifest wire-up, W3 derived
graph, W4 engine) with dependencies, verification requirements, and
the adversarial-review triple at the end.

**Wave 2 landed on `main` (`7e6b8bd`)**: Claude adapter + hook binary,
append-only store, derived dependency graph, HTTPS probe, engine
validity evaluation with probe registry / anti-thrash ledger / coverage
gate, `freshdag check` end-to-end with load-bearing exit codes, six
scenarios, 414 test results green.

---

## 6.2. Wave 3 — the dogfood wave

**Decided 2026-08-16 by `architect` at the Wave 2 completion review.**

### The one thing

> **Produce a certificate from a real, unmodified Claude Code session,
> and measure honestly how much of that session FreshDAG could see.**

FreshDAG has never done this. Every certificate in the repository — all
six scenarios, every conformance fixture, every one of the 414 tests —
is computed from hand-written IR. The 414 tests establish that the
implementation matches our assumptions. Nothing yet establishes that our
assumptions match an agent.

That is the largest single gap between "the code works" and "the thesis
is real", and it is far larger than the gaps the wave recorded (no macOS
observer, only two probe schemes, six fixtures). Those are all *answers*
to a question we have not asked. Which one matters is currently a guess,
and it is a guess we can retire cheaply.

### Why this and not the obvious candidates

- **More probes** (`attio://`, `mcp://`) — a bet that external
  dependencies dominate. Plausible, unmeasured. Each new scheme is real
  work and cannot be un-shipped.
- **A macOS observer backend** — a bet that `bash`/`task` opacity
  dominates. Also plausible, also unmeasured, and the observer memo
  already says the honest macOS answer is expensive.
- **Recomputation / `refresh-on-stale`** — the product-adversary is
  right that it is the value prop, but building minimal recomputation on
  top of detection we have never validated against a real run is
  building the second floor first.
- **More fixtures** — raises confidence in the engine, not in the
  thesis. Fixtures test us against our own model of the world.

The dogfood wave's output is a **number**, and that number picks Wave 4.
`docs/EVALUATION.md §3` already names it: **coverage silence rate**,
plus the fraction of dependency edges resolving to `no-probe-available`
and the fraction of computations carrying an undischarged `bash`/`task`
obligation. If real sessions are mostly `no-probe-available`, Wave 4 is
probes. If they are mostly unobserved subprocesses, Wave 4 is the
observer. If they are mostly files and it works, Wave 4 is
recomputation. Right now all three are defensible, which means none of
them is chosen.

### Hard gate before any of it: W9. Close the partial-coverage hole

**ADR 0011 lands before W10 and W11. This is a sequencing constraint,
not a preference.**

The Wave 2 verification found that an observer which declares itself
blind (`partial: {"fs.read": "cannot see reads inside subprocesses"}`)
discharges a `bash`/`task` observation obligation exactly as well as a
real one, and the certificate reports `valid`, exit 0. The engine never
reads `partial`; `CoverageEntry` drops it at the manifest→certificate
boundary.

That hole is **masked today** for an accidental reason: nothing in
production registers a coverage manifest, so every real check caps at
`unknown` and no real adapter output ever reaches `valid` at all. W10
and W11 exist precisely to remove that mask. Landing them first converts
a masked hole into a live one, on the path that produces the numbers
Wave 3 exists to generate — which would make those numbers worse than
useless, because they would be confidently wrong.

Owner: `core-engineer` (types, schema) with `store-engineer`
(`SilenceMeaning` becomes the single implementation),
`observer-engineer` and `claude-adapter` (reclassify their manifests).

**Expect the fsatrace observer to stop discharging too** (ADR 0011,
Amendment, Correction 3). Its shipped `fs.read` note is an
under-approximation — mmap reads are not emitted; statically linked
processes are invisible — so `bash`-invoking computations on Linux go
non-`valid` after the migration. That is the correct answer, not a
regression, and W9 must not be graded as broken for producing it.

**W9.1. The volatile reason-code split** (`core-engineer`) — **DONE,
2026-08-17.** ADR 0009's two reason codes (`volatile-within-ttl-unprobed`,
`dependency-changed-during-computation`), all three schema enums, the
`--accept-likely-valid` floor, and the fixtures §Consequences named.
The hole is closed: `--accept-likely-valid` no longer lifts a
never-probed volatile edge to exit 0.

Two notes for whoever reads this next:

- The `volatile-unprobed-within-ttl` fixture §Consequences asks for was
  not created. `volatile-external-dep` **is** that scenario — it
  registers no probe — and it was only ever asserting the wrong code.
  It now asserts `volatile-within-ttl-unprobed`. Adding a second,
  near-identical fixture to satisfy the letter of the ADR would have
  been coverage theatre.
- `dependency-changed-during-computation` also forces the conflicting
  edge's verdict to `Unknown`, which is a behaviour change beyond adding
  a vocabulary entry: before this, the store recorded the conflict, the
  engine logged a `graph.edge_conflict` diagnostic, and the certificate
  still said `valid`.

### What it costs

Small in code, and mostly work already owed. Three workstreams, all
downstream of W9:

**W10. Close the record loop** (blocking; `graph-engineer` +
`integration-engineer`). Implements ADR 0007 items 1–2: the engine
publishes its coverage manifest, the CLI registers it, and `check`
appends the engine's `probe.checked` / `diagnostic` events. Without
this, a real store's checks leave no trace and the anti-thrash protocol
stays inert. Also lands ADR 0007 item 3, the additive `probe_identity`
payload field.

Must also close ADR 0007 Amendment P1 **in the same PR**:
`probe.checked.trust_class` currently records the ledger's *adopted*
class, so replaying an engine-emitted event promotes a `heuristic`
dependency to `Valid`. Latent only because `--record` was dropped;
restoring `--record` without this ships a silent-promotion path.

**W11. Wire the hook to a store, not a file** (`claude-adapter` +
`integration-engineer`). Today `freshdag-claude-hook` appends to a bare
JSONL path and never registers a coverage manifest, so a store built
from a real session reports `producer-missing-from-coverage` on
everything and every artifact is `unknown` for the wrong reason. Two
concrete gaps:

- The hook must register `freshdag-adapter-claude`'s manifest in the
  store's `coverage.jsonl` on first write.
- **Nothing emits `artifact.produced`.** Without it there is no artifact
  to check. The adapter cannot know which file is "the artifact" — that
  is a user declaration — so this needs a minimal surface: promote
  `Write`/`Edit` outputs, or a `freshdag mark <path>` command. Smallest
  honest option wins; `integration-engineer` proposes.

**W12. Coverage honesty report** (`integration-engineer` + `eval-engineer`).
A `freshdag coverage` (or `why --coverage`) that computes the Tier-1
metrics above from a real store. This is the deliverable; W10 and W11
are its preconditions.

**W13. Run it, and write down what happened** (`architect` +
`eval-engineer`). Ten ordinary sessions with the hook installed. Mutate
the world. Run `check`. Record the outcome in `docs/DOGFOOD.md`,
including the sessions where FreshDAG saw nothing useful.

Landing alongside, because they touch the reason vocabulary and the
fixtures W13 will read: the contract-change PRs for ADR 0009 (two new
reason codes) and ADR 0010 (demotion trigger), and the ADR 0007
follow-through.

### The risk, named up front

The honest outcome may be "we saw 20% of it." That is the point. A wave
that can only confirm is not an experiment, and `docs/NOVELTY.md §2` now
rests the wedge on execution rather than on invention — which makes
`docs/EVALUATION.md`, not `docs/NOVELTY.md`, the document where the
claim is won or lost.

## 6.3. Remediation owed from the 2026-08-17 architect review

The retrospective `architect` review of the 26 PRs merged on 2026-08-17
(ADRs 0013 ratified with conditions, 0014 ratified with conditions,
0015 and 0016 new) left the following owed. None of it is new feature
work; all of it is closing a gap the review found.

**None of these is a reason to open new contract surface.** Read
§"Standing instruction" below before starting any of them.

### Blockers on further vocabulary growth

**R1. The reason-code drift guard** (`core-engineer`; `verifier`
confirms). ADR 0015 Decision 4, all four parts: one declaration site for
the variant list and its wire strings; schemas checked against the enum
rather than against a hand-maintained copy; a test that parses the
`§Reason Codes` table out of `certificate-contract.md` and compares it
to the enum both directions; a negative fixture pinning that an unknown
code fails to deserialize. The verifier confirms by reproducing the
original break — add a variant, omit it everywhere else, require red.

Until R1 lands, no fifteenth reason code, except one required to close a
demonstrated invariant-#7 violation (ADR 0015 Decision 5).

### `freshdag mark` becomes a first-class producer (ADR 0016)

**R2.** Injected `Clock` and `IdGen` in `mark`; the ambient pair stays
in `freshdag-cli::main` (`integration-engineer`). Prerequisite for R3.

**R3.** `fixtures/adapter-conformance/cli-mark/` — one fixture per
branch `b052a98` had to repair, plus a golden `artifact.produced`
envelope (`integration-engineer` authors, `eval-engineer` reviews).

**R4.** `mark`'s manifest declares `artifact.produced` as
`under-approximates` (`integration-engineer`). Latent today —
`artifact.produced` is not an effect kind — and the same shape as the
hole W9 closed.

**R5.** *Contract change.* A §Artifact Attribution clause in
`adapter-contract.md`, and a §Event Envelope edit in `execution-ir.md`
admitting a fourth producer shape and separating the normative
exact-string rule from the naming convention (`integration-engineer`
authors; `architect` signs as contract owner). Full process — label,
policy answers, owner acknowledgements.

### Corrections to the record

**R6.** `crates/freshdag-core/src/determinism.rs` still carries the
purity argument ADR 0013 §Decision 1 explicitly rejects
(`core-engineer`). Documentation only. A future agent reading the code
first — which `.claude/rules/` assumes they do — currently gets the
rejected argument.

### Carried forward, not new

**R7.** Undecorated test doubles in core's public API — ADR 0013 §Still
open, condition 3 of the PR #11 review, still unresolved.
`SeededIdGen`'s own documentation advertises that its output is
"indistinguishable from real UUIDv7s to any consumer that only inspects
the version," which is exactly what makes accidental production use
undetectable at the point of use. Candidate remedies: a feature gate, or
a rename to `ConformanceClock` / `ConformanceIdGen`. Decide it or
document why it is acceptable; do not leave it in a §Still open section
for a third wave.

### The masking risk, named so it is not forgotten

ADR 0014 caps every artifact from adapter #1 at `unknown`, permanently,
because Claude Code exposes no recipe. §6.2 set W10/W11 in motion to
*remove* an accidental universal mask; this is a deliberate one, and it
is still the right call — the alternative emitted no certificate at all.

Two standing constraints while it holds:

- The synthetic fixture path must keep exercising `valid`
  (`fixtures/certificate-conformance/positive/*` and four scenarios do
  today). A suite where nothing reaches `valid` cannot detect a
  promotion bug, and every latent freshness defect below the cap would
  be invisible. `b052a98`'s subagent-blindness defect is the worked
  example: unobserved delegation reaching `valid`, live only because the
  cap was hiding it.
- No `docs/DOGFOOD.md` figure may be read as evidence about the world
  while the cap is universal. The log already says this.

### Standing instruction: stop adding contract surface

The single most important thing this project should do next is **finish
the dogfood wave**. §6.2 pre-committed to it: the wave's output is a
number, and that number picks Wave 4. One session of ten exists, at 8%
observability, with `valid` unreachable through the only real adapter.

On 2026-08-17 the project added four reason codes, two ADRs, a new
producer, and a new CLI command, and measured its own thesis once. That
ratio is inverted. Contract surface is the cheapest thing to add and the
most expensive thing to be wrong about, and nothing added that day was
demanded by a real session — every one of them came from reading our own
code.

Until sessions 2–10 are in `docs/DOGFOOD.md`, the default answer to
"should this be a new reason code / field / status / contract clause?"
is **no**, and the burden is on the proposer to name the real session
that demanded it. The exceptions are R1–R6 above, which remove surface
or guard what exists, and anything closing a demonstrated invariant-#7
violation.

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

Two additional gates on `certificate-contract.md` specifically
(ADR 0015 Decision 5):

5. The reason-code drift guard (§6.3 R1) is in place. Freezing a
   vocabulary whose consistency is enforced only by one exhaustive
   `match` in the CLI freezes whatever happens to be in it.
6. The dogfood wave has reported (§6.2). A vocabulary that has never met
   a real session is a set of guesses, and stabilisation is the point at
   which adding a member becomes a `v0.2` schema bump.

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
- **A first-class `DependencyKey` core type.** Deferred 2026-08-16 by
  `architect`. The probe contract calls the key "scheme-specific
  opaque", and opacity is load-bearing: a scheme-discriminated enum
  would make every new scheme (`attio://`, `mcp://`) a `freshdag-core`
  change, which is invariant #14 and the "do NOT modify `freshdag-core`
  to accommodate the adapter" rule in `.claude/rules/architecture.md`.
  A bare newtype over `String` buys type-safety the `(scheme, key)` pair
  already provides, at the cost of a contract change touching core,
  store, engine, probes, two schemas, and every fixture. Not worth one
  today.
  **Trigger that forces it:** the first demonstrated key-aliasing
  defect — two spellings of the same dependency producing two graph
  nodes, two reverse-index entries, or two anti-thrash ledger keys. The
  `unicode-path` (NFC vs. NFD) and `symlink-swap` fixtures in
  `docs/EVALUATION.md §2`'s backlog are the ones most likely to expose
  it. When it lands it is a **newtype whose constructor is the single
  canonicalization point**, never a scheme enum.

- **Drift observations surviving probe removal.** Opened 2026-08-16 by
  `architect` while ruling ADR 0009 Amendment 2. A `volatile` edge whose
  probe reported `Drift` and whose probe is then uninstalled falls back
  to `likely-valid`, because probe removal is `Unknown` per
  `docs/contracts/probe-contract.md §Anti-thrash` and the TTL floor then
  licenses `likely-valid` on its own. A recorded `probe.checked` with
  result `drift` is a durable fact about the world; it arguably should
  outlive the probe that observed it. Consuming prior drift observations
  is a store-projection design (`store-engineer` + `graph-engineer`),
  not an evaluation-order tweak, and is out of scope for the volatile
  fix. Only `volatile` is affected — every other class falls to
  `unknown`, not to a floor.

- **Folding `freshdag-store` into `freshdag-engine`.** Rejected
  2026-08-16 by `architect`; see `ARCHITECTURE.md §4`. Not reopened
  without new information. Note that the friction W4 reported —
  `Engine::check` filtering its own event vector by `computation_id`,
  duplicating `CoverageRegistry::coverage_for_computation` — is an
  argument for a store API addition, not a merge, and is tracked in §6.2
  as part of W10.

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
