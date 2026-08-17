# ADR 0012: No fourth partial-coverage member for bounded under-approximation

- **Status:** accepted — merged as `accepted` prematurely; `architect`
  review has since been supplied, ratifying the decision and requiring
  the Amendment below
- **Date:** 2026-08-17
- **Deciders:** human owner (Noah Kostesku), by delegation
- **Reviewed by:** `architect`, 2026-08-17 — sign off with conditions
- **Consulted:** ADR 0011 and its 2026-08-16 Amendment; `observer-engineer`'s
  ratification of the fsatrace manifest
- **Supersedes:** the open question left at the end of ADR 0011's Amendment
- **Amended:** 2026-08-17 — see §Amendment. The "inert horn" of the
  original §Decision is **withdrawn**; the decision itself stands.

## Context

ADR 0011's Amendment closed with a question it declined to answer:

> `under-approximates` currently collapses "misses mmap reads" and
> "misses everything" into one non-discharging bucket. That is the
> conservative choice and invariant #15 supports it, but if the fsatrace
> gap is later closed to a narrow, bounded under-approximation, the
> vocabulary has no way to say so and the observer still cannot
> discharge. Whether a bounded/scoped under-approximation deserves a
> fourth member is a real question and deliberately out of scope here.

The concern is genuine and has a name: the same **honesty-is-punitive**
argument that killed the blunt "any `partial` note disqualifies" rule
appears to apply one level down. A producer that narrows its blindness
from *everything* to *only mmap reads* has strictly improved, and the
vocabulary records no difference. Nothing rewards narrowing a gap.

## Decision

**No fourth member. `under-approximates` stays a single bucket.**

The argument turns on what `PartialReason` is *for*.
`PartialReason::discharges` is documented as "the whole machine-readable
content of the vocabulary" — the sole thing any consumer may key a
decision off. So a proposed member has exactly two possible fates, and
both fail.

### If a bounded member discharges, it is unsound

`over-approximates` discharges because over-reporting is **monotone in
the safe direction**. Extra edges produce extra staleness, which
invariant #15 explicitly prefers. What licenses discharge is the
*direction* of the error, and direction is a property no amount of
magnitude changes.

Under-approximation has no such property, and boundedness does not
supply one. **A bounded mechanism does not bound the harm.** "I miss
only mmap reads" is bounded in *how* the observer fails; the
*consequence* is unbounded, because the one dependency it missed may be
the only one that mattered. A computation that read its single decisive
input via `mmap` and everything else via `read()` yields a certificate
with every observed dependency fresh and the deciding one absent. There
is no theorem taking "small gap" to "small error", so there is nothing
to build a discharge rule on.

That is invariant #7 exactly: if you cannot prove a dependency is
unchanged, do not report the artifact valid. A bounded miss is still a
miss.

The bound is also **unauditable in a way over-approximation is not**.
Both are producer self-reports, but they fail differently when the
report is wrong. A producer that wrongly claims `over-approximates`
costs spurious staleness. A producer that wrongly claims its
under-approximation is narrow costs a wrong `valid` — and nothing in the
certificate, the log, or a third-party recheck can detect the
difference, because the evidence of the miss is precisely what is
missing.

### If a bounded member does not discharge, it adds nothing here

> **Withdrawn and replaced by the Amendment below.** As originally
> written this section argued that a non-discharging member is *inert*
> — "free text with a schema entry, the ADR 0006 mistake in a new
> shape." That argument was refuted on review and is not load-bearing.
> The corrected form follows.

A member that does not change `discharges` may still be worth having.
This one is not — but the reason is specific to it, not general.

What a fourth member would add over the status quo is a
*machine-readable* bound, and §"If a bounded member discharges, it is
unsound" is the argument that no machine may act on that bound. What
remains is the human-readable content, and that is already carried
losslessly by `PartialCoverage::note` and `known_limitations`. So the
member would duplicate what the note already says, in a field no
consumer may key a decision off. Nothing is gained; a vocabulary member
is spent.

## Consequences

- The vocabulary stays at three members. No schema change, no
  `ReasonCode` change, no engine change. This ADR is a decision not to
  act.
- **Narrowing a gap remains unrewarded, and that is accepted.** The
  honesty-is-punitive argument does not transfer from the blunt-rule
  case, and the reason it does not is worth stating plainly: under the
  blunt rule, a producer could regain discharge by *deleting a true
  note* — the incentive was to lie. Here a producer cannot regain
  discharge by any documentation change at all, only by actually
  closing the gap. The incentive points at the fix rather than at the
  paperwork. An unrewarded improvement is a weaker complaint than an
  incentive to conceal.
- The route from `under-approximates` to `over-approximates` stays open
  and is the intended one: **close the gap, or over-report instead of
  missing.** An observer that cannot see mmap reads may hash the file at
  `open` and emit a pessimistic `fs.read` — observer-contract §Required
  Behavior #4 already requires precisely this. That converts a miss into
  a coarse over-report, which discharges. The vocabulary is not blocking
  the fsatrace observer; §Required Behavior #4 is unimplemented.
