# ADR 0012: No fourth partial-coverage member for bounded under-approximation

- **Status:** accepted
- **Date:** 2026-08-17
- **Deciders:** human owner (Noah Kostesku), by delegation
- **Consulted:** ADR 0011 and its 2026-08-16 Amendment; `observer-engineer`'s
  ratification of the fsatrace manifest
- **Supersedes:** the open question left at the end of ADR 0011's Amendment

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

### If a bounded member does not discharge, it is inert

Then no decision keys off it, and a vocabulary member no decision keys
off is free text with a schema entry — the ADR 0006 mistake ADR 0011
exists to end, arriving in a new shape.

The human-readable bound is already carried, losslessly, by
`PartialCoverage::note` and `known_limitations`. What a fourth member
would add is *machine*-readability of the bound, and the section above
is the argument that no machine may act on it.

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
- **`under-approximates-bounded`, non-discharging.** Rejected: inert,
  and reintroduces the free-text-with-a-schema-entry defect.
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

## Note on authority

ADR 0011's Amendment escalated this to the human owner and declined to
rule. The human owner delegated the decision rather than making it
personally, and this ADR records the reasoning so the delegation is
auditable rather than merely asserted.

`architect` review is still owed and has not happened. If that review
disagrees, the thing to attack is the claim in
§"If a bounded member discharges, it is unsound" — that no theorem takes
a bounded mechanism to a bounded harm. Everything else follows from it.
