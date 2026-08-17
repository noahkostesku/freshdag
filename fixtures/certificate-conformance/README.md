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
  code from the closed `ReasonCode` set (kebab-case; see
  `schemas/certificate/v0.1.json`): `drift`, `probe-unknown`,
  `trust-class-heuristic-caps-at-likely-valid`,
  `trust-class-volatile-caps-at-likely-valid`, `ttl-expired`,
  `coverage-deficit`, `no-dependencies-observed`,
  `probe-trust-demoted`, `producer-missing-from-coverage`. Anything
  else fails to deserialize.
- `certificate.json`'s optional **`status.reasons[].detail`** is human
  context only (probe failure text, HTTP status). No decision may key
  off it.

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
  stated preference), never spurious freshness.

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
