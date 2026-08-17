# ADR 0013: The core determinism boundary, and the engine-clock carve-out

- **Status:** accepted
- **Date:** 2026-08-17
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
