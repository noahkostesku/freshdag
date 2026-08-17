# Contract: Validity Certificate

**Status:** provisional (v0.1).

**Owner:** `core-engineer`.

**Governs:** the `.freshdag/*.cert.json` file emitted alongside every
produced artifact. This is FreshDAG's most user-facing artifact and
its shareable primitive.

**Invariants relied on:** #6, #7, #8, #9, #13.

---

## Purpose

The certificate is a portable, human-readable manifest tying an
artifact to its dependencies, their fingerprints, and their trust
classes. It is what `freshdag check` reads; what humans open to
answer "why does the system say this is stale"; and what integrations
(Attio webhook, Clay column, GitHub check) will eventually consume.

The certificate is content-addressed by its own bytes (`cert_id`) so
that shared certificates cannot be silently rewritten.

## Shape

> **Every payload in this document is illustrative, never
> descriptive.** It shows the *shape* a conformant producer must
> satisfy. It is not a factual claim about anything in `crates/`, and
> no ADR, engine branch, test, or review may cite it as evidence of
> what an in-tree producer declares — cite the source file (ADR 0011,
> Amendment, Ruling 5). Where a shipped producer and an example here
> diverge, that is a conformance gap in the producer, not a
> contradiction in this contract. Example producer names are
> deliberately suffixed `-example` so they cannot be mistaken for one.

```json
{
  "cert_id":     "blake3:...",
  "schema":      "freshdag.certificate/v0.1",
  "artifact": {
    "id":       "blake3:...",
    "path":     "briefs/acme.md",
    "kind":     "text/markdown",
    "content_hash": "blake3:...",
    "size":     4213
  },
  "produced_by": {
    "computation_id": "opaque",
    "recipe":       "research-account",
    "recipe_hash":  "blake3:...",
    "adapter":      "freshdag-adapter-example/0.1.0",
    "started":      "2026-08-15T13:45:12.001Z",
    "ended":        "2026-08-15T13:46:07.892Z"
  },
  "depends_on": [
    {
      "key":            "file:///abs/path/ICP.md",
      "scheme":         "file",
      "trust_class":    "exact",
      "fingerprint":    "blake3:1a2b...",
      "observed_at":    "2026-08-15T13:45:14.220Z",
      "produced_by":    null
    },
    {
      "key":            "attio://company/acme",
      "scheme":         "attio",
      "trust_class":    "versioned",
      "fingerprint":    "version:42",
      "observed_at":    "2026-08-15T13:45:16.007Z"
    },
    {
      "key":            "https://acme.com/pricing",
      "scheme":         "https",
      "trust_class":    "versioned",
      "fingerprint":    "etag:\"abc123\"",
      "observed_at":    "2026-08-15T13:45:18.541Z"
    },
    {
      "key":            "web.search(\"acme pricing changes 2026\")",
      "scheme":         "web.search",
      "trust_class":    "volatile",
      "fingerprint":    "blake3:...",
      "observed_at":    "2026-08-15T13:45:22.812Z",
      "ttl_seconds":    3600
    }
  ],
  "comparator": {
    "name":     "exact"
  },
  "status": {
    "value":    "valid",              // "valid" | "stale" | "unknown" | "likely-valid"
    "checked":  "2026-08-15T13:46:08.001Z",
    "reasons":  []                    // populated when not "valid"
  },
  "observation_coverage": [
    { "producer": "freshdag-adapter-example", "version": "0.1.0",
      "role": "adapter", "emits": ["tool.*", "fs.read", "fs.write"],
      "partial": {
        "fs.*": { "reason": "blind-in-scope",
                  "note": "no visibility inside bash/task subprocesses" }
      } },
    { "producer": "freshdag-observer-example", "version": "0.1.0",
      "role": "observer", "emits": ["fs.read", "fs.write"],
      "partial": {
        "fs.read": { "reason": "over-approximates",
                     "note": "reads reported at directory granularity" }
      },
      "known_limitations": ["glibc only"] }
  ]
}
```

## Field Rules

- **`schema`** is mandatory and versioned. Consumers refuse unknown
  schema versions.
- **`artifact.content_hash`** is a BLAKE3 of the artifact's bytes
  (canonicalized per its kind).
- **`depends_on[].trust_class`** MUST be one of `exact`, `versioned`,
  `heuristic`, `volatile`. Absence is an error.
- **`depends_on[].fingerprint`** is a trust-class-tagged string
  (`blake3:...`, `sha256:...`, `version:...`, `etag:...`,
  `mtime:...`).
