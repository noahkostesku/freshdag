# ADR 0009: Two reason codes — mid-computation dependency conflict, and unprobed volatile

- **Status:** accepted
- **Date:** 2026-08-16
- **Deciders:** architect
- **Consulted:** `store-engineer` (owns `EdgeConflict`), `graph-engineer`
  (owns the emitter), `core-engineer` (owns the `ReasonCode` enum and
  `schemas/certificate/`), `probe-engineer` (owns the probe contract's
  volatile clause), `eval-engineer` (owns the fixtures).
- **Extends:** ADR 0006 (closed reason-code vocabulary). Does not
  supersede it — the closedness argument is unchanged; this ADR adds two
  members and the process for doing so is exactly the one ADR 0006
  anticipated.
- **Requires:** a `contract-change`-labelled PR. Two schema enums
  (`schemas/certificate/v0.1.json`, `schemas/scenario/v0.1.json`), the
  `ReasonCode` enum in `freshdag-core`, and
  `docs/contracts/certificate-contract.md §Reason Codes`.

## Context

Wave 2 surfaced two edge conditions that the certificate cannot
currently express. Both are cases where the machine-readable output is
either wrong or under-determined, and in both the only thing
distinguishing them from a neighbouring case is `detail` — which
`docs/EVALUATION.md §2` forbids any assertion from depending on. A
distinction no test can make is not a distinction the contract has.

### A. The world moved *during* the computation

`freshdag_store::graph::EdgeConflict` records the same dependency
observed twice within one computation with different fingerprints. The
store keeps the first observation as the edge and records the divergence,
explicitly deferring the meaning to the engine. The engine
(`engine.rs`) emits a `graph.edge_conflict` diagnostic and then lets the
probe answer normally.

That is not sound. The certificate records fingerprint `f₁`; the
computation may have consumed `f₂`. A later probe returning `Match` on
`f₁` proves the world still matches *an* observation — not the one the
artifact was derived from. Treating that `Match` as evidence of validity
lets an artifact whose input demonstrably changed mid-flight report
`valid`. That is invariant #7 by a side door: evidence about the wrong
proposition is not evidence.

Nor is it `Drift`. `Drift` asserts the world moved *since* production,
which is a different, checkable claim. Here we do not know which value
was used, and saying "drift" would be as false as saying "valid".

### B. A volatile dependency inside its TTL with no probe

`ARCHITECTURE.md §7` licenses `volatile` inside TTL → `Likely Valid`
with no probe (see the §7 amendment landing with this ADR). The engine
implements it and emits
`ReasonCode::TrustClassVolatileCapsAtLikelyValid` with
`detail: "within-declared-ttl"`.

But that same code is emitted when a probe *did* run and matched. The
two situations differ in exactly the way FreshDAG claims to care about —
one has a probe result behind it and one does not — and the only signal
separating them is the non-normative `detail` string. This is the single
place in the system where "no probe ran" is not `Unknown`; it is the
last place that distinction should be invisible.

## Decision

Add two edge-scoped reason codes to the closed vocabulary.

### 1. `dependency-changed-during-computation`

- **Rust:** `ReasonCode::DependencyChangedDuringComputation`.
- **Scope:** edge. `dependency_key` names the conflicted dependency.
- **Emitted when:** the computation's `ComputationNode.conflicts`
  contains an entry for this dependency.
- **Verdict:** `EdgeVerdict::Unknown`. Not `Drift`.
- **Precedence:** it short-circuits. The edge is decided before probe
  selection, alongside the TTL check, because no probe result could
  change the answer — the ambiguity is about which fingerprint the
  artifact was derived from, and no amount of information about the
  present world resolves it.
- **`detail`:** may carry both fingerprints; nothing may assert on it.

The existing `graph.edge_conflict` diagnostic stays. The log schedules;
the certificate explains.

### 2. `volatile-within-ttl-unprobed`

- **Rust:** `ReasonCode::VolatileWithinTtlUnprobed`.
- **Scope:** edge.
- **Emitted when:** the edge is `volatile`, the declared TTL has not
  elapsed, and no probe was consulted.
- **Verdict:** `EdgeVerdict::Match`, capped at `LikelyValid` exactly as
  today. Nothing about the aggregation changes.
- **`TrustClassVolatileCapsAtLikelyValid` narrows** to its honest
  meaning: a probe ran, matched, and the recorded class caps the result.

Naming is the `core-engineer`'s to finalize in the contract-change PR;
the semantics above are the decision.

## Consequences

- An artifact with a mid-computation conflict can no longer be `valid`.
  Correct, and this is a behaviour change that will move at least one
  scenario if a fixture exercises it — none does today, which is itself
  the finding.
- `freshdag why` gains two answers a user could not previously get:
  "your input changed while the agent was reading it" and "nothing
  checked this; you are inside a declared TTL."
- Reason-code count goes 10 → 12. ADR 0006's ordering rule
  (edge-scoped before artifact-scoped) is unchanged; both new codes are
  edge-scoped and sort with their `depends_on[]` entry.
- Two schema enums, one Rust enum, and
  `crates/freshdag-core/tests/scenario_wellformed.rs`'s explicit code
  list must be updated together. `ReasonCode::as_wire_str` is exhaustive
  by construction, so the compiler catches the Rust half.
