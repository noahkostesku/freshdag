# Certificate conformance fixtures

Long-lived correctness fixtures for FreshDAG's trust model. These
fixtures assert the certificate contract's Field Rules
(`docs/contracts/certificate-contract.md`) and, by extension,
architectural invariants #6, #7, #9.

## Layout

Each fixture is a directory under `positive/` or `illegal/` with two
files:

- `certificate.json` — the certificate under test.
- `expected.json` — the expected outcome. Shape:

  ```json
  {
    "invariant_check": "pass" | "fail",
    "reason": "<free-text label; when `fail`, MUST match the InvariantError variant name>"
  }
  ```

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

Illegal:

- `heuristic-dep-marked-valid` — invariant #7 violation.
- `volatile-dep-marked-valid` — invariant #7 violation.
- `valid-without-recipe-hash` — invariant #9 violation.
- `stale-without-reasons` — invariant #6 violation.
- `naked-volatile` — Volatile dep without TTL.
- `empty-observation-coverage` — anti-pattern.
- `wrong-schema-version` — v0.2 schema string on a v0.1 certificate.

Missing (to add in follow-up workstreams):

- Coverage-deficit fixtures — need the engine's coverage-deficit
  computation to exist first (that's an engine test, not a pure
  certificate test).
- Malformed producer-binding fixtures — need engine-side validation.
- Unknown-future-field fixtures — depend on the ADR decision about
  envelope strictness (see `docs/contracts/execution-ir.md`).