- **`status.value`** MUST NOT be `valid` if any dependency's trust class
  is `heuristic` or `volatile` — the highest achievable status is
  `likely-valid`. This is a JSON Schema-enforced assertion (see
  `schemas/certificate/v0.1.json`), not just a convention.
- **`status.reasons`** MUST be non-empty whenever `status.value != valid`.
- **`produced_by.recipe_hash`** MUST be present whenever
  `status.value` is `valid` or `likely-valid`.
- **`observation_coverage`** lists every producer that contributed to
  the certificate, with its coverage manifest version so downstream
  consumers can interpret silences.
- **`observation_coverage[].partial`** mirrors the producer's coverage
  manifest and MUST be carried onto the certificate, not summarized or
  dropped. A certificate that omits its producers' declared blindness
  cannot be re-checked by a third party, because the one fact that
  would flip the verdict is not in the document. See §Partial Coverage.

### Reason Codes

`status.reasons[].reason` is a **closed vocabulary**, not free text. The
authoritative list is `ReasonCode` in
`freshdag-core::dependency::validity`; the identical list is enumerated
in `schemas/certificate/v0.1.json` and `schemas/scenario/v0.1.json`, and
a test asserts all three agree. A consumer that encounters a reason
string outside the enum MUST treat the certificate as unreadable rather
than guess.

Codes are either **edge-scoped** — they explain one entry of
`depends_on[]`, and `dependency_key` names it — or **artifact-scoped** —
they explain the certificate as a whole, and `dependency_key` is the
empty string `""` (not `null`, not omitted).