- **Fixtures required** before this ships. `eval-engineer` owns
  `docs/EVALUATION.md` and adds these to the §2 v0 backlog as part of
  the contract-change PR:
  - `dep-changed-mid-computation` — the same `fs.read` emitted twice
    within one computation with different hashes; assert
    `status.value == "unknown"` and
    `status.reasons[0].reason == "dependency-changed-during-computation"`.
    Closest existing backlog entry is `mcp-nondeterministic-response`;
    this is the file-level version and is cheaper.
  - `volatile-unprobed-within-ttl` — extends the existing
    `volatile-external-dep` scenario with the no-probe-registered arm;
    assert `likely-valid` and `volatile-within-ttl-unprobed`.

## Rejected alternatives

- **Leave the conflict as a diagnostic only (Wave 2's behaviour).**
  Rejected. It permits `valid` on an artifact whose input provably moved
  mid-computation, and a diagnostic in the log is not visible to
  anything that consumes a certificate — which is the portable artifact
  §2 now rests on.
- **Treat the conflict as `Drift`.** Rejected: it asserts something we
  did not observe, and it would make the artifact `stale` rather than
  `unknown`. `stale` is a claim; `unknown` is the absence of one, and
  the absence is what we have.
- **Have the store keep the *last* observation instead of the first.**
  Rejected as an alternative to a reason code: it changes which
  fingerprint we are wrong about, not whether we are. It is also a store
  contract change with no independent justification.
- **Distinguish the volatile cases with `detail` only (status quo).**
  Rejected: `docs/EVALUATION.md §2` forbids pass/fail assertions on
  `detail`, so the distinction would be permanently untestable, and
  invariant #13 requires public contracts be testable.

---

## Amendment, 2026-08-16 — bounding the TTL as evidence (verifier D2)

Raised by the `verifier` at Wave 2 rejection, ruled by `architect` the
same day. Appended rather than superseding: the decision above stands
unchanged and this constrains it.

**The question.** `volatile-within-ttl-unprobed` makes the branch
legible, but leaves a never-probed dependency at `LikelyValid` and
therefore at exit 0 under `--accept-likely-valid`. Is legibility
sufficient?

**Ruling: legibility is necessary but not sufficient. There is a floor
— on the unbounded parts of the argument, not on the trust class.**

`ARCHITECTURE.md §7`'s amendment justifies this branch on the grounds
that the producer's declared TTL is *present evidence*. That argument is
sound only if the declaration is bounded and its timestamp is real.
Neither is checked today, and the verifier found both holes:

- `ttl_seconds: 1000000000` (~31 years) behaves identically to
  `ttl_seconds: 3600`. A producer can purchase unlimited freshness by
  declaring a large integer.
- `observed_at` is never sanity-checked against now. A `probe.checked`
  dated 2099 satisfies `now > expires_at == false` and stays
  `likely-valid` forever.

So the RFC 9111 analogy in §7 was doing more work than it can bear.
HTTP's `max-age` arrives from an origin over an authenticated channel,
and RFC 9111 §4.2.3 requires the cache compute `current_age` including
transit, with a `Date` it validates. FreshDAG has none of that
machinery, so an unbounded self-declared lifetime is not the same
object. Four constraints restore the argument:

1. **A declared TTL is evidence only within a configured maximum.**
   The engine gains `max_volatile_ttl`, conservative default 24h.
   Beyond it the edge is `Unknown` with `TtlExpired`, whatever the
   producer declared. A `volatile` dependency is by definition one with
   no trustworthy freshness signal; a day is already generous.
2. **A future `observed_at` is not evidence.** If `observed_at` exceeds
   now beyond a small skew tolerance, the edge is `Unknown`. TTL
   arithmetic must not go negative-fresh. This promotes the `clock-skew`
   entry in `docs/EVALUATION.md §2`'s backlog from backlog to blocker.
3. **An exit-code floor, expressed through the new code rather than a
   new status.** `--accept-likely-valid` MUST NOT accept a certificate
   whose likely-valid basis includes `volatile-within-ttl-unprobed`.
   Opting in requires a second, explicit flag (`--accept-unprobed-volatile`
   or equivalent); a user who wants "nothing ever checked this, ship it"
   has to say so. This is why legibility is not merely cosmetic: the
   reason code is the mechanism that makes the floor implementable
   without inventing a fifth `ValidityStatus`, which `LikelyValid`
   otherwise describes correctly. CLI exit codes are stable ABI
   (`docs/OWNERSHIP.md`), so `integration-engineer` co-signs.
4. **The fixture stops enshrining one arm.**
   `fixtures/scenarios/volatile-external-dep` is the only non-file
   scenario with no `input_probes`, so it currently pins this branch as
   *the* expected volatile answer. It gains a probed sibling, the
   unprobed arm pins `volatile-within-ttl-unprobed`, and two new
   fixtures cover the over-long TTL and the future timestamp.

What is **not** changed: the class still caps at `LikelyValid` rather
than falling to `Unknown`. Demoting it wholesale would make `volatile`
unusable — no probe can ever exist for `time.now()` — and would undo
ADR 0004's central argument. The fix is to bound the evidence, not to
discard it.

Separately noted for the implementer: `engine.rs:350-363` places this
licensed verdict in the `Err(no_probe)` arm of arbitration, which
`ARCHITECTURE.md §7` now forbids. The code motion is already dispatched
by the `release-manager` and is not re-litigated here.
