# ADR 0016: Producer obligations follow the declared role, not the crate name — `freshdag mark` is a producer

- **Status:** accepted
- **Date:** 2026-08-17
- **Deciders:** `architect`
- **Consulted:** `docs/contracts/execution-ir.md §Event Envelope`,
  §Coverage Declarations; `docs/contracts/adapter-contract.md`
  §Responsibilities, §Testing; `.claude/rules/architecture.md` §"Adding
  a new adapter"; `crates/freshdag-core/src/determinism.rs`; ADR 0013;
  commit `b052a98`
- **Owners who must acknowledge:** `integration-engineer` (owns
  `freshdag-cli`), `claude-adapter` (precedent for adapters),
  `eval-engineer` (owns `fixtures/**`)
- **Contract change:** **not made here.** This ADR rules that two
  clauses of `execution-ir.md` and `adapter-contract.md` are
  insufficient and dispatches the edits through the contract-change
  process. The `architect` does not edit contracts unilaterally,
  including their own (`CLAUDE.md`).

## Context

`freshdag mark <path>` landed on 2026-08-17 (`1f76830`) and closed the
artifact half of W11: nothing else emits `artifact.produced`, so without
it a store has no artifact to check. The design judgement behind it is
right and is not in question — which file is "the artifact" is a **user
declaration, not an observation**, and an adapter that promoted every
write would be asserting something nobody told it.

The problem is what `mark` became in the process. `freshdag-cli` now:

- mints and appends canonical IR events (`artifact.produced`),
- publishes its own `CoverageManifest` with `role: ProducerRole::Adapter`
  into the store's `coverage.jsonl`,
- implements an **attribution rule** — the artifact belongs to the
  computation of the most recent recorded `fs.write` of the path — that
  is load-bearing for invariant #9,

while:

- calling `Uuid::now_v7()` and `OffsetDateTime::now_utc()` directly
  (`mark.rs:264`, `mark.rs:272`), with no injected `Clock` or `IdGen`,
- shipping **no** fixtures under `fixtures/adapter-conformance/`,
- appearing nowhere in `docs/OWNERSHIP.md` as a producer,
- emitting `producer: "freshdag-cli"`, which matches none of the three
  shapes `execution-ir.md §Event Envelope` enumerates.

`b052a98` then found three defects in the attribution rule in a single
verifier pass — an unverifiable write could claim authorship of another
session's bytes (a direct invariant-#9 violation), selection used
physical rather than canonical log order, and the path comparison was
realpath-versus-lexical, which broke `scripts/demo.sh` on the project's
own development platform. Three defects in a rule that exists only in a
doc comment.

## Decision 1: obligations attach to producers, not to crate names

`adapter-contract.md` scopes itself to "every crate named
`freshdag-adapter-*`". `freshdag-cli` is not so named, so by the
contract's own letter none of §Responsibilities applies to it. That
scoping is wrong, and reading it literally is how a crate acquired
producer powers without producer duties.

**Ruling: a crate that emits canonical IR events into a store and
publishes a coverage manifest is a producer, and the producer
obligations apply to it in full, whatever it is called.** The manifest's
`role` field — which ADR 0006 made required precisely so vantage point
could not be inherited by default — is the discriminator, not the crate
name.

`mark`'s choice of `role: Adapter` is **correct** and is ratified. It
observes nothing; it translates a user declaration into IR. Only
`Observer` discharges the subprocess observation obligation, so an
`Adapter` declaring `artifact.produced` discharges nothing, which is the
safe reading.

**`mark` must therefore become a first-class producer.** The concrete
obligations follow.

## Decision 2: the injected `Clock` and `IdGen` are required

`crates/freshdag-core/src/determinism.rs` opens by saying a producer's
emission path "never calls `OffsetDateTime::now_utc()` or
`Uuid::now_v7()` directly. It calls a `Clock` and an `IdGen` its caller
supplies." `mark` does exactly what that module exists to forbid, in the
same repository, three commits after ADR 0013 ratified the boundary.

This is not pedantry, and there are two independent reasons:

1. **It is what makes conformance fixtures possible at all.** Ambient
   time and identity are incompatible with the golden-file harnesses
   `adapter-contract.md §Testing` requires. `mark` has no fixtures
   *because* it has no injected sources; the two facts are the same
   fact.
2. **`Uuid::now_v7()` does not supply what the envelope promises across
   processes.** `execution-ir.md §Ordering` requires a per-producer
   total order on `event_id`. `now_v7`'s monotonicity guarantee is
   within a generator context; `mark` is a fresh process per
   invocation, so two marks inside the same millisecond can order
   arbitrarily. Small, real, and structurally identical to the
   ordering defect `b052a98` already fixed one layer up.

**Required:** `mark` takes an injected `Clock` and `IdGen`;
`main.rs` supplies the ambient pair, which stays in `freshdag-cli` per
ADR 0013 Decision 1 (each crate owns its own read of the world).

## Decision 3: fixtures under `fixtures/adapter-conformance/cli-mark/`

`.claude/rules/architecture.md` requires "at least one fixture under
`fixtures/adapter-conformance/<name>/`" for a new adapter, and
`adapter-contract.md §Testing` requires the coverage declaration be
machine-checked against a golden set of emitted events. `mark` has
neither. The Claude adapter ships thirteen such fixtures.

**Required, at minimum,** one fixture per branch `b052a98` had to
repair, because these are demonstrated failure modes rather than
imagined ones:

- a write carrying no recorded hash → **refused** (this is the
  invariant-#9 case, and the previous test
  `a_write_with_no_recorded_hash_is_markable` asserted the inverse),
