# ADR 0014: Cap on missing recipe identity, rather than refusing to seal

- **Status:** accepted — ratified with conditions by `architect` review,
  2026-08-17 (retrospective; the change had already merged). See
  §Ratification.
- **Date:** 2026-08-17
- **Reviewed by:** `architect`, 2026-08-17 — ratify with conditions
- **Deciders:** human owner (Noah Kostesku), by delegation
- **Consulted:** invariant #9; `docs/contracts/certificate-contract.md`
  §Field Rules; ADR 0006 (closed reason-code vocabulary);
  `freshdag-adapter-claude`'s coverage manifest
- **Contract change:** yes — thirteenth `ReasonCode`, both JSON schemas,
  certificate-contract table and changelog

## Context

`certificate-contract.md §Field Rules` requires `produced_by.recipe_hash`
whenever the status is `valid` or `likely-valid`. That follows from
invariant #9: *every artifact is traceable to the computation that
produced it.* An artifact that cannot be tied to a reproducible recipe
has not been shown to be the output of one.

The engine enforced this in `seal()` by **returning an error**
(`EngineError::MissingRecipeHash`) when a certificate reached that state.

For a long time nothing hit it, because no dependency ever verified: the
Claude Code adapter emitted `fs.read` with no fingerprint, every edge was
dropped as `NoFingerprint`, and every artifact came back
`no-dependencies-observed`. Once reads were fingerprinted the path became
live immediately — and the first artifact whose dependencies all verified
produced a **tool error**, exit 3.

That is the wrong answer, and specifically it is wrong for the case that
actually occurs.

`freshdag-adapter-claude`'s own manifest has said all along:

> COMPUTATION IDENTITY GAP: Claude Code exposes no recipe, so
> `recipe_id_or_hash` is synthesized as `claude-code-session:<session_id>`
> … `Computation::recipe_hash` cannot be populated from hook payloads —
> which by invariant #9 **caps such computations below `valid`**.

The manifest described a cap. **Nothing implemented one.** The engine
refused instead, and no code path anywhere turned the absence into a
verdict.

## Decision

**Add `ReasonCode::RecipeIdentityUnavailable` (wire:
`recipe-identity-unavailable`), artifact-scoped, and have the engine cap
at `unknown` instead of refusing to seal.**

`seal()` now attaches the reason and caps whenever `recipe_hash` is
absent and the status would otherwise be `valid` or `likely-valid`. The
existing `MissingRecipeHash` error is retained as a **backstop**: the cap
should make it unreachable, and it stays so that a future edit which
bypasses the cap fails loudly rather than emitting a certificate claiming
more than it can support.

### Why a verdict and not an error

The distinction the exit codes draw is the whole argument. Exit `> 2`
means *"the tool failed; this says nothing about the artifact."* Exit `2`
means *"not provably valid; do not reuse."*

A missing recipe hash is not a tool failure. It is a **fact about the
available evidence**, and it is precisely what `unknown` exists to
express. Reporting it as a tool error told a CI job to ignore a result
that was, in truth, a correct and actionable "I cannot prove this."

It is also **permanent for some runtimes**. An error implies a fault to
be fixed. For Claude Code there is nothing to fix: the runtime exposes no
recipe, so every artifact it produces sits at this ceiling. A tool that
returns a hard error on its own primary adapter's normal output is
mis-modelling the world, not defending an invariant.

### Why a new code rather than reusing one

None of the twelve fits. `no-dependencies-observed` asserts zero
dependencies, which is false here — the dependencies verified.
`coverage-deficit` and `producer-missing-from-coverage` are statements
about *observation coverage*, and this is not an observation gap: nothing
went unobserved. The gap is in the **identity of the computation**, which
no existing code names.

Reusing one would put a false statement in front of a user reading
`freshdag why` — the ADR 0009 defect this project has already corrected
once.

### Why the cap is `unknown`, not `likely-valid`

`likely-valid` also requires `recipe_hash` under §Field Rules, so it is
not available. `unknown` is the correct floor and matches the existing
`cap_at_unknown` used by the coverage codes.

`stale` is deliberately **not** capped. Drift is positive evidence the
artifact is out of date; a missing recipe identity does not make it less
out of date. A test pins this.

## Consequences

