# FreshDAG — Implementation Wave 2

> **How to use this file.** This is a version-controlled execution plan
> for a Claude Code release-manager session. A fresh session should
> read `CLAUDE.md` first, then this file, then verify actual
> repository state (git log, cargo test, current contracts) before
> executing. Repository state and contracts are authoritative when
> this prompt is stale; escalate to the human on divergence rather
> than silently redesigning.

Act as the FreshDAG **release-manager** for Wave 2. Do not skip the
base-state read; the Wave 2 plan below assumes the Wave 1 tip is
`5cb280a` and 103 tests are green. Confirm before doing anything.

## 0. Establish base state

**Read (in this order):**

1. `CLAUDE.md` — the project constitution.
2. `ARCHITECTURE.md` — especially §5 invariants and §7 validity evaluation.
3. `docs/NOVELTY.md` — the surviving thesis lives in §2; the firewall in §3.
4. `docs/BUILD_PLAN.md` — the workstream DAG.
5. `docs/OWNERSHIP.md` — who owns what; who arbitrates.
6. `docs/EVALUATION.md` — the fixture set and correctness bar.
7. All files under `docs/contracts/` — every contract Wave 2 touches.
8. `docs/research/http-validity-probes.md` — semantics recommendation for W5.2.
9. `.claude/rules/architecture.md`, `.claude/rules/worktrees.md`,
   `.claude/rules/git.md`, `.claude/rules/novelty.md`,
   `.claude/rules/testing.md`.
10. `.claude/agents/release-manager.md` — your charter.
11. The full canonical model in `crates/freshdag-core/src/**` — this
    is what you extend, not what you replace.

**Inspect:**

```bash
git status
git log --oneline -10
git branch -vv
cargo build --workspace
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

**Expected baseline:**

- Local `main` at commit `5cb280a` (Wave 1 tip), which is
  `fix(core): apply adversarial-review findings from Wave 1 Phase E`.
- 103 test results, 100% pass on macOS. On Linux with `fsatrace` on
  `$PATH`, additional observer smoke tests may pass; without, the
  fsatrace tests skip cleanly.
- No uncommitted changes. `origin/main` should match local `main`
  after Wave 1 was pushed.

If reality diverges from this baseline, the first task is diagnosing
the delta and reporting to the human. Do not proceed with Wave 2
work until you understand why.

## 1. Non-negotiable invariants (Wave 2 additions in bold)

Every invariant from `ARCHITECTURE.md §5` still binds. The four that
Wave 2 will most often strain:

- **Unknown never becomes Fresh.** Every new code path in the engine,
  store, or adapter must preserve this. When the engine consumes
  `EdgeVerdict::Match { … observed_fp, observed_trust_class }`, it
  MUST use them for anti-thrash logic and MUST NOT throw them away
  (Wave 1 widened this shape specifically for you — do not narrow it).
- **Heuristic never silently promotes to Exact/Versioned.**
- **Claude-Code-specific concepts stay out of `freshdag-core`.** The
  Claude adapter compiles hook JSON into canonical IR; it does not
  export hook payload shapes to anyone.
- **Coverage-manifest silence is not "nothing happened."** The
  coverage-deficit rule in `Certificate::check_coverage_deficit` is
  the load-bearing enforcement point. The engine MUST call it before
  emitting a `Valid` certificate.
- **Correctness beats cache hit rate.** No shortcut skips the
  coverage-deficit rule or the anti-thrash rule to improve numbers.
- **Contracts change explicitly, never silently.** Any diff under
  `docs/contracts/` or `schemas/` requires the contract-change
  process in `.claude/rules/architecture.md`.

## 2. Phase A — serial `ReasonCode` enum

Land `ReasonCode` first. Every other Wave 2 workstream depends on it,
and the change ripples through the certificate schema and the
scenarios. Do not fan out until Phase A is committed and green.

**Scope:**

- New `pub enum ReasonCode` in
  `crates/freshdag-core/src/dependency/validity.rs` with wire form
  kebab-case (`#[serde(rename_all = "kebab-case")]`).
- Variants (from Wave 1 usage plus the two Wave 2 additions):
  `Drift`, `ProbeUnknown`, `TrustClassHeuristicCapsAtLikelyValid`,
  `TrustClassVolatileCapsAtLikelyValid`, `TtlExpired`,
  `CoverageDeficit`, `NoDependenciesObserved`, `ProbeTrustDemoted`,
  `ProducerMissingFromCoverage`.
