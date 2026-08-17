# ADR 0013: The core determinism boundary, and the engine-clock carve-out

- **Status:** accepted — merged as `accepted` prematurely; `architect`
  review supplied 2026-08-17 (retrospective), ratifying **Decision 2
  outright and Decision 1 with conditions**. See §Amendment.
- **Date:** 2026-08-17
- **Reviewed by:** `architect`, 2026-08-17 — ratify with conditions
- **Deciders:** human owner (Noah Kostesku), by delegation
- **Consulted:** `architect` review of the 2026-08-17 `contract-change`
  PRs (#8–#15), condition 1 and condition 2 on PR #11; `ARCHITECTURE.md`
  §4–§5; `docs/contracts/execution-ir.md`
- **Records:** a boundary drawn in PR #11 and merged without an ADR

## Context

PR #11 moved the `Clock` and `IdGen` traits, and their deterministic
implementations `FixedClock` and `SeededIdGen`, out of
`freshdag-adapter-claude` and `freshdag-observer` and into
`freshdag-core`. The ambient implementations that read the environment
— `SystemClock`, `UuidV7Gen` — stayed in each producer crate.

The PR executed no ruling. It was flagged as "architect's call" and
merged on the owner's direction, so the boundary is currently drawn in
code and justified nowhere. The subsequent `architect` review signed it
off **with conditions**, and found that the justification the PR gave
was the wrong one and that the change had introduced a collision it did
not notice. This ADR records the boundary, replaces the argument, and
closes the naming trap.

## Decision 1: the port is core's; the read of the world is the edge's

`freshdag-core` owns the `Clock` and `IdGen` **traits** and their
**deterministic** implementations. Every producer crate keeps its own
ambient pair.

### The argument, stated correctly

PR #11 justified this by saying `FixedClock` and `SeededIdGen` are pure
— a base instant plus a counter — "so they belong to the domain model as
naturally as any other value type."

**That argument is rejected**, though the conclusion it reached is
right. Purity is not a membership criterion for `freshdag-core`; it
would prove too much, admitting any pure helper anyone cared to write.

The load-bearing argument is about **ownership of the values**:

> `IrEvent::event_id` and `IrEvent::ts` are core-owned fields.
> `docs/contracts/execution-ir.md §Event Envelope` makes `event_id` a
> UUIDv7 and gives it a contractual per-producer total order. A
> contract core owns, over a field core owns, is core's to enforce — so
> the generator of those field values is core's business by exactly the
> logic that puts `Fingerprint` there.

The ambient implementations are excluded by a different and equally
specific rule: `ARCHITECTURE.md` says of this crate that "it has no I/O
and no dependency on any runtime." A `SystemClock` calling `now_utc()`
reads the environment. That is the whole distinction — not purity in
the abstract, but *whether the thing reads the world*.

Duplicating a six-line ambient pair per producer crate is therefore not
a failure to deduplicate. It is the boundary doing its job.

### Invariants

Neither trait mentions Claude Code, fsatrace, or any runtime, so
invariant #14 (adapters do not leak into the core) holds. No core code
path calls `now_utc()`, so the no-I/O rule holds.

## Decision 2: two clocks, permanently, under different names

The workspace has two clock abstractions. They are **not** to be
merged, and after this ADR they are not to be confused either.

| | emission clock | evaluation clock |
|---|---|---|
| Where | `freshdag_core::determinism::Clock` | `freshdag_engine::EvalClock` |
| Answers | "stamp the next event" | "what time is it now?" |
| Advance | auto-advances once per call | idempotent; moved explicitly |
| Bounds | `Debug` (`!Sync`, holds a `Cell`) | `Debug + Send + Sync` |
| Test double | `FixedClock` | `FrozenClock` |

They cannot be unified even in principle. Core's `FixedClock` holds a
`Cell` and so cannot satisfy the engine's `Send + Sync` bound, which
`Engine` requires because it is shareable and `check` takes `&self`. And
an auto-advancing clock would be actively wrong in the engine: two TTL
comparisons within a single `check` would disagree about the present.

### The trap this closes

After PR #11 the workspace contained two public traits named `Clock`
with incompatible bounds, two public types named `FixedClock` with
opposite advance semantics, and three `SystemClock`s. `freshdag-cli`
depends on both crates, so both names were in scope for anyone working
there. The three rejected alternatives recorded in #11's merge comment
never mention the engine's clock at all — the collision was not weighed,
because it was not seen.

Nothing was unsound; the compiler separates the two by bounds. The cost
is that the distinction was discoverable only by compiler error, and a
future agent reaching for "the `FixedClock`" would have had to find out
which one it had by failing to build.

**Therefore:** `freshdag_engine::Clock` → `EvalClock`, and
`freshdag_engine::FixedClock` → `FrozenClock`. The names now state which
question the clock answers.

`SystemClock` is deliberately **not** renamed. The multiple
`SystemClock`s share one meaning — read the wall clock — and differ only
in which trait they satisfy. Same name for the same concept is not a
trap; same name for different concepts was.

## Consequences

- `freshdag-engine`'s public API renames two items. Purely mechanical:
  the types are used only within `freshdag-engine`, and `freshdag-cli`
  references neither directly.
- The emission/evaluation distinction is now documented at the
  definition site in `crates/freshdag-engine/src/clock.rs`, not only
  here.
- **This is not a contract change.** No `docs/contracts/*.md`, no
  `schemas/*`, and none of the core types named in
  `.claude/rules/architecture.md` are touched. `Clock`/`IdGen` fall
  outside that list, and no wire form moves. The `architect` review
  ruled on this question directly when it was raised by PR #11.
- Decision 1 is what makes per-producer identifier seeding core's
  responsibility rather than each producer's. That followed as a
  separate change: unifying the two generators had merged two
  accidentally-disjoint identifier spaces into one shared counter, so
  both conformance harnesses minted identical `event_id`s.
  `SeededIdGen::for_producer` restores disjointness deliberately, and
  core derives the tag from the producer name rather than holding a
  registry of producer names — which would breach invariant #14 and
  Decision 1 alike.

## Still open

**Test doubles ship undecorated in core's public API.** This is
condition 3 of the `architect` review of PR #11, and this ADR does not
resolve it. `SeededIdGen`'s own documentation advertises that its output
is "indistinguishable from real UUIDv7s to any consumer that only
inspects the version" — which is precisely what would make accidental
production use undetectable at the point of use. The candidate remedies
are a feature gate or a rename to `ConformanceClock` / `ConformanceIdGen`.
Recorded here so it is not lost; not decided here.

## Rejected alternatives

- **A separate `freshdag-determinism` crate.** Defensible, and it buys
  nothing: every producer already depends on `freshdag-core`, so the
  crate would add a manifest and a dependency edge to move code that has
  no other consumer.
- **Leaving the duplication in the two producer crates.** This is what
  PR #11 fixed, and the shape bug it fixed is the argument against
  reverting: the observer's generator emitted `Uuid::from_u128(counter)`
  — version nibble 0, no variant bits, not a UUIDv7 by any reading —
  while `execution-ir.md` called the field a UUIDv7. Two copies of a
  contract-bearing implementation drifted, and only one was right.
- **Merging the engine's clock into core's.** Impossible: `!Sync` versus
  `Send + Sync`, and auto-advance versus idempotent. See Decision 2.
- **Keeping both named `Clock` and relying on the compiler.** Rejected:
  soundness is not the issue, discoverability is. `.claude/rules/`
  exists because this repo expects agents to read names before they read
  bounds.

## Note on authority

The `architect` review named itself as approver for this ADR with
`core-engineer` drafting. Neither role was filled: it was drafted in the
owner's main session on the owner's direction, and **`architect` sign-off
is owed and has not happened.**

If that review disagrees, the claim to attack is Decision 1's
replacement argument — that ownership of `IrEvent`'s fields, rather than
purity, is what puts the generator in core. Decision 2 is a naming
change and survives either way.

---

## Amendment (2026-08-17): the ownership argument is narrowed, and three conditions

Supplied by the retrospective `architect` review the ADR asked for.
**Both decisions stand. The boundary is in the right place.** What
changes is the argument for Decision 1, which as written proves too
much, and three conditions attach.

### The replacement argument does not survive as stated

Decision 1 rejects PR #11's purity argument because it "would prove too
much, admitting any pure helper anyone cared to write." That rejection
is correct. The substitute proves too much in the opposite direction:

> A contract core owns, over a field core owns, is core's to enforce —
> so the generator of those field values is core's business.

Applied consistently this puts far too much in core.
`IrEvent::producer` is a core-owned field with a contractual shape that
`execution-ir.md §Event Envelope` calls "load-bearing rather than
decorative"; its generator is each adapter. `IrEvent::session_id` and
`IrEvent::payload` are core-owned fields whose generators are runtime
readers. `Fingerprint` — the ADR's own analogy — is a core type whose
*generator* is `freshdag-adapter-claude`'s `DiskContent`, which hashes a
file off disk. Core owns those types and their validity predicates. It
does not own the things that produce their values.

So "core owns the field, therefore core owns the generator" is false as
a general rule, and citing it will license the next
should-this-be-in-core argument in the wrong direction.

### The rule that does hold

The ADR states the correct rule and then demotes it to a supporting
note ("a different and equally specific rule … not purity in the
abstract, but *whether the thing reads the world*"). Promote it, and
sharpen it:

> **Core owns the construction of a value exactly when satisfying the
> contract for that value is a total function of the contract itself —
> requiring no read of the world and no knowledge of any runtime.**
> Every other constructor lives at the edge that does the reading.

`SeededIdGen` qualifies: `execution-ir.md` fully specifies `event_id`
(UUIDv7 shape, per-producer total order), and a generator meeting that
specification needs nothing but the specification. `FixedClock`
qualifies for the same reason. `SystemClock`, `UuidV7Gen` and
`DiskContent` do not: each reads the world. `Fingerprint`'s
canonicalization and comparison do qualify, and are in core, which is
why the analogy works once it is stated this way.

This rule reaches the same verdict on `Clock`/`IdGen`, survives the
`producer` / `session_id` / `DiskContent` counterexamples, and does not
readmit arbitrary pure helpers — a helper nobody's contract demands is
not a total function of any contract.

`SeededIdGen::for_producer` is worth checking against it explicitly,
since it is the one place core consumes a producer's identity. It takes
an opaque string and folds it; it holds no registry, knows no runtime,
and would behave identically for a producer that does not exist.
Invariant #14 holds.

### Condition 1 — the definition site still carries the rejected argument

`crates/freshdag-core/src/determinism.rs` §"What lives here, and what
deliberately does not" says `FixedClock` and `SeededIdGen` "are pure —
a base instant plus a counter — so they belong to the domain model as
naturally as any other value type." That is verbatim the argument
§Decision 1 rejects.

§Consequences claims "the emission/evaluation distinction is now
documented at the definition site." Decision 2 was documented there;
Decision 1's superseded justification was left standing. A future agent
reading the code — which `.claude/rules/` says is the first thing they
do — gets the rejected argument, not this one.

**Required:** `core-engineer` replaces that paragraph with the rule
above and cites this ADR. Documentation-only; no behaviour changes.

### Condition 2 — "not a contract change" is right, and is not the whole answer

§Consequences is correct that no `docs/contracts/*.md`, no `schemas/*`,
and none of the types enumerated in `.claude/rules/architecture.md` are
touched, so the contract-change process did not apply. Sustained.

It is still a **breaking public API change to `freshdag-engine`**, whose
owner is `graph-engineer` with `core-engineer` and `verifier` as
reviewers (`docs/OWNERSHIP.md`), and `freshdag-cli` carries a mutual
sign-off requirement for public-API-shape changes. Neither happened. The
cost is nil — zero external consumers, and the CLI references neither
name — so this is recorded, not reopened. The general point is in
`.claude/rules/architecture.md` §"Reviews that are owed even when the
contract-change process does not apply".

### Condition 3 — `accepted` was premature, and the error was a repeat

This ADR merged with `Status: accepted` while its own closing section
said "`architect` sign-off is owed and has not happened." An ADR cannot
be accepted and pending review at once.

That is the identical bookkeeping error `architect` had already found in
ADR 0012 and required corrected, hours earlier the same day. The
correction was applied to 0012's text and did not propagate to the next
ADR written. A one-line rule now exists in
`.claude/rules/architecture.md` so it is checkable rather than
remembered: **an ADR whose own text says review is owed merges as
`proposed`.**

The status line above is updated rather than rewritten, so the record
shows what happened.

### Not reopened

- **Decision 2 (`EvalClock` / `FrozenClock`).** Ratified without
  conditions. Two public traits named `Clock` with incompatible bounds
  and two public `FixedClock`s with opposite advance semantics, both in
  scope in `freshdag-cli`, is a real trap; the names now state which
  question each clock answers. Leaving `SystemClock` unrenamed is right
  for the reason given — same name for the same concept.
- **The impossibility argument for two clocks** (`!Sync` versus
  `Send + Sync`; auto-advance versus idempotent). Checked and correct.
- **§Still open — undecorated test doubles in core's public API.**
  Remains open, and is now tracked in `docs/BUILD_PLAN.md §6.3` rather
  than only here. `SeededIdGen`'s documentation advertising that its
  output is "indistinguishable from real UUIDv7s" is an accurate
  statement of a hazard with no guard on it.
- **The 16-bit producer tag.** `4ece576` corrected the overstated
  disjointness claim and pinned the fail-safe behaviour. The remaining
  claim — that a collision yields duplicate `event_id`s, which
  `linearize_checked` detects and the engine refuses to certify over —
  is the right shape: no certificate rather than a wrong one.
