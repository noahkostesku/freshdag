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

## Schema

The machine-readable JSON Schema lives at
`schemas/certificate/v0.1.json`. Consumers should validate against it.
