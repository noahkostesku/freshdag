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

Reclassifying the three manifests in-tree is the respective owners':
the fsatrace observer's two notes are `over-approximates`; the Claude
adapter's are `blind-in-scope`.

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
> it declares `fs.read` coverage whose partial reason, if any, is
> `over-approximates`.

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
  reports `unknown` with `coverage-deficit`, exit 2.
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
  overridden.
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
