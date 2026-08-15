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
    "adapter":      "freshdag-adapter-claude/0.1.0",
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
    { "producer": "freshdag-adapter-claude", "version": "0.1.0" },
    { "producer": "freshdag-observer-fsatrace", "version": "0.1.0",
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
| `probe-unknown` | edge | A probe ran and could not decide. |
| `no-probe-available` | edge | No probe could be selected: none registered for the scheme, registration failed, arbitration tied, or the probe that recorded the fingerprint was removed (probe-contract §Anti-thrash Protocol, "Probe removal"). Distinct from `probe-unknown`, which asserts a probe executed. |
| `trust-class-heuristic-caps-at-likely-valid` | edge | The edge matched, but at `heuristic` trust; invariant #8 forbids reporting it as `valid`. |
| `trust-class-volatile-caps-at-likely-valid` | edge | The edge matched inside its TTL at `volatile` trust. |
| `ttl-expired` | edge | A `volatile` edge's TTL elapsed without re-observation. |
| `probe-trust-demoted` | edge | The signal backing the recorded trust class disappeared; the engine emitted a `probe.trust_demoted` diagnostic and forced re-observation. |
| `coverage-deficit` | artifact | Effects occurred that no producer in `observation_coverage` claims to cover. |
| `producer-missing-from-coverage` | artifact | An event in the stream names a producer absent from `observation_coverage`. |
| `no-dependencies-observed` | artifact | The computation produced an artifact with zero observed dependencies. Absence of evidence is not evidence of freshness. |

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

**Only an observer discharges the bash/task obligation.** Each entry in
`observation_coverage` declares a `role` (`adapter` | `observer` |
`probe`). A `tool.invoked` of kind `bash` or `task` creates an
observation obligation that ONLY a producer with `role: "observer"`
declaring `fs.*` coverage can discharge. An adapter that declares
`fs.read`/`fs.write` does NOT discharge it, however broad its `emits`
list: adapters synthesize filesystem events from tool inputs they can
see, and are blind inside subprocesses by construction. This is why the
adapter contract's own coverage example pairs `fs.read` with the partial
note "only from Read tool; subprocess reads via observer" — that note
describes a producer that cannot answer the question this rule asks.

This is why v0 on macOS (no observer) reports `unknown` — not `valid` —
on any computation that invoked `Bash`. That is correct behaviour, not
a defect.

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

- **v0.1 — Wave 2 Phase A.** `status.reasons[].reason` narrowed from a
  free-form string to the closed `ReasonCode` enum; wire form changed
  from snake_case to kebab-case; optional `status.reasons[].detail`
  added; `observation_coverage[].emits` added to the schema to match the
  Rust type; `observation_coverage[].role` added as a required field so
  the coverage-deficit rule can distinguish an observer from an adapter.
  Certificates emitted by Wave 1 code do not validate against the
  post-change schema. See ADR 0006.