- Replace `ValidityReason.reason: String` with
  `reason: ReasonCode`. Add `#[serde(default, skip_serializing_if =
  "Option::is_none")] pub detail: Option<String>` for human context
  (rate-limit reason, HTTP status, etc.).
- Update every construction site in `Validity::aggregate` and
  `Certificate::check_coverage_deficit`.
- Update `schemas/certificate/v0.1.json` so
  `depends_on[].reasons[].reason` is an enum of the new wire strings.
- Update `schemas/scenario/v0.1.json` so
  `expected.certificate_status.reason_codes[]` is also enum-restricted.
- Update the certificate conformance fixtures and scenario fixtures
  whose `reason` field is now a wire enum, not free text.
- All 103 tests must stay green.

**Verifier:** independent read-only sub-agent checks schema/type
agreement, wire-form stability, and that no old string reasons
survive in code (grep for the old string literals).

**Commit:** `feat(core): Phase A — ReasonCode enum + typed reasons`.

## 3. Phase B — parallel workstreams (isolated worktrees)

After Phase A lands on `main`, dispatch the following four
workstreams as independent implementation agents. Each MUST run in
its own git worktree per `.claude/rules/worktrees.md`. Verifier for
each MUST be a different agent than the implementer.

### W1 — Claude Code adapter (`crates/freshdag-adapter-claude`)

**Owner charter:** `.claude/agents/claude-adapter.md`.
**Contract:** `docs/contracts/adapter-contract.md`.

**Scope:**

