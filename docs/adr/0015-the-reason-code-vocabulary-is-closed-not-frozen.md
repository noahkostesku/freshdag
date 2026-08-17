# ADR 0015: The reason-code vocabulary is closed, not frozen — the admission test, the registry, and the guard

- **Status:** accepted
- **Date:** 2026-08-17
- **Deciders:** `architect`
- **Extends:** ADR 0006 (closed reason-code vocabulary). Does not
  supersede its argument. It **supersedes ADR 0006 §Decision's sentence
  "The vocabulary is ten codes" and the ten-row table beneath it as a
  normative statement of membership** — that table is now a historical
  record of the vocabulary on 2026-08-15.
- **Consulted:** ADR 0009 (10 → 12), ADR 0012 (the amended test for a
  warranted member), ADR 0014 (12 → 13), the `unproven-dependency`
  change of 2026-08-17 (13 → 14), the verifier finding recorded in ADR
  0014 §Consequences, `docs/contracts/certificate-contract.md §Change
  Policy`, `docs/NOVELTY.md §2`.
- **Contract change:** no. This ADR adds no code, changes no wire form,
  and touches no schema. It rules on the process by which codes are
  admitted and on where the vocabulary is written down.

## Context

On 2026-08-17 the `ReasonCode` enum went from ten members to fourteen in
a single day:

| | Added | By | Declared as extending 0006 |
|---|---|---|---|
| 10 → 12 | `volatile-within-ttl-unprobed`, `dependency-changed-during-computation` | ADR 0009 | yes |
| 12 → 13 | `recipe-identity-unavailable` | ADR 0014 | no |
| 13 → 14 | `unproven-dependency` | commit `560151e`, no ADR | no |

ADR 0006 says "The vocabulary is ten codes" and calls it **closed**. A
reader arriving at that document today is told something false by a
document that is still marked `accepted`, and three of the four new
members did not link themselves back to the decision they modify.

Three further facts frame the ruling:

1. **The certificate contract is `provisional (v0.1)`**, and its Change
   Policy explicitly licenses non-additive change inside `v0.1` given an
   ADR, same-PR migration, and a changelog entry. Every widening met
   that policy, and the changelog now records all of them (including
   ADR 0009's, recorded late).
2. **The "guard" on the vocabulary does not guard.** A verifier added a
   fourteenth variant to the enum, omitted it from `ALL_REASON_CODES`
   and both JSON schemas, and the whole suite passed.
   `assert_eq!(ALL_REASON_CODES.len(), 13)` is a hand-maintained count
   an enum addition does not perturb, and `schema_reason_enums_match_rust`
   compares the schemas against `ALL_REASON_CODES` rather than against
   the enum — so a break at the top of the chain hides the entire chain.
   The only real guard is the compiler, via an exhaustive `match` in
   `freshdag-cli::render::prose`. The contract table in
   `certificate-contract.md` is guarded by nothing at all.
3. **The vocabulary is copied into prose in at least four places** —
   the contract table, `fixtures/certificate-conformance/README.md`,
   `fixtures/scenarios/README.md`, and ADR 0006 — and every copy but the
   contract table was stale within a day (9 of 14 and 11 of 14
   respectively).

## Decision 1: closed means closed at any instant, not frozen forever

**A closed vocabulary that grows is not a contradiction. A closed
vocabulary with no admission test and no freeze date is.**

ADR 0006's closedness argument is about a property that still holds
exactly: at any instant the set is finite, enumerable, schema-enforced,
and has no `Other(String)` escape hatch, so a consumer can exhaustively
handle it and MUST refuse a certificate carrying a code outside it. That
argument never claimed the set could not gain a member; ADR 0009 said so
explicitly — "the process for doing so is exactly the one ADR 0006
anticipated."

What ADR 0006 did not do is distinguish **closed** from **frozen**, and
the day's churn is what that omission costs. This ADR draws it:

- **Closed** — a property of the set at any instant. Holds today and is
  not weakened by growth.
- **Frozen** — a property of the set over time. Does **not** hold today,
  is not claimed, and is scheduled below.

No document may describe the vocabulary as stable, settled, or final
while `certificate-contract.md` is `provisional`. "Closed" is the only
word available, and it means the first thing.

## Decision 2: the admission test

A proposed reason code is warranted only if it passes **all four**. This
is ADR 0012's amended test generalised from `PartialReason` to
`ReasonCode`, plus what the four 2026-08-17 additions actually turned
on.

**1. Distinctness.** Some world-state produces this code and no existing
code, *and* some world-state produces an existing code and not this one.
A code that is always accompanied by another code, and adds nothing to
it, is a synonym.

**2. Truthfulness of the alternative.** Reusing the nearest existing
code must put a **false sentence** in front of a user reading
`freshdag why` — not merely an imprecise one. This is the ADR 0009
defect the project has corrected once and the test ADR 0014 applied
correctly: `no-dependencies-observed` on a computation with three
verified dependencies is false, not vague.

**3. Machine-readability with a consumer.** The code must be something a
consumer *can act on*, and at least one of these must be named in the
proposing ADR:

- an engine branch (a cap, a gate, a verdict), or
- an exit-code floor, or
- a certificate-diffing or coverage-reporting rule.

Following ADR 0012's Amendment, a code that changes no verdict is **not
automatically inert** — a closed member keys `freshdag why` prose,
certificate diffing and gap prioritisation, which free text cannot. But
"keys prose" alone is the weakest possible answer and requires the
proposer to say why the prose distinction cannot ride on `detail`.

**4. Not already carried losslessly.** If the content is fully present
in `detail`, `PartialCoverage::note`, or a producer's
`known_limitations`, and no consumer may act on the difference, the
member is duplicative and is refused (ADR 0012 §Decision, as amended).

**And a fifth, procedural:** the proposing ADR MUST carry
`**Extends:** ADR 0006` and MUST add a `certificate-contract.md`
changelog entry in the same PR. ADR 0009 did the first and not the
second; ADR 0014 did the second and not the first; the
`unproven-dependency` change did neither and had no ADR at all.

### Applying it to the four members added on 2026-08-17

| Code | 1 Distinct | 2 Alternative is false | 3 Consumer | 4 Not duplicative | Verdict |
|---|---|---|---|---|---|
| `volatile-within-ttl-unprobed` | yes | yes — "a probe ran and matched" was false | yes — the `--accept-likely-valid` floor | yes | **warranted** |
| `dependency-changed-during-computation` | yes | yes — `drift` asserts an unobserved claim | yes — forces `EdgeVerdict::Unknown` | yes | **warranted** |
| `recipe-identity-unavailable` | yes | yes — `no-dependencies-observed` is false when deps verified | yes — caps at `unknown` in `seal.rs` | yes | **warranted** |
| `unproven-dependency` | yes | yes — the condition had no expression at all | yes — caps at `unknown`, and closed a live invariant-#7 hole | yes | **warranted** |

**All four pass.** The vocabulary's growth is not the finding; the
absence of a written test the growth could have been checked against is.
Two of the four are the strongest members in the set on criterion 3,
because they change a verdict rather than only a sentence.

`unproven-dependency` deserves a specific note, since it is the one
added without an ADR. Judged on merit it is the best-justified code in
the vocabulary: before it, a computation that read four files and
fingerprinted three certified over the three **in silence**, because
`no-dependencies-observed` fires only on the empty set. That is a live
invariant-#7 hole — an artifact reported valid over an input nobody
could check — and it became reachable the moment the adapter started
fingerprinting reads. Naming the observation rather than promoting it to
a dependency is exactly right: promoting it would fabricate the evidence
invariant #7 demands.

**One open design question it exposes, deliberately not decided here.**
`unproven-dependency` is artifact-scoped with the affected keys in
`detail`, because the keys are deliberately absent from `depends_on[]`
and there is no edge to attach an edge-scoped reason to. That is forced
by the certificate's shape, and it means the *identity* of an
unverifiable input is reachable only by parsing `detail` — which
`certificate-contract.md §The detail field` forbids consumers from
doing. The certificate has no place to name an input that exists and
cannot be verified. A field (`unproven_inputs[]`) would fix it and is
**not authorised now**: it is new contract surface, and Decision 5
freezes the shape until the dogfood wave reports. Recorded so it is not
rediscovered as a bug.

## Decision 3: one normative registry; prose copies are pointers, never lists

The vocabulary is written down in exactly **two** normative places, and
they are ordered:

1. **`ReasonCode` in `freshdag-core::dependency::validity`** — the
   source of truth. Rust and both JSON schemas MUST agree with it.
2. **The table in `docs/contracts/certificate-contract.md §Reason
   Codes`** — the normative human-readable mirror, carrying each code's
   scope and meaning. A code exists for consumers only if it is here.

`schemas/certificate/v0.1.json` and `schemas/scenario/v0.1.json` are
mechanical mirrors of (1), enforced by test.

**Every other mention of the set is a pointer, not a copy.** No README,
no ADR, no doc-comment, and no test comment may enumerate the members.
An enumeration in a non-normative document is a copy that will be stale
by the next contract change, and four of them were stale within a day.

`fixtures/certificate-conformance/README.md` and
`fixtures/scenarios/README.md` are corrected to pointers in the same
change as this ADR. ADR 0006's table is annotated as historical rather
than rewritten: an ADR records what was decided when, and editing its
decision text to match today's state destroys the record.

## Decision 4: the drift guard is mandatory, and the current one does not count

**Required, before any further reason code is added:**

1. **A single declaration site.** The variant list, `as_wire_str`,
   serde's rename, and the test-time enumeration MUST be generated from
   one list — a declarative macro over `(Variant, "wire-string")` pairs
   is the obvious form and adds no dependency. `ALL_REASON_CODES` stops
   being hand-maintained, so omitting a variant from it becomes
   impossible rather than untested.
2. **The schemas are checked against the enum, not against the hand
   list.** With (1) these coincide; without (1) the current test is
   checking a copy against a copy.
3. **The contract table is checked.** A test parses the `§Reason Codes`
   table out of `certificate-contract.md` and asserts its code column
   equals the enum, both directions. The core test module already reads
   repository files via `repo_root()`, so this is cheap. Today the
   normative human-readable mirror is guarded by nothing, which is the
   worst-guarded artifact in the chain and the one external consumers
   will read.
4. **A negative fixture.** One certificate carrying an
   invented-but-plausible code, asserting it fails to deserialize —
   pinning ADR 0006's "a consumer MUST treat it as unreadable rather
   than guess" as behaviour rather than prose.

Until all four exist, no PR may state or imply that the vocabulary's
consistency is test-enforced. ADR 0014 made that claim, a verifier
disproved it, and the claim is corrected in place. The correction is not
the fix.

Owner: `core-engineer`, with `verifier` confirming (1)–(4) by
reproducing the original break — add a variant, omit it everywhere else,
and require a red suite.

## Decision 5: the vocabulary freezes when the contract stabilises, and there is a moratorium until then

**Moratorium.** No further `ReasonCode` member is admitted until either
(a) Decision 4's guard is in place **and** the code passes Decision 2,
or (b) the code is required to close a demonstrated invariant-#7
violation, in which case it may land ahead of the guard with the guard
following in the next PR. Closing a live soundness hole outranks
process; nothing else does.

**Freeze.** The vocabulary is a **gate on the certificate contract's
provisional → stable transition** (`docs/BUILD_PLAN.md §6.1`). When
`certificate-contract.md` becomes `stable`, `v0.1` freezes, and from
that point a new code is a `v0.2` schema bump and a new
`CERTIFICATE_SCHEMA_V0_2` constant — because a JSON Schema enum is
closed, so adding a member is additive for writers and **breaking for
readers**. That asymmetry is already recorded in the certificate
contract's changelog and is the whole reason growth is not free.

Stabilisation should not be attempted before the dogfood wave reports
(`BUILD_PLAN §6.2`). Freezing a vocabulary that has never met a real
session would freeze the wrong one.

## Consequences

- ADR 0006 remains `accepted`; its argument is untouched. Its
  membership table becomes historical, and an amendment on it points
  here.
- Two fixture READMEs lose their enumerations.
- `core-engineer` owes the Decision 4 guard. It is a blocker on the
  fifteenth code, not on current work.
- Nothing on the wire changes. No certificate becomes readable or
  unreadable because of this ADR.

## Novelty

Per `.claude/rules/novelty.md`, ADR motivations receive novelty review.
`novelty-reviewer` is **not** consulted here, because this ADR makes no
novelty claim and adds no feature — it constrains a process. It does
bear on §2, so the bearing is stated rather than left implicit, and any
disagreement is `novelty-reviewer`'s to raise.

`docs/NOVELTY.md §2` was rewritten on 2026-08-16 to retire the
"machine-checked never-promote rule" claim (EA-Graph owns it). The
surviving wedge is a **conjunction**, and its closest conjunct here is
"emits the whole judgment as a **portable manifest a third party can
re-check**." §2's stated defence names three things, of which (b) is
"the certificate as a portable artifact other tools can consume rather
than a proprietary status."

**The churn does not falsify the wedge, and it does attack defence
(b).** A vocabulary that moves 40% in a day is not yet a portable
artifact; it is an internal enum with a schema file attached. A third
party's validator broke four times on 2026-08-17. That is a defensible
state for a contract explicitly marked `provisional` with no external
consumers — and it stops being defensible the moment anything outside
this repository validates a certificate, which is precisely when the
wedge starts paying.

Also worth stating plainly, since ADR 0006 §Novelty made the opposite
move: the **closed-vocabulary-plus-non-normative-sidecar shape is
OpenVEX's** (`justification` + `impact_statement`), with RFC 5280
`CRLReason` as the older ancestor, both already on the §1 collision
table at High and Medium. Growing that vocabulary well is table stakes,
not a contribution, and no positioning document may present the
reason-code set as evidence of anything. What remains ours is the trust
classes the codes range over and the coverage-deficit rule grounded in
producer vantage point — unchanged by this ADR.

No new collision was discovered in this review, so `docs/NOVELTY.md §1`
is not updated.

## Rejected alternatives

- **Rewrite ADR 0006's table to fourteen rows.** Rejected. An ADR is a
  dated record of a decision, not a live reference. Editing its decision
  text to match the present destroys the only evidence of what was
  decided when — and the growth from ten is exactly the thing a reader
  needs to be able to see. An amendment, and a pointer to the normative
  registry, is the honest fix.
- **Declare the vocabulary open and add `Other(String)`.** Rejected for
  ADR 0006's original reason, which is undamaged: every consumer would
  route unknown conditions through `Other` and the vocabulary would stop
  being one. The contract-change process is the intended friction.
- **Freeze the vocabulary now at fourteen.** Rejected. The dogfood wave
  has produced one session, with 8% observability and no `valid`
  reachable through adapter #1. Freezing a vocabulary against a system
  that has never certified a real artifact freezes guesses. The
  moratorium in Decision 5 gets the discipline without the premature
  commitment.
- **Cap the vocabulary at a number.** Rejected: a count is not a test.
  A cap would be argued around by overloading `detail`, which
  `certificate-contract.md §Anti-patterns` already forbids by name
  ("encoding a new reason code inside `detail`"). Decision 2 puts the
  friction where the judgement is.
- **Require a full ADR for every code.** Rejected as written, and
  partially adopted. `unproven-dependency` closed a live invariant-#7
  hole found by a verifier; holding that behind an ADR would have left
  an unsound `valid` in `main` for the duration. Decision 5's carve-out
  says so explicitly, and the record is owed afterwards rather than
  waived — this ADR is where the four additions are adjudicated.