- **Wire-additive, reader-breaking.** A consumer holding the pre-change
  `v0.1` validator rejects a certificate carrying the new code, because
  JSON Schema enums are closed. This is recorded in the
  certificate-contract changelog, as is the earlier 10 → 12 widening that
  ADR 0009 made without an entry.
- **The CLI stopgap is removed.** `freshdag check` briefly mapped
  `MissingRecipeHash` to exit 2 itself, with the engine's error text.
  That was a correct signal produced in the wrong layer, and it is now
  deleted: the certificate explains itself.
- **`valid` remains unreachable through the Claude Code adapter.** This
  ADR does not change that; it changes what the tool *says* about it,
  from a crash to a certificate whose reasons name the ceiling.
- The reason count moves 12 → 13. `ALL_REASON_CODES`, both schemas, the
  contract table, and the CLI's prose renderer are updated in the same
  PR.

  **Corrected 2026-08-17 after verifier review.** This originally read
  "each is guarded by a test that fails if one drifts from another",
  which is false and was demonstrated so: a verifier added a fourteenth
  variant to the enum, deliberately omitted it from `ALL_REASON_CODES`
  and both schemas, and the whole core suite still passed. The
  `assert_eq!(ALL_REASON_CODES.len(), 13)` "guard" is a hand-maintained
  count that an enum addition does not perturb, and
  `schema_reason_enums_match_rust` compares the schemas against
  `ALL_REASON_CODES` rather than the enum — so a break at the top of the
  chain hides the whole chain.

  The only real guard is the compiler: the exhaustive `match` in
  `freshdag-cli`'s `prose()` fails to build. The contract table is
  guarded by nothing at all. Closing this properly means deriving the
  variant list rather than hand-maintaining it, or round-tripping every
  schema enum member through serde so the schema becomes the source of
  truth. Not done here; recorded so the claim is not relied on.

## Rejected alternatives

- **Keep refusing, and let each caller reinterpret.** This is what the
  CLI did as a stopgap. It puts a validity judgement in a presentation
  layer, and every future consumer would have to rediscover it. The
  engine owns validity.
- **Synthesize a `recipe_hash` from the session id.** Rejected outright.
  It would manufacture the exact evidence invariant #9 requires, from
  something that is not a recipe, and license `valid` on it. This is the
  invariant-#7 failure mode with extra steps.
- **Let `likely-valid` through without a recipe hash.** Rejected: §Field
  Rules requires it for both, and weakening that to make an adapter look
  better is the wrong direction of fix.
- **Widen the *status* vocabulary instead** (e.g. an
  `unidentified-computation` status). Rejected: statuses are what a
  consumer branches on and there are deliberately four. The reason
  vocabulary is where explanations belong (ADR 0006).

## Note on authority

Drafted in the owner's session on the owner's direction.
**`architect` sign-off is owed and has not happened**, and this is a
contract change, so the PR carries the `contract-change` label and the
policy answers.

If that review disagrees, the claim to attack is §"Why a verdict and not
an error": that a permanently-absent recipe hash is a fact about
evidence rather than a fault. Everything else follows from it.

---

## Ratification (2026-08-17): the load-bearing claim survives; three conditions

The `architect` review attacked §"Why a verdict and not an error" as
instructed. **The claim survives, and the decision is ratified.** The
reasoning, and what the review found that the ADR did not say.

### The attack, and why it fails

The strongest objection is that a missing `recipe_hash` is a **producer
fault**, not an evidence state. Under that reading
`freshdag-adapter-claude` is a non-conformant producer, the engine's
refusal was correctly reporting a broken adapter, and capping at
`unknown` launders a producer defect into a verdict — removing the
pressure to fix it and normalising it for every adapter that follows.

That objection is answered by the IR contract, not by this ADR.
`docs/contracts/execution-ir.md §Event Envelope` says:

> Adapters that cannot supply a `recipe_id_or_hash` MUST synthesize one
> from a session-scoped stable identifier and record the rule used.

The contract already contemplates producers that cannot supply recipe
identity and prescribes exactly what the Claude adapter does. The
absence is **licensed by the contract the adapter is conformant to**, so
it is not a fault. That settles it, and it is a stronger footing than
the exit-code argument the ADR leads with — the exit-code argument
establishes only that `unknown` is the right *report* if the condition
is an evidence state, which is the thing under dispute.

