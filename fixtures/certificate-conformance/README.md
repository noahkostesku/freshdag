# Certificate conformance fixtures

Long-lived correctness fixtures for FreshDAG's trust model. These
fixtures assert the certificate contract's Field Rules
(`docs/contracts/certificate-contract.md`) and, by extension,
architectural invariants #6, #7, #9.

## Layout

Each fixture is a directory under `positive/` or `illegal/` with two
required files and one optional one:

- `certificate.json` — the certificate under test.
- `expected.json` — the expected outcome. Shape:

  ```json
  {
    "invariant_check": "pass" | "fail",
    "reason": "<free-text label; when `fail`, MUST match the InvariantError variant name>",
    "coverage_check": "pass" | "fail",   // optional; required iff events.json exists
    "coverage_reason": "<InvariantError variant name when coverage_check is `fail`>"
  }
  ```

- `events.json` (optional) — an IR event stream. Present only when the
  fixture exercises `Certificate::check_coverage_deficit`, which is the
  one rule the certificate alone cannot answer: it asks what the
  computation *did*. A fixture that ships `events.json` MUST set
  `coverage_check`, and one that sets `coverage_check` without shipping
  `events.json` is a fixture-authoring error the harness reports.

Note that `invariant_check` and `coverage_check` are independent. A
certificate can be structurally flawless — every field rule satisfied,
`check_invariants` returning `Ok` — and still be a lie about the world,
which is exactly what the coverage-deficit fixtures encode. Those
fixtures live under `illegal/` with `"invariant_check": "pass"`.

### Two different fields named `reason` — do not confuse them

- `expected.json`'s top-level **`reason`** is a harness field. When
  `invariant_check` is `fail` it MUST be an `InvariantError` variant
  name (`NakedVolatile`, `EmptyObservationCoverage`, ...); when `pass`
  it is free-text prose. It is NOT a wire reason code.
- `certificate.json`'s **`status.reasons[].reason`** is a wire reason
  code from the closed `ReasonCode` set (kebab-case). Anything outside
  the set fails to deserialize.

  **The members are deliberately not listed here.** Read them from
  `docs/contracts/certificate-contract.md §Reason Codes`, which carries
  each code's scope and meaning, or from `ReasonCode` in
  `freshdag-core::dependency::validity`, which is the source of truth.
  `schemas/certificate/v0.1.json` is the machine-checkable mirror.

  An enumeration in this file would be a fourth copy of a vocabulary
  that changed four times on 2026-08-17, and the copy that used to be
  here was stale at 9 of 14 members within a day. ADR 0015 Decision 3
  makes every non-normative mention a pointer rather than a list.
- `certificate.json`'s optional **`status.reasons[].detail`** is human
  context only (probe failure text, HTTP status). No decision may key
  off it.

### Producer names here are illustrative, never descriptive

Fixture manifests describe the *shape* a producer may declare. They are
not factual claims about anything in `crates/`, and no ADR, engine
branch, test, or review may cite one as evidence of what a shipped
producer declares — cite the source file (ADR 0011, Amendment,
Ruling 5).

Every synthetic producer is therefore named for the behaviour it
illustrates and suffixed `-example`
(`freshdag-observer-coarse-reads-example`, not
`freshdag-observer-fsatrace`). Reusing a real producer string is what
led ADR 0011 to assert that the fsatrace observer over-approximates
when `crates/freshdag-observer/src/linux.rs` declares the opposite.
A fixture asserting that some observer discharges must not be readable
as asserting that *ours* does.

## The load-bearing rule

**Do not change core types to make an `illegal/` fixture pass.** The
`illegal/` fixtures encode the trust model. If a fixture starts
passing when it should fail, that is a correctness regression — fix
the code, not the fixture.

## What's exercised

`crates/freshdag-core/tests/certificate_conformance.rs` walks these
directories, parses each certificate, calls
`Certificate::check_invariants`, and asserts against `expected.json`.
Adding a new fixture requires no test-code changes.

## Coverage today

Positive:

- `exact-dep-valid` — an all-`exact` dependency artifact reports `valid`.
- `versioned-dep-valid` — `versioned` deps also reach `valid`.
- `heuristic-dep-likely-valid` — a `heuristic` dep caps at `likely-valid`.
- `volatile-dep-with-ttl-likely-valid` — a `volatile` dep with a TTL
  and non-empty `reasons` is a legal `likely-valid`.
- `unknown-with-reasons` — `unknown` status with populated `reasons` is legal.
- `over-approximating-observer-valid` — an observer whose `fs.read`
  coverage is `over-approximates` DOES discharge a `bash` obligation.
  This is the fixture that keeps the coverage-deficit rule from being
  inert: over-reporting costs spurious staleness (invariant #15's
  stated preference), never spurious freshness. The observer is
  hypothetical: no in-tree producer is currently known to qualify as
  `over-approximates` (ADR 0011, Amendment, Correction 3), so this
  fixture pins headroom in the rule, not present behaviour.

Illegal:

- `heuristic-dep-marked-valid` — invariant #7 violation.
- `volatile-dep-marked-valid` — invariant #7 violation.
- `valid-without-recipe-hash` — invariant #9 violation.
- `stale-without-reasons` — invariant #6 violation.
- `naked-volatile` — Volatile dep without TTL.
- `empty-observation-coverage` — anti-pattern.
- `wrong-schema-version` — v0.2 schema string on a v0.1 certificate.
- `blind-observer-marked-valid` — an observer that declares
  `blind-in-scope` on `fs.read` must NOT discharge the obligation from
  a `bash` invocation. This is the certificate the verifier reproduced
  at exit 0 before ADR 0011: two stores differing only in the
  observer's `partial` map both said "safe to reuse."
- `fs-write-only-observer-marked-valid` — an observer declaring only
  `fs.write` sees zero dependencies, so it cannot discharge a `bash`
  obligation. Validity is about inputs.

Missing (to add in follow-up workstreams):

- Malformed producer-binding fixtures — need engine-side validation.
- Unknown-future-field fixtures — depend on the ADR decision about
  envelope strictness (see `docs/contracts/execution-ir.md`).