- `LD_PRELOAD` evasion (setuid, static linking, raw syscalls) is
  unaffected either way. `observer-engineer` classified fsatrace's
  `fs.read` as `blind-in-scope` for that reason, and no member of this
  vocabulary could make a structurally blind observer discharge.

## Rejected alternatives

- **`under-approximates-bounded`, discharging.** Rejected: unsound, per
  the argument above. This is the option the question was really asking
  about.
- **`under-approximates-bounded`, non-discharging.** Rejected: it
  duplicates, in a field no consumer may act on, a bound that
  `PartialCoverage::note` already carries losslessly. (Originally
  rejected as "inert" — see the Amendment for why that framing was
  wrong.)
- **A numeric or probabilistic bound** (`misses < 1% of reads`).
  Rejected harder. It invites exactly the reasoning invariant #7
  forbids — trading a probability of correctness against a cache hit —
  and the number would be a producer's self-report about the tail of its
  own failure distribution, which is the least reliable number a
  producer could publish.
- **Deciding later, when a real bounded case appears.** Tempting, and
  rejected because the question is not empirical. The argument above
  does not depend on any producer's behaviour, so a real case would not
  add information — it would only add pressure to decide under it.
  *This rejection is narrower than it was written.* It holds for the
  **magnitude** version of the member. It does not hold for the
  **scope-predicate** version, whose discharge would depend on the event
  stream and on other producers' manifests — see §Amendment.

## Note on authority

ADR 0011's Amendment escalated this to the human owner and declined to
rule. The human owner delegated the decision rather than making it
personally, and this ADR records the reasoning so the delegation is
auditable rather than merely asserted.

`architect` review **has now happened** (2026-08-17) and ruled the
delegation valid: the human owner is the principal, the constitution's
escalation path terminates at them, and a delegated ruling recorded with
its reasoning is auditable in a way an undocumented one would not be.
The ADR does not need re-issuing.

It also found the bookkeeping wrong, which this Amendment corrects: this
document merged as `accepted` while its own closing paragraph said
review was owed. An ADR cannot be accepted and pending review at once —
it should have merged as `proposed`.

The review attacked the load-bearing claim as instructed. That claim —
that no theorem takes a bounded mechanism to a bounded harm — **survives
for the member as posed**, and the unauditability paragraph was assessed
as the strongest in the ADR. The second horn did not survive.

---

## Amendment (2026-08-17): the inert horn is withdrawn, and compositional discharge is open

Required by the `architect` review. **The decision is unchanged: no
fourth member, today.** What changes is one branch of the argument for
it, and the scope of what the ADR may claim to have settled.

### Why the inert horn was wrong

It argued that a non-discharging member is inert, on the premise that
`PartialReason::discharges` is "the whole machine-readable content of
the vocabulary."

Apply that premise to the vocabulary this ADR defends.
`UnderApproximates` and `BlindInScope` **both** return `false` from
`PartialReason::discharges` (`crates/freshdag-core/src/ir/coverage.rs`).
By the horn's own reasoning one of them is already inert and should be
collapsed into the other — which ADR 0011 deliberately did not do, and
this ADR does not propose. The argument cannot hold both that a
non-discharging distinction is inert and that its own vocabulary rightly
contains two of them.

The premise is what fails. ADR 0011 justified three members on the
**direction and kind** of the gap, not on discharge behaviour, and a
non-discharging *closed* member is still machine-readable: it keys
`freshdag why` prose, certificate diffing, coverage-gap reporting, and
gap prioritisation. ADR 0006's defect was **free text** — unbounded
strings no consumer could enumerate — not "a code no engine branch
happens to branch on."

The corrected rejection is narrower and is now in §Decision: the member
adds nothing *here*, because its human-readable content is already
carried losslessly by `note`. Not because non-discharging members are
worthless in general.

### The question this ADR forecloses without deciding

ADR 0011's escalation asked about a "bounded/**scoped**"
under-approximation. The commit that raised it said "bounded **or
scoped**." The body above addresses only the magnitude version, and the
scoped version was dropped without argument.

A **scope-predicate** member is a different object. Its discharge would
not be a nullary constant but a **function of the event stream and of
the other producers' coverage** — so it is neither unsound-by-
construction (the discharging horn does not reach it) nor duplicative of
`note` (the corrected second horn does not reach it either). It would
permit two partial producers to **jointly** discharge an obligation
neither discharges alone.

That is precisely the composition a coverage manifest exists to express,
and precisely what today's per-producer, any-producer-suffices rule
forecloses.

The best rebuttal is available and is *not* universal: for a scope a
producer is blind in, the absence of events in that scope is itself
unobservable, so a self-reported scope is usually undecidable. **Usually
is not always.** Some scopes are decidable from another producer's
evidence — `proc.spawn` events bound what a subprocess could have
touched, and a statically-linked binary is inspectable.

**Recorded as open.** Compositional/joint discharge is not decided by
this ADR, and the two-horned argument above must not be read as having
covered it. Anyone reaching for it should expect to write a new ADR, not
to cite this one.

### What the reader should attack

If a future review disagrees with the decision, the target is still the
claim in §"If a bounded member discharges, it is unsound" — that no
theorem carries a bounded mechanism to a bounded harm. That is what the
`architect` review probed and could not break. The open question above is
a different question, not a crack in this one.
