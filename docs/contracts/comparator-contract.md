# Contract: Equivalence Comparator

**Status:** provisional (v0.1). Not exercised in v0 (which is
detection-only), but the shape is stable enough to design against.

**Owner:** `core-engineer`.

**Governs:** comparators used to decide whether a recomputed artifact
is materially equivalent to its prior version, driving early cutoff.

**Invariants relied on:** #6, #7, #8, #11, #15.

---

## Purpose

Validity says "an input changed, so we must recompute." Equivalence
says "the recomputed output is or is not meaningfully different from
what we had before." Early cutoff — Salsa-style — depends on this
distinction.

Comparators are pluggable per artifact kind. FreshDAG ships a small
set; users may register custom comparators.

## Interface

```
fn compare(
    prior: &Artifact,
    fresh: &Artifact,
    config: &ComparatorConfig,
) -> ComparisonResult

enum ComparisonResult {
    Equivalent          { evidence: Evidence },
    Different           { evidence: Evidence, delta: Option<Delta> },
    Uncertain           { reason: String },        // e.g., LLM judge disagreed with itself
}
```

`evidence` is a structured, human-readable trace of how the decision
was reached, recorded on the certificate. This is non-optional: an
opaque "equivalent" with no audit trail cannot appear on a certificate.

## Built-in Comparators

| Comparator | Semantics | Notes |
| --- | --- | --- |
| `exact` | Byte equality after canonicalization (line endings, trailing whitespace, BOM). | Default for text/binary artifacts. |
| `json-structural` | Deep, order-insensitive JSON equality. Arrays are sequences unless annotated `set`. | Preferred for `.json` artifacts. |
| `set` | Set equality of enumerated members. | For artifacts that are unordered collections. |
| `numeric(tolerance, rel_or_abs)` | Element-wise numeric equality within tolerance. | For metrics, scores, embeddings. |
| `judge(rubric_id, model, threshold)` | LLM-as-judge with an explicit rubric. | Non-deterministic; must record model + version + rubric. |
| `custom(name)` | User-supplied function registered by name. | Escape hatch. |

`exact` and `json-structural` are pure. `numeric` is pure given the
tolerance. `judge` and `custom` are potentially non-deterministic; the
comparator identity, version, and outcome are recorded on the
certificate.

## Non-determinism Rules

- If a comparator is non-deterministic (any `judge` variant, some
  `custom` implementations), FreshDAG MUST record the model, prompt,
  temperature, and any seed used.
- If two invocations of a non-deterministic comparator disagree, the
  engine treats the result as `Uncertain` and refuses early cutoff.
  Better to over-recompute than to silently cut off propagation on a
  coin flip.

## Where Comparators Are Selected

- Adapter or user declares the intended comparator when the artifact is
  produced (`artifact.produced` IR event carries the comparator name).
- Users may override the comparator per artifact kind via CLI
  configuration.
- The engine defaults to `exact` if none is declared.

## Testing

- `fixtures/comparator-conformance/` will contain golden equivalence
  cases (e.g., JSON reordering, whitespace-only diffs, semantically
  equivalent numeric outputs).
- LLM-judge tests use recorded model responses to stay deterministic in
  CI.