- Hook binary consumes Claude Code hook payloads on stdin (JSON,
  one payload per invocation as Claude Code's hook system does).
- Emits canonical IR events per `docs/contracts/execution-ir.md`.
  Coverage manifest declares `session.*`, `tool.*`, plus synthesized
  `fs.read` / `fs.write` for the `Read` / `Write` / `Edit` tools.
- Bash and Task tool invocations emit `tool.invoked` events with
  `tool_kind = "bash" | "task"` (per the S0 contract update);
  filesystem effects INSIDE those subprocesses are observer territory
  and the adapter's coverage manifest MUST reflect that with a
  `partial` note.
- MCP tools use the naming convention `mcp/<server>/<tool>` and
  `tool_kind = "mcp"`.
- MUST emit a `diagnostic` event on any hook payload it cannot
  classify. Silent drops are a contract violation (adapter contract
  §Responsibilities #5).

**Fixtures:** at least three golden hook payloads under
`fixtures/adapter-conformance/claude/` with expected IR event streams.

**Verification:** conformance harness parses the golden payloads,
replays them through the adapter, and asserts the emitted IR stream
matches the golden — deterministic (no wall-clock leakage into
`event_id`; use a seeded UUIDv7 fake or override `ts`).

### W2 — append-only store (`crates/freshdag-store`)

**Owner charter:** `.claude/agents/store-engineer.md`.
**Contract:** `docs/contracts/execution-ir.md §Ordering` — the
canonical linearization rule `(ts, producer, event_id)` is the
determinism guarantee.

**Scope:**

- JSONL sink that append-writes IR events. Buffer-to-disk on
  backpressure per adapter/observer contracts — never drop.
- JSONL reader that returns events in producer order (per producer,
  totally ordered by `event_id`).
- A `linearize(events: impl Iterator<Item = IrEvent>) -> Vec<IrEvent>`
  helper implementing the canonical linearization for cross-producer
  determinism.
- Reconstruction test: authored event stream → append → read →
  linearize → assert byte-identical to a sorted-in-memory
  reconstruction. This is the invariant #5 keystone.

**Do NOT:**

- Add derived-state layouts. That is W3.
- Introduce a database. JSONL on disk only for v0.
- Add cross-machine event routing.

### W5.2 — HTTPS probe (`crates/freshdag-probes/src/https.rs`)

**Owner charter:** `.claude/agents/probe-engineer.md`.
**Contract:** `docs/contracts/probe-contract.md`.
**Design source:** `docs/research/http-validity-probes.md` — treat
its decision matrix as authoritative.

**Scope:**

- Implements the `Probe` trait for scheme `https`.
- Uses `If-None-Match` when the recorded fingerprint is an ETag;
  falls back to `If-Modified-Since` when only Last-Modified is
  recorded.
- Strong ETag → `versioned`. Weak ETag (`W/"..."`) → `heuristic`
  (never versioned). Last-Modified → `heuristic`.
- Content-hash fallback is **opt-in per dependency** (a probe-config
  flag). Streaming BLAKE3, size-capped. No canonicalization in v0 —
  raw bytes with media type recorded as metadata. Escalate any push
  to canonicalize before shipping.
- No credential storage; auth-configured probes are a follow-up.
- Emits `probe.trust_demoted` diagnostic when a version signal
  disappears (per anti-thrash protocol).
- Fixture-backed tests spin up an in-process `httptest` server on
  `127.0.0.1:0`. NO real network access from CI.

**Add dependency:** `reqwest` with `blocking` feature. Discuss with
architect if you want to introduce an async runtime — Wave 1's
`Probe` trait is sync; async is a decision that must be made
explicitly, not smuggled.

### W6.2 — observer coverage-manifest wire-up

**Owner charter:** `.claude/agents/observer-engineer.md`.

**Scope:**

- The `CoverageEntry` in `freshdag-core::certificate` now carries
  `emits: Vec<EventKindPattern>`. Wave 1 populated this via
  `From<&CoverageManifest>`. Wave 2 wires it end-to-end: producers
  register their manifests with the store; the store attaches the
  producer's manifest to each certificate's `observation_coverage`.
- Add a smoke test where a scripted observer with `emits: []` and
  a bash `tool.invoked` cannot produce a `valid` certificate
  (exercises `Certificate::check_coverage_deficit` via a real
  event stream, not the synthetic fixtures Wave 1 used).

This is small; it can share a worktree with W2 if the store owner
agrees.

### W3 — derived dependency graph (`crates/freshdag-store`)

**Dependency:** blocked on W2.

**Scope:**

- Consumes the canonical IR event stream (via W2's reader) and
  materializes:
  - a per-computation dependency-graph view (`Vec<Dependency>` per
    `ComputationId`),
  - a `depends_on` index from `DependencyId` to the set of
    `ArtifactId`s that consume it (for blast-radius reasoning),
  - the producers-that-contributed set per computation (feeds
    `observation_coverage`).
- Derived state is disposable (`derived/` directory that can be
  dropped and rebuilt from the JSONL log).
- Deterministic reconstruction test: drop derived, replay, verify
  identical.

### W4 — engine validity evaluation (`crates/freshdag-engine`)

**Dependency:** blocked on W3, W5.2, and Phase A `ReasonCode`.

**Scope:**

- `Engine::check(&self, artifact_id) -> Certificate` — the load-bearing
  entry point.
- Dispatches per-dependency probe checks to a `ProbeRegistry`.
- **Anti-thrash implementation:** per-(dependency_key, probe_identity)
  in-memory state tracking `pending_escalation: Option<(TrustClass,
  count: u8, first_seen: Instant)>` and `last_adopted_trust_class`.
  N=2 consecutive higher-trust observations before adoption; explicit
  demotion emits `probe.trust_demoted`. Persistent state (in the
  store) is a follow-up.
- **Coverage-deficit enforcement:** engine calls
  `Certificate::check_coverage_deficit(events)` before returning any
  `Valid` cert. If the check errors, downgrade the status.
- Aggregation via `Validity::aggregate` using the widened
  `EdgeVerdict::Match { recorded_trust_class, observed_trust_class,
  observed_fp }`.
- Wire the six `fixtures/scenarios/` scenarios end-to-end: given the
  scripted input observations, the engine produces a certificate
  matching each scenario's `expected` block.

### Optional Wave-2 side quests

These can slot into any of the above worktrees without a new one:

- **Mutation testing.** Add `cargo mutants` gating to CI targeting
  `Certificate::check_invariants`, `Certificate::check_coverage_deficit`,
  and `Validity::aggregate`. Any surviving mutant that flips a
  `stale/unknown → valid` outcome fails CI. This is the
  eval-adversary's #1 recommendation for closing the correctness gap.
- **CLI wire-up (blocks on W4).** `freshdag check <artifact>` — reads
  the certificate, runs the engine's check, prints the result with
  exit codes `0=valid`, `1=stale`, `2=unknown`, `>2=tool error`.
- **CLI golden test (blocks on CLI).** Snapshot the 30-second demo
  output at `tests/cli-golden/demo.txt` so a reflow of CLI text is a
  detectable regression.

## 4. Verification (per workstream)

Every Wave 2 workstream is reviewed by a **different agent than the
primary implementer** (per verifier charter). Findings are
categorized:

- **BLOCKING** — must be resolved before merge to `main`.
- **IMPORTANT** — should be resolved; carrying to a follow-up
  requires an issue.
- **NON-BLOCKING** — informational.

The verifier report format is in `.claude/agents/verifier.md`. Do
not merge with unresolved BLOCKINGs.

## 5. Adversarial triple at the end of Wave 2

After all Phase B workstreams are integrated, dispatch three
adversarial reviewers in parallel (same pattern as Wave 1 Phase E).
The specific questions to answer this time:

**Architecture:**
- Does the anti-thrash implementation actually prevent the flap
  scenarios enumerated in `docs/contracts/probe-contract.md
  §Anti-thrash Protocol`? Give a specific input that would flap and
  show the code rejects it.
- Did the Claude adapter accidentally leak hook-payload shapes into
  `freshdag-core`? Grep for `hook`, `PreToolUse`, `PostToolUse`,
  `transcript_path` in core.
- Does W3's derived-graph layout survive an S1 type change (e.g.,
  adding a `DependencyKey` type) without a rewrite?

**Correctness:**
- Can the engine emit a `Valid` cert without calling
  `check_coverage_deficit`? Trace every path from `Engine::check` to
  cert emission.
- What happens when a probe returns `Unknown { retryable: true }`
  during a scheduled recheck — does the engine correctly hold the
  previous status, or does it emit `unknown` and lose the last-known
  fingerprint?
- Is HTTPS probe cost model acceptable at scale? Estimate cost for
  a certificate with 20 HTTP deps at ETag revalidation vs. one
  content-hash fallback.

**Novelty:**
- Did Wave 2 push FreshDAG into the firewall (`docs/NOVELTY.md §3`)?
  Specifically: is the append-only store now indistinguishable from
  a generic tracing log; is the engine now doing what LangGraph
  checkpoints do; is the HTTPS probe now a semantic cache?
- Any new collision to add to NOVELTY.md §1 that Wave 2's actual
  shape now overlaps with?

## 6. Do NOT ship in Wave 2

The following are deferred per `docs/BUILD_PLAN.md §7`. Do not sneak
them in via a workstream:

- `refresh-on-stale` closed-loop mode. v0.5 candidate.
- Attio / Clay integration. Post-v0.
- Comparator implementations. Deferred to the recomputation
  workstream.
- Windows observer.
- macOS syscall observation. Users on macOS get honest `unknown`;
  do not fake it.
- Visual UI (`apps/web/`).
- Remote store.
- Cross-machine event routing.
- Distributed execution.

## 7. Escalation conditions

Escalate to the human on any of:

- A contract change that is not strictly additive.
- Any invariant strain — especially any code path where `Unknown`
  could become `Valid`.
- Any novelty-firewall concern (a Wave 2 feature that drifts toward
  generic tracing, provenance graphs, semantic caching, workflow
  authoring, or "make for X" positioning).
- Any proposal to push to `origin/main`. Push is human-authorized
  per event, not standing.
- Any proposal to introduce an async runtime, a database, or a new
  workspace member.
- Any decision on the four open architectural questions from the
  Wave 1 completion report:
  1. `ProbeRegistry` anti-thrash state — in-memory vs persistent.
  2. `DependencyKey` shape — String, typed struct, or trait.
  3. Envelope strictness — should `IrEvent` use
     `#[serde(deny_unknown_fields)]` (an ADR).
  4. Should `freshdag-store` fold into `freshdag-engine` for v0.

## 8. Operating principle

FreshDAG's value is not that it can observe many things. Its value
is that when it says **this artifact is still valid**, that
statement has a precise, inspectable reason behind it. Wave 1 made
that promise machine-checkable at the certificate boundary
(`check_invariants`) and at the event-stream boundary
(`check_coverage_deficit`). Wave 2 extends the promise through the
engine, the store, the Claude adapter, and the HTTPS probe —
without weakening either check.

Build the minimum trustworthy substrate. Do not maximize concurrency
for its own sake. Maximize independent useful work with a clean
integration path.

## 9. Wave 2 execution DAG (recommended order)

```
[Phase A: ReasonCode + typed reasons]
              │
     ┌────────┼─────────┬────────────┐
     │        │         │            │
     ▼        ▼         ▼            ▼
[W1 Claude][W2 store  [W5.2 HTTPS  [Mutation
 adapter]   append]    probe]       testing]
              │
              ▼
        [W3 derived
         dep graph]
              │
              ▼
        [W4 engine
         validity]
              │
              ▼
        [W7 CLI wire-up]
              │
              ▼
    [CLI golden test]
              │
              ▼
    [Adversarial triple]
              │
              ▼
    [Wave 2 completion report]
```

Independent throughout: W1 (Claude adapter emits v0.1 IR without
engine dependency), mutation testing (targets what already exists).
Everything else has the visible dependencies above.
