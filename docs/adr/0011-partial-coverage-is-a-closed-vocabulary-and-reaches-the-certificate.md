# ADR 0011: Partial coverage is a closed vocabulary, and it reaches the certificate

- **Status:** accepted
- **Date:** 2026-08-16
- **Deciders:** architect
- **Consulted:** `verifier` (found it — D1, Wave 2 rejection),
  `core-engineer` (owns `CoverageManifest`, `CoverageEntry`,
  `schemas/certificate/`), `store-engineer` (owns `SilenceMeaning`, the
  correct and currently-unused implementation), `observer-engineer` and
  `claude-adapter` (both publish `partial` maps that this ADR reclassifies).
- **Extends:** ADR 0006. Same argument, one layer down.
- **Requires:** a `contract-change`-labelled PR. Touches
  `freshdag-core` (`CoverageManifest.partial`, `CoverageEntry`),
  `schemas/coverage-manifest/`, `schemas/certificate/v0.1.json`,
  `docs/contracts/certificate-contract.md`,
  `docs/contracts/adapter-contract.md`,
  `docs/contracts/observer-contract.md`.
- **Blocks:** ADR 0007's record loop, and W11 in `docs/BUILD_PLAN.md
  §6.2`. See §Sequencing.

## Context

`CoverageManifest.partial` is a `BTreeMap<String, String>` — event-kind
pattern to human-readable note — with the doc comment "Consumers should
treat partial-covered silences with the same suspicion as uncovered
silences."

The store implements exactly that: `ComputationCoverage.partial_notes`,
`interpret_silence`, and `SilenceMeaning::PartiallyObserved`, whose own
doc comment says "Treat with the same suspicion as `Unobserved`."

**The engine never reads any of it.** `grep -rn SilenceMeaning
crates/freshdag-engine/` is empty. `CoverageEntry` has no `partial`
field, so `From<&CoverageManifest>` drops it at the manifest→certificate
boundary, and `has_fs_covered_observer` tests only `role` and
`covers()`. `CoverageManifest::covers` even documents that it "does not
consider `partial` — that's a separate consumer-side signal", and no
consumer consults it.

The verifier reproduced the consequence against the real binary. Two
stores identical except the observer's `partial` map, both invoking
`bash`:

```
partial {}                                              → valid, exit 0
partial {"fs.read": "cannot see reads inside subprocs"}  → valid, exit 0
```

An observer that declares itself blind discharges the `bash`/`task`
obligation as well as a real one, and the certificate says the artifact
is safe to reuse.

This defeats the coverage-deficit rule at its hinge. The `role` field
exists precisely so that only an observer discharges the obligation; a
self-declaredly-blind observer passing it makes `role` a formality. And
`docs/contracts/certificate-contract.md` §Coverage-Deficit Rule already
*argues from* `partial` — it justifies excluding adapters by citing "the
adapter contract's own coverage example pairs `fs.read` with the partial
note 'only from Read tool; subprocess reads via observer' — that note
describes a producer that cannot answer the question this rule asks."
The contract's reasoning names `partial`; the implementation reads only
`role`. This ADR does not add a new requirement. It finishes one.

### Why the obvious fix is wrong

> **Factually corrected by the 2026-08-16 Amendment.** The example below
> is the contract's *illustrative* manifest, not the shipped observer's;
> `crates/freshdag-observer/src/linux.rs` declares the opposite, and the
> counterexample does not survive. The decision does; see Correction 2
> for the argument that replaces this one.

"Any `partial` note for a kind ⇒ cannot discharge" fails immediately.
The reference Linux observer in `docs/contracts/observer-contract.md`
declares:

```json
"partial": {
  "fs.write": "rename-atomic writes are correlated at close; …",
  "fs.read":  "mmap reads are pessimistic: hashed at mmap time"
}
```

Under a blunt rule, the one real observer we have could never discharge
a `bash` obligation, and the rule would be inert in the opposite
direction. But those notes are not the same kind of claim as "cannot see
reads inside subprocesses":

- *"mmap reads are pessimistic"* — the observer **sees** the read and
  may over-report. Over-approximation yields extra dependencies, hence
  extra staleness. It fails safe.
- *"cannot see reads inside subprocesses"* — the observer does not see
  the event at all. It fails unsafe.

Because `partial` is free text, no machine can tell these apart. That is
the real defect: `partial` is prose where a machine decision is
required. It is precisely the disease ADR 0006 diagnosed for
`ValidityReason.reason` — "a contract you cannot test is a convention" —
one layer down.

## Decision

Three changes.

### 1. `partial` becomes a closed vocabulary plus a non-normative note

Each entry carries a `PartialReason` and keeps its human-readable
`note`. The reason is what machines read; the note is what humans read
and nothing may assert on. This is ADR 0006's shape, and OpenVEX's
(`justification` + `impact_statement`, `docs/NOVELTY.md §1`).

| `PartialReason` | Meaning | Discharges an obligation? |
| --- | --- | --- |
| `over-approximates` | May report events that did not happen, or report them more coarsely than reality. Never misses one. | **Yes** |
| `under-approximates` | May miss real events of this kind. | **No** |
| `blind-in-scope` | Structurally cannot observe this kind in some scope (e.g. inside subprocesses). | **No** |

The direction of the error is the whole point: over-approximation
produces spurious staleness, which invariant #15 explicitly prefers.
Under-approximation and blindness produce spurious freshness, which is
invariant #7.

**Migration is fail-safe by construction.** The wire form accepts either
a bare string (the current shape) or `{reason, note}`. A bare string
decodes as `under-approximates`. Old manifests keep parsing and get the
conservative answer; a producer that deserves to discharge must now say
so explicitly. Defaulting the other way would be the invariant-#7
mistake this ADR exists to fix.

Reclassifying the three manifests in-tree is the respective owners'.
~~the fsatrace observer's two notes are `over-approximates`; the Claude
adapter's are `blind-in-scope`.~~ **Withdrawn by the 2026-08-16
Amendment, Correction 1** — factually wrong for fsatrace, and not this
ADR's call in either case. Owners classify; this ADR supplies only the
vocabulary and the fail-safe default.

### 2. `CoverageEntry` carries `partial`

`From<&CoverageManifest>` stops dropping it, and
`schemas/certificate/v0.1.json` gains the field.

This is not optional bookkeeping. `CoverageEntry`'s own doc comment says
`emits` is "required for `check_coverage_deficit` to be checkable from
the certificate + event stream alone." The same sentence applies to
`partial`, and more forcefully: a certificate that omits its producers'
declared blindness cannot be re-checked by anyone, because the fact that
would change the verdict is not in it. `docs/NOVELTY.md §2` now rests
the wedge on the certificate being a portable artifact a third party can
re-check. A certificate that hides the producer's own admission of
blindness is not one.

### 3. One implementation of silence semantics, and it is the store's

`has_fs_covered_observer` becomes:

> An observer discharges a `bash`/`task` observation obligation only if
> it declares `fs.read` coverage and **every** `partial` entry whose
> pattern matches `fs.read` carries the reason `over-approximates`.

("every", not "the most specific" — added by the 2026-08-16 Amendment,
Correction 4. `partial` is a conjunction of admissions; a narrow entry
must not annotate away a broad one. Adding a `partial` entry can only
ever make a producer discharge less.)

Two corrections are folded in:

- **`&&`, not `||`.** The current predicate is `covers(FsRead) ||
  covers(FsWrite)`, so an observer declaring only `fs.write` discharges
  a `bash` obligation while being unable to see a single dependency.
  Validity is about inputs. `fs.read` is the dependency-bearing kind and
  is the one that must be covered. (Found while ruling on D1; not
  previously reported.)
- **The engine consumes the store's `SilenceMeaning`** rather than
  growing a second implementation. The store already computes
  `ComputationCoverage.partial_notes` per computation. Two
  implementations of silence semantics that disagree is the finding
  here; the resolution is one implementation, in the component that owns
  derivation from the log (`ARCHITECTURE.md §4`), consumed by the engine
  — plus the data on the certificate so third parties can recheck it
  without the store.

## Consequences

- The verifier's two-store test inverts: the blind observer's store
  reports `unknown` with `coverage-deficit`, exit 2. **Amendment,
  Correction 3:** so does the real one. No in-tree producer is known to
  qualify as `over-approximates`, so `bash`-invoking computations on
  Linux go non-`valid` after the migration.
- Every producer's `partial` map must be reclassified. Three exist.
- Certificates get wider. Acceptable; `known_limitations` already
  ships human-readable text for the same audience.
- **Sequencing (D7).** Today this hole is masked: nothing in production
  registers a coverage manifest, so real adapter output never reaches
  `valid` at all. ADR 0007's record loop and W11 remove that mask.
  **This ADR lands before either.** `docs/BUILD_PLAN.md §6.2` is
  amended to make it a hard gate rather than a parallel item.
- A fixture is required in the certificate-conformance negative suite:
  *observer declaring `blind-in-scope` on `fs.read`, computation invokes
  `bash`, certificate claims `valid`* — the checker must reject it. This
  is the sixth anti-pattern the negative suite was built for.

## Rejected alternatives

- **Treat any `partial` note as disqualifying.** Rejected: disqualifies
  the reference observer, so the rule would be inert or routinely
  overridden. ~~(Premise false — see Amendment, Correction 2.)~~ Still
  rejected, on the replacement argument: it makes honest documentation
  punitive, and it leaves `partial` free text, so the certificate
  carries the fact of an admission without its direction.
- **Leave `partial` free text and have the engine pattern-match the
  note.** Rejected outright — it is the free-text-reason-code mistake
  ADR 0006 was written to end, and invariant #13 requires public
  contracts be testable.
- **Keep `partial` off the certificate and consult only the store.**
  Rejected: it makes the certificate uncheckable standalone, which is
  the property `docs/NOVELTY.md §2` now depends on.
- **Default a bare-string `partial` to `over-approximates` to avoid
  reclassification work.** Rejected: a silent-wrong-answer generator on
  the invariant-#7 path, which is the same reasoning that made
  `CoverageManifest.role` deliberately have no serde default.

---

## Amendment, 2026-08-16 — the worked example was wrong

Found by `core-engineer` while implementing this ADR, who correctly
declined to act on its authority and escalated. Ruled by `architect` the
same day. Appended, not rewritten: the decision stands in full. What
follows corrects a factual claim in the reasoning and in
§Decision 1's closing paragraph.

**The error.** §Why the obvious fix is wrong argues that a blunt "any
`partial` note ⇒ cannot discharge" rule fails because the reference
Linux observer's notes are over-approximations, quoting *"mmap reads
are pessimistic: hashed at mmap time"* from
`docs/contracts/observer-contract.md`. §Decision 1 then concludes "the
fsatrace observer's two notes are `over-approximates`."

The shipped observer declares the opposite.
`crates/freshdag-observer/src/linux.rs` publishes:

- `fs.read` — "mmap reads bypass LD_PRELOAD interception and **are not
  emitted** (observer-contract §Correctness Pitfalls #2); statically
  linked or raw-syscall processes are invisible"
- `fs.write` — "rename-atomic writes are emitted against the temporary
  path only; the synthetic `fs.write` at the rename target required by
  observer-contract §Required Behavior #3 **is not yet implemented**"

Both are missing emissions, not pessimistic ones — the fail-unsafe
direction, and the class this ADR exists to catch. The ADR quoted the
contract's example manifest and treated it as a description of the
crate. It is not one; see the second ruling below.

**Correction 1.** §Decision 1's sentence "the fsatrace observer's two
notes are `over-approximates`" is withdrawn. It states a conclusion
this ADR has no standing to reach: classifying a producer's own notes
is that producer's owner's call. `observer-engineer` classifies the
fsatrace manifest, and `claude-adapter` its own, in the migration PR.
No implementer may cite this ADR as authority for either
classification.

**Correction 2 — the motivating counterexample does not survive, and
the decision does not depend on it.** If both fsatrace notes are
under-approximating, then the blunt rule would not have disqualified a
legitimate observer; it would have correctly disqualified a genuinely
blind one, and §Rejected alternatives' first entry rests on a false
premise. The decision is nevertheless unchanged, on two arguments that
need no example:

- **Invariant #13.** `partial` is prose where a machine decision is
  required. That is true whatever any current producer declares, and it
  is the whole of §Decision 2 — the certificate must carry the
  *direction* of a producer's admission, not merely the fact that it
  made one. A blunt rule leaves `partial` free text and leaves a
  third-party rechecker with "there was a note."
- **The blunt rule makes honesty punitive.** A producer that sees every
  event but reports coarsely — directory-granular reads, a hash taken
  at mmap time rather than at each fault — is strictly safer than one
  that reports nothing, and under a blunt rule its only way to keep
  discharging is to delete the note. An incentive to under-document is
  the opposite of what a coverage manifest is for. This is the durable
  form of the argument and replaces the withdrawn example.

**Correction 3 — record the behavioural consequence honestly.**
§Consequences says "the verifier's two-store test inverts." On the
corrected facts, so does the real one: no in-tree producer is currently
known to qualify as `over-approximates`, so the expected post-migration
state is that **the fsatrace observer does not discharge a `bash`/`task`
obligation either**, and `bash`-invoking computations on Linux go
non-`valid`. That is a finding, not a regression — the observer cannot
see mmap reads or statically linked processes, so it cannot answer the
question the rule asks — but it is a much larger consequence than
§Consequences records and it will be mistaken for a bug during W9.

It follows that, on every manifest in the tree today, the closed
vocabulary and the blunt rule produce identical verdicts. The
vocabulary earns its keep as the certificate's machine-readable
explanation (§Decision 2) and as headroom for a producer that
legitimately over-approximates — not, today, as a behavioural
difference. State that plainly rather than letting the next reader
discover it and conclude the ADR was decorative.

**Correction 4 — every matching entry must discharge.** Confirmed as
proposed by the implementer, and promoted here from a test name
(`a_specific_partial_entry_cannot_override_a_broader_blindness`) to
normative text in §Decision 3. Where several `partial` patterns match
an event kind, the obligation for that kind is discharged only if
**every** matching entry's reason is `over-approximates`. Most-specific-
wins is rejected.

A coverage manifest is a conjunction of admissions, not a lookup table.
Most-specific-wins is a *resolution* rule, appropriate where a later
value replaces an earlier one; admissions do not replace each other.
The Claude adapter carries both `fs.*` ("filesystem effects inside
`bash` and `task` invocations are invisible to this adapter") and a
narrower `fs.read`; under most-specific-wins the narrow entry would
annotate away the broad admission, which is this ADR's own defect in a
new shape — a machine-readable field whose meaning can be edited away
by adding data. The property to preserve, and the one to test, is
**monotonicity: adding a `partial` entry can only ever make a producer
discharge less.**

**Ruling 5 — a contract's reference manifest is illustrative, never
descriptive.** The ambiguity that produced this error is now closed
generally, because it will otherwise produce another one.

Example payloads in `docs/contracts/` describe the *shape* a conformant
producer must satisfy. They are not, and may not be read as, factual
claims about anything in `crates/`. Consequences:

- No ADR, engine branch, test, or review may cite a contract's example
  manifest as evidence of what an in-tree producer declares. Cite the
  source file.
- Where a shipped producer and the example diverge, the divergence is a
  conformance gap in the producer, not a contradiction in the contract.
  `observer-contract.md §Correctness Pitfalls #2` ("Hash at mmap time;
  document pessimism") is a **requirement on implementers**, and the
  example manifest shows an observer that has met it. `linux.rs` has
  not; its `partial` note is the honest declaration of that gap, and it
  cites Pitfall #2 as the requirement it is failing. Read that way the
  two are consistent and no reconciliation of the contract's text is
  needed — only a label.
- The proximate cause was that the contract's example uses `"producer":
  "freshdag-observer-fsatrace"`, the same string `linux.rs` emits, which
  invites exactly the substitution made here. `observer-engineer` owns
  `observer-contract.md` and should rename the example producer to
  something unmistakably illustrative and add an explicit
  "illustrative, not a description of any shipped observer" banner
  above it. Same edit applies to every example manifest in
  `docs/contracts/`. `architect` does not make those edits here.

**Open, escalated to the human, not decided:** `under-approximates`
currently collapses "misses mmap reads" and "misses everything" into
one non-discharging bucket. That is the conservative choice and
invariant #15 supports it, but if the fsatrace gap is later closed to a
narrow, bounded under-approximation, the vocabulary has no way to say
so and the observer still cannot discharge. Whether a bounded/scoped
under-approximation deserves a fourth member is a real question and
deliberately out of scope here.