| Code | Scope | Meaning |
| --- | --- | --- |
| `drift` | edge | A probe observed a fingerprint different from the recorded one. |
| `probe-unknown` | edge | A probe ran and could not decide, so it contributed no evidence. Distinct from `no-probe-available`, which asserts none ran. Usually accompanies an `unknown` edge; on a `volatile` edge inside a validated TTL the edge is `likely-valid` instead, because the TTL survives where the probe said nothing — and `--accept-likely-valid` does not lift that to exit 0. |
| `no-probe-available` | edge | No probe could be selected: none registered for the scheme, registration failed, arbitration tied, or the probe that recorded the fingerprint was removed (probe-contract §Anti-thrash Protocol, "Probe removal"). Distinct from `probe-unknown`, which asserts a probe executed. |
| `trust-class-heuristic-caps-at-likely-valid` | edge | The edge matched, but at `heuristic` trust; invariant #8 forbids reporting it as `valid`. |
| `trust-class-volatile-caps-at-likely-valid` | edge | A probe **ran and matched** on a `volatile` edge inside its TTL. |
| `volatile-within-ttl-unprobed` | edge | A `volatile` edge is inside a validated TTL and **no probe was consulted** — none registered for the scheme, arbitration tied, or the probe was removed. A probe that ran and could not decide is `probe-unknown`, not this: this code's name and ADR 0009 §Decision 2's emission condition both say *unprobed*. Same verdict as `trust-class-volatile-caps-at-likely-valid` on strictly weaker evidence. `--accept-likely-valid` does not lift an artifact carrying this code to exit 0. |
| `ttl-expired` | edge | A `volatile` edge's TTL elapsed without re-observation. |
| `probe-trust-demoted` | edge | The signal backing the recorded trust class disappeared; the engine emitted a `probe.trust_demoted` diagnostic and forced re-observation. |
| `coverage-deficit` | artifact | Effects occurred that no producer in `observation_coverage` claims to cover. |
| `producer-missing-from-coverage` | artifact | An event in the stream names a producer absent from `observation_coverage`. |
| `no-dependencies-observed` | artifact | The computation produced an artifact with zero observed dependencies. Absence of evidence is not evidence of freshness. |
| `dependency-changed-during-computation` | edge | The same dependency was observed more than once within one computation with different fingerprints: the input changed while the agent was reading it. The recorded fingerprint is one of at least two and nothing says which the computation consumed, so the edge is `unknown` and the artifact can never be `valid`. |
| `recipe-identity-unavailable` | artifact | The computation carries no `recipe_hash`, so no certificate about it may claim `valid` or `likely-valid` (§Field Rules, invariant #9). The dependencies may all have verified; what is missing is the identity of the computation they belong to. Some runtimes cannot supply one at all — Claude Code exposes no recipe — so for those this caps every artifact they produce. The engine **caps at `unknown`** rather than refusing to emit: the absence is a fact about the evidence, not a tool failure. |
| `unproven-dependency` | artifact | The store identified an observation naming a dependency but could not promote it to a verifiable edge — no fingerprint was observed, a `volatile` observation arrived with no TTL, or the payload was malformed. The dependency exists and its state is unknown, so it is deliberately absent from `depends_on` (recording it would fabricate evidence) and named here instead. `detail` carries the affected keys. Read-after-own-write and impure reads do NOT raise this: they are positive findings that no external dependency exists at that key. |

Three vocabularies in FreshDAG share spellings and must not be
conflated: reason codes (this table), probe results
(`probe.checked.result`: `match` | `drift` | `unknown`), and validity
statuses (`valid` | `likely-valid` | `stale` | `unknown`). In
particular, `probe-trust-demoted` is a *reason code on a certificate*;
`probe.trust_demoted` is the *diagnostic event* in the execution IR that
records the same occurrence in the append-only log.

**Ordering.** `status.reasons[]` is ordered by the position in
`depends_on[]` of the dependency each reason refers to; artifact-scoped
reasons sort after every edge-scoped reason. This ordering is
contractual: `cert_id` hashes it, and fixtures pin `reasons[0]`.

**Adding a code** is a non-additive change to this contract even though
it is additive to the Rust enum, because consumers validate against the
schema's enum. It follows the contract-change process.

### The `detail` field

`status.reasons[].detail` is an OPTIONAL free-text string carrying human
context for a code (`"http-status=429"`, `"tool_kind=bash"`). It exists
so `freshdag why` can be specific without growing the code vocabulary.

1. **`detail` is non-normative.** `status.value` MUST be a function of
   reason codes, trust classes, and probe verdicts alone. No consumer —
   engine, CLI, UI, or integration — may branch on `detail`'s content.
   Parsing `detail` to reach a validity decision is a contract
   violation.
2. **`detail` may never carry a distinction that changes behaviour.** If
   two situations warrant different statuses, or different handling by
   any code path, they require different `ReasonCode`s or a new typed
   field — not different `detail` strings.
3. **Absence of `detail` never strengthens a status.** A reason without
   `detail` means exactly what its code means.
4. **`detail` MUST be deterministic.** Given identical inputs and
   identical external responses, `detail` MUST be byte-identical across
   runs. It MUST NOT contain elapsed times, timestamps, PIDs, ephemeral
   ports, memory addresses, or retry counters. `detail` is inside the
   `cert_id` preimage; nondeterminism here makes certificates
   unreproducible and breaks the `reproducibility` fixture.
5. **`detail` MUST NOT carry secrets.** No credentials, no
   `Authorization` headers, no query strings that may embed tokens, no
   response bodies. Certificates are shareable primitives.
6. `detail` SHOULD be under 512 bytes.

### Coverage-Deficit Rule (invariant #7 enforcement)

The engine computes a **coverage deficit** for each computation:

```
observed_effect_kinds  = { fs.*, proc.*, net.* events emitted while
                           this computation was active }
covered_effect_kinds   = union over observation_coverage of the
                           kinds each producer declares it emits
deficit                = observed_effect_kinds - covered_effect_kinds
```

If `deficit` is non-empty (there is a category of effect the
computation exhibited but no producer claims coverage for), or if any
declared `tool.invoked` of kind `bash|task` occurred without a
corresponding observer producer in `observation_coverage`,
`status.value` MUST NOT be `valid`. This turns invariant #7 into a
machine-checked property of the certificate.

**Only an observer discharges the bash/task obligation, and only one
that claims to see reads.** A `tool.invoked` of kind `bash` or `task`
creates an observation obligation. A producer discharges it iff all
three hold:

1. `role == "observer"`. An adapter that declares `fs.read`/`fs.write`
   does NOT discharge it, however broad its `emits` list: adapters
   synthesize filesystem events from tool inputs they can see, and are
   blind inside subprocesses by construction.
2. `emits` covers **`fs.read`** specifically. Not `fs.write`, and not
   "either one." Validity is about *inputs*: a producer that sees only
   writes contributes zero dependency edges, so it cannot answer the
   question this rule asks even in principle.
3. **Every** `partial` entry whose pattern matches `fs.read` carries
   the reason `over-approximates`. Not "the most specific one" — a
   manifest is a conjunction of admissions, and a narrow entry must
   not annotate away a broad one (ADR 0011, Amendment, Correction 4).
   See §Partial Coverage.

This is why v0 on macOS (no observer) reports `unknown` — not `valid` —
on any computation that invoked `Bash`. That is correct behaviour, not
a defect.

### Partial Coverage

`observation_coverage[].partial` maps an event-kind pattern to a
**closed** reason plus a free-text note:

| `reason` | Meaning | Discharges an obligation? |
| --- | --- | --- |
| `over-approximates` | May report events that did not happen, or report them more coarsely than reality. Never misses one. | **Yes** |
| `under-approximates` | May miss real events of this kind. | **No** |
| `blind-in-scope` | Structurally cannot observe this kind in some scope (e.g. inside subprocesses). | **No** |

The direction of the error is the whole criterion. Over-approximation
produces spurious *dependencies*, hence spurious staleness, which
invariant #15 explicitly prefers; under-approximation and blindness
produce spurious *freshness*, which invariant #7 forbids.

This is why a blunt "any `partial` note disqualifies" rule is wrong,
on two grounds that do not depend on what any current producer
declares (ADR 0011, Amendment, Correction 2):

- **Invariant #13.** A blunt rule leaves `partial` free text, so the
  certificate records that an admission was made but not its
  *direction*. A third-party rechecker is left with "there was a
  note." The machine-readable `reason` is the whole point.
- **A blunt rule makes honesty punitive.** A producer that sees every
  event but reports coarsely — directory-granular reads, a hash taken
  at mmap time rather than at each fault — is strictly safer than one
  that reports nothing. Under a blunt rule its only way to keep
  discharging is to delete the note. An incentive to under-document is
  the opposite of what a coverage manifest is for.

Note what this does *not* claim: that any observer shipped in this
repository over-approximates. None is currently known to. The
vocabulary earns its keep as the certificate's machine-readable
explanation and as headroom for a producer that legitimately
over-approximates — not, today, as a behavioural difference from the
blunt rule.

`note` is **non-normative**, under the same rules as
`status.reasons[].detail`: no consumer may branch on it, it MUST be
deterministic (it is inside the `cert_id` preimage), and it MUST NOT
carry secrets. Deciding from the note rather than the reason is the
free-text mistake ADR 0006 exists to end.

A `partial` value MAY also be a bare string, which is the pre-ADR-0011
shape and is read as `under-approximates`. Unknown and legacy input
lands on "does not discharge" on purpose: a producer that deserves to
discharge must say so explicitly, and defaulting the other way is a
silent-wrong-answer generator on the invariant-#7 path.

When several `partial` keys match a kind (say `fs.*` and `fs.read`),
**every** match must discharge. A producer cannot annotate its way out
of its own broadest admission with a narrower entry.

## Certificate Update Semantics

Certificates are immutable. Rechecking an artifact produces a
successor certificate (`cert_id` differs). `freshdag check` prints the
latest status but never rewrites a certificate.

## Portability

Certificates are pure JSON, self-describing, and independent of the
FreshDAG version that produced them within the same schema major
version. A certificate emitted on machine A can be checked on machine B
if machine B has probe implementations for every dependency scheme
referenced.

## Anti-patterns

- Reporting `valid` when any probe returned `Unknown`.
- Reporting `valid` on an artifact whose recipe hash is `null`.
- Aggregating multiple observations into a single dependency entry
  without preserving the individual fingerprints.
- Rewriting a certificate in place.
- Emitting a certificate without an `observation_coverage` entry for
  every producer that contributed.
- Branching on `status.reasons[].detail` to make a validity decision.
- Encoding a new reason code inside `detail` instead of extending the
  `ReasonCode` enum through the contract-change process.
- Reporting `valid` on a computation that invoked `bash`/`task` when
  the only observer covering `fs.read` declares itself
  `under-approximates` or `blind-in-scope` for it.
- Dropping `observation_coverage[].partial` when writing a certificate,
  which makes the coverage-deficit rule uncheckable by anyone who does
  not have the producing store.
- Branching on `observation_coverage[].partial.*.note`, or classifying
  a producer's fidelity by pattern-matching that prose.
- Discharging a `bash`/`task` coverage obligation with an adapter's
  `fs.*` declaration rather than an observer's.

## Schema

The machine-readable JSON Schema lives at
`schemas/certificate/v0.1.json`. Consumers should validate against it.

## Change Policy

This contract falls under the contract-change policy
(`.claude/rules/architecture.md`).

While this contract's status is **provisional**, `v0.1` is a mutable
draft: a non-additive change may land inside `v0.1` provided it (a)
carries an ADR, (b) migrates every fixture and schema in the same PR,
and (c) is recorded in the changelog below. The justification is that no
certificate exists outside `fixtures/` and there is no external consumer
to break.

Once this contract's status becomes **stable**, `v0.1` freezes. From
that point every non-additive change — narrowing an enum, removing a
field, changing a field's type, tightening a constraint — requires a
version bump to `schemas/certificate/v0.2.json` and a new
`CERTIFICATE_SCHEMA_V0_2` constant. Additive changes (new optional
fields) continue to land in place.

### Changelog

- **v0.1 — `unproven-dependency` added.** Fourteenth reason code,
  artifact-scoped. The store has always recorded observations it could
  not promote to edges, with `ExclusionReason::is_unproven_dependency()`
  distinguishing "a dependency exists here but is unverifiable" from "no
  dependency exists here". The engine never consulted them: it evaluated
  `node.dependencies` only, so an input the producer saw but could not
  fingerprint was absent from the certificate entirely, and
  `no-dependencies-observed` fires only when the set is empty. A
  computation with three verified edges and one unfingerprinted read
  therefore certified over the three in silence. Found by verifier
  review 2026-08-17; the path became reachable when the Claude adapter
  started fingerprinting reads, since its byte cap and unreadable-file
  handling both produce exactly this exclusion. Additive on the wire and
  reader-breaking on the same terms as the entry below. Artifacts that
  were `valid` or `likely-valid` with an unproven exclusion present are
  now capped at `unknown`, which is the point.

- **v0.1 — `recipe-identity-unavailable` added.** Thirteenth reason
  code, artifact-scoped. Previously the engine *refused to seal* a
  certificate whose status would be `valid`/`likely-valid` without a
  `recipe_hash`, which reported a tool failure for an artifact whose
  evidence was merely incomplete — and for runtimes that can never
  supply a recipe (Claude Code exposes none), refusing was permanent.
  The engine now caps at `unknown` and attaches this code. Additive on
  the wire; a consumer holding the pre-change v0.1 validator rejects a
  certificate carrying it, because JSON Schema enums are closed. No
  artifact that was `valid` becomes less valid. **Corrected 2026-08-17
  after verifier review:** an earlier wording here claimed "the affected
  certificates could not be emitted at all before", which is false. A
  certificate already capped by a coverage code *was* emitted before —
  the old refusal only fired on `valid`/`likely-valid`, and such a
  certificate was already `unknown`. It now carries an additional reason
  and therefore a different `cert_id`, since `cert_id` hashes
  `status.reasons[]`. Any stored certificate of that shape will not
  compare equal across the upgrade. See ADR 0014.
- **v0.1 — reason-code enum widened 10 → 12** (recorded late).
  `volatile-within-ttl-unprobed` and
  `dependency-changed-during-computation` were added by ADR 0009 without
  a changelog entry. Flagged by the `architect` review of 2026-08-17:
  additive is right for *writers* and wrong for *readers*, since a
  consumer holding the older v0.1 validator rejects a valid new
  certificate. Recorded here so the omission does not recur silently.

- **v0.1 — Wave 2 Phase B.** `observation_coverage[].partial` added:
  present in the producer's coverage manifest all along, but dropped at
  the manifest→certificate boundary, which made the coverage-deficit
  rule uncheckable from the certificate alone. Each value narrowed from
  a free-form note to a closed `reason` (`over-approximates` |
  `under-approximates` | `blind-in-scope`) plus a non-normative `note`;
  the bare-string form still parses and reads as `under-approximates`.
  The bash/task obligation now requires `fs.read` specifically, rather
  than `fs.read` or `fs.write`. Additive on the wire; certificates that
  were `valid` behind a self-declaredly-blind observer, or behind an
  `fs.write`-only observer, become `unknown`. See ADR 0011.
- **v0.1 — Wave 2 Phase A.** `status.reasons[].reason` narrowed from a
  free-form string to the closed `ReasonCode` enum; wire form changed
  from snake_case to kebab-case; optional `status.reasons[].detail`
  added; `observation_coverage[].emits` added to the schema to match the
  Rust type; `observation_coverage[].role` added as a required field so
  the coverage-deficit rule can distinguish an observer from an adapter.
  Certificates emitted by Wave 1 code do not validate against the
  post-change schema. See ADR 0006.