- two writes of the same path in non-canonical physical order → the
  canonically-later one is selected,
- a lexical recorded path against a realpath target (the
  `/var` → `/private/var` case) → matched,
- current bytes differing from the recorded write → refused,
- a successful mark → golden `artifact.produced` envelope.

Owner: `integration-engineer` authors; `eval-engineer` reviews
(`docs/OWNERSHIP.md`).

## Decision 4: the attribution rule belongs in a contract, not a doc comment

"The artifact belongs to the computation of the most recent recorded
`fs.write` of this path, and `mark` refuses unless that write carries a
content hash matching the file's current bytes" is a **semantic rule
that decides invariant-#9 attribution**. It currently exists in a
module doc comment and in one entry of the manifest's `capabilities`
map, which nothing reads.

Every other rule of that weight is in a contract.
`adapter-contract.md §Identity Model` governs `computation_id` and says
changes to its semantics are a contract change; artifact attribution —
which computation a *produced artifact* is bound to — is not covered
anywhere.

**Required:** a §Artifact Attribution clause in `adapter-contract.md`,
stating the rule, its refusal conditions, and that changing it is a
contract change. Authored by `integration-engineer`, merged through the
contract-change process with `architect` sign-off as contract owner.

## Decision 5: the producer name stays; the contract clause widens

`execution-ir.md §Event Envelope` enumerates
`"freshdag-adapter-<runtime>" | "freshdag-observer-<backend>" |
"freshdag-probe-<scheme>"` and says, immediately below, that "`producer`
is matched by exact string, so the shape above is load-bearing rather
than decorative." `freshdag-cli` fits none of the three.

Read carefully, what is load-bearing is that the string on an event is
**literally** the string in the registered manifest —
`check_coverage_deficit` compares exact strings, and a mismatch means
`producer-missing-from-coverage` and uninterpretable silences. `mark`
registers `freshdag-cli` and emits `freshdag-cli`, so attribution works.
The deviation is from a naming *convention*, not from the matching rule.

**Ruling: do not rename the producer.** The log is append-only
(invariant #4). Live stores already contain `freshdag-cli` events —
`docs/DOGFOOD.md` session 1 is one — and renaming a producer orphans
recorded history from the manifest that interprets it, converting a
cosmetic mismatch into a real attribution failure. Renaming to fit a
convention would break the invariant to satisfy a doc.

**Required instead:** the §Event Envelope clause is edited to admit a
fourth shape (`freshdag-cli` is a producer of user declarations) and to
state plainly which part of the block is normative — the exact-string
match — and which is convention. `architect` owns
`execution-ir.md`; the edit goes through the contract-change process
like any other.

## Decision 6: the "not partial" claim on `mark`'s manifest is wrong, and is latent

`mark`'s manifest declares `partial: {}` with the comment "`mark` emits
exactly the declaration the user made, and nothing else is in its scope
to miss."

That reasoning defines the scope as the declaration rather than as the
event kind. The manifest declares `emits: ["artifact.produced"]`, and
`covers()` reads `emits`, so what it tells a consumer is: *this producer
covers artifact production, completely.* It does not. It emits an
`artifact.produced` only for files a human typed a command about, and is
blind to every other artifact a computation produced.

Under ADR 0011's vocabulary that is `under-approximates` on
`artifact.produced` — and this is the exact shape of the hole W9 closed:
a producer whose honest blindness is invisible at the
manifest→certificate boundary.

**It is latent, not live.** `artifact.produced` is not in `EFFECT_KINDS`
(`fs.*`/`proc.*`/`net.*` only) and no discharge rule keys on coverage of
it today. So this is a correction, not an emergency.

**Required:** `mark`'s manifest declares
`artifact.produced: { reason: "under-approximates", note: … }`. It costs
nothing today and stops a future rule that keys on artifact coverage
from silently over-trusting this producer.

## Consequences

- `freshdag-cli` gains a producer row in `docs/OWNERSHIP.md` — done in
  the same change as this ADR.
- Four required follow-ups land in `docs/BUILD_PLAN.md §6.3`:
  injected sources, fixtures, the attribution clause, the partial
  declaration. Two of them (Decisions 4 and 5) are contract changes and
  must carry the label and the policy answers.
- The precedent generalises. `freshdag-engine` is scheduled to publish
  its own coverage manifest and append `probe.checked` events (ADR 0007
  / W10). It will become a producer under exactly this ruling, and
  should be built with injected sources and conformance fixtures from
  the first commit rather than acquiring them retrospectively.

## Rejected alternatives

- **Leave it; `mark` is "just the CLI".** Rejected. It mints events a
  certificate is built from and implements an invariant-#9 attribution
  rule that three defects were found in on its first verifier contact.
  The obligations exist for exactly this.
- **Move `mark` into a new `freshdag-adapter-cli` crate to satisfy the
  contract's crate-name scoping.** Rejected: it renames the problem.
  The scoping clause is what is wrong, and a crate boundary drawn to
  satisfy a grep is not a boundary.
- **Rename the producer string to `freshdag-adapter-cli`.** Rejected —
  see Decision 5. Append-only history outranks a naming convention.
- **Have the adapter promote every `Write` to an artifact and delete
  `mark`.** Rejected, and `mark`'s own module documentation gives the
  reason: it would assert a user declaration nobody made. The gap
  `docs/DOGFOOD.md` records — "there was no obvious moment to run
  `freshdag mark`, and nothing prompted for one" — is a workflow
  problem, and the fix for it is not to start guessing.