The second half of the claim — permanence — is established
independently: for a runtime exposing no recipe there is no fix, so an
error code, which by the CLI's own §exit.rs semantics means "the tool
failed", asserts something false about the world.

### Condition 1 — the code conflates two conditions, and must stay that way

`recipe-identity-unavailable` fires identically when the runtime can
never supply a recipe (Claude Code) and when a producer that *could*
supply one simply did not (a bug). The certificate does not distinguish
them, and invariant #6 takes a real if minor loss: a user cannot tell
"fix your adapter" from "this runtime can never do better."

**No fifteenth code.** Under ADR 0012's amended test the distinction is
carried losslessly by the producer's `known_limitations` and by
`detail`, and no engine branch could act on it — both cases cap at
`unknown` under invariant #7, which is the whole machine-readable
consequence. A member that changes no verdict and duplicates a note is
the case ADR 0012 rejects. This paragraph exists so the next agent that
notices the conflation finds it already ruled on.

### Condition 2 — the cap is a mask, and the record must keep saying so

This is the finding the ADR does not make, and it is the significant
one. `docs/BUILD_PLAN.md §6.2` set W10/W11 in motion specifically to
*remove* an accidental universal mask ("nothing in production registers
a coverage manifest, so every real check caps at `unknown`… which would
make those numbers worse than useless"). This ADR installs a
**deliberate** universal mask over adapter #1 on the same day: every
artifact the Claude adapter produces now caps at `unknown` for one
structural reason, whatever else the engine gets right or wrong.

`b052a98` is the proof this matters. The subagent-blindness defect it
fixed — an artifact reaching `valid` after an unobserved delegation —
was, in its own words, "not live today only because the recipe-identity
cap holds every Claude-adapter artifact at `unknown`, which is masking
rather than safety."

The cap is still the right call: the alternative masks harder, since
refusing to seal emits no certificate at all. But two things are
required and are ratified as conditions:

1. **The synthetic path must keep testing `valid`.** It does today —
   `fixtures/certificate-conformance/positive/*` and four scenarios
   carry a `recipe_hash` and reach `valid`, so engine correctness above
   the cap is exercised. This must not be allowed to lapse; a suite in
   which nothing reaches `valid` cannot detect a promotion bug.
2. **No dogfood number may be read as evidence about the world while
   the cap is universal.** `docs/DOGFOOD.md` already says this
   ("it is masking, not safety"). It stays.

### Condition 3 — the exit-code move needed a sign-off it did not get

See §6 of the 2026-08-17 review and `docs/OWNERSHIP.md`: exit codes are
stable ABI with mutual sign-off between `integration-engineer` and
`graph-engineer`. Deleting `CheckError::NoRecipeIdentity` and moving the
condition from exit 3 to exit 2 is an ABI change made without it.

**Ratified after the fact, because the direction is provably safe:** no
path that previously exited `0` changed, and no path that previously
exited non-zero now exits `0`. A CI consumer's reuse decision is
identical before and after; only the "ignore this result" versus "do not
reuse" reading moves, and it moves toward the true one. The requirement
is recorded, not waived — see `.claude/rules/architecture.md` §"Reviews
that are owed even when the contract-change process does not apply".

### Sustained without conditions

- **Why a new code rather than reusing one.** Correct.
  `no-dependencies-observed` would state something false; the coverage
  codes are about vantage point, and nothing here went unobserved.
- **`unknown`, not `likely-valid`.** Forced by §Field Rules; correct.
- **`stale` not capped.** Correct and consistent with every other use of
  `cap_at_unknown`.
- **Keeping `EngineError::MissingRecipeHash` as an unreachable
  backstop.** Correct, and well placed: `seal.rs` catches it after every
  downgrade has been applied, which is the last point before bytes
  exist.
- **Rejecting a synthesized `recipe_hash`.** Emphatically correct. That
  is the invariant-#7 failure mode with extra steps, as the ADR says.
- **The self-correction in §Consequences** (the vocabulary "guard" that
  does not guard). Accurate, and now acted on — see ADR 0015 and
  `docs/BUILD_PLAN.md §6.3`.
