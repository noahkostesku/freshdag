# Scenarios

Deterministic evaluation scenarios that drive the engine's Wave-2 tests.
See `docs/EVALUATION.md §2` for the fixture set and
`schemas/scenario/v0.1.json` for the format.

## What ships in W5 (this workstream)

Six baseline scenarios: `file-dep`, `irrelevant-file`,
`hidden-subprocess-dep`, `versioned-external-dep`,
`volatile-external-dep`, `coverage-deficit`. Each is a `scenario.json`
under `fixtures/scenarios/<name>/` conforming to the v0.1 scenario
schema.

## What does NOT ship in W5

The engine that actually consumes these. `crates/freshdag-engine` is
still a placeholder — its S1 landing lands with W3/W4.

`crates/freshdag-core/tests/scenario_wellformed.rs` currently
validates that every scenario file:

- deserializes into the expected shape,
- has a non-empty `input_observations`,
- has a `expected.certificate_status.value` that is one of the four
  legal statuses,
- for any expected `stale` / `unknown` / `likely-valid` status,
  carries at least one `reason_codes[]` entry (invariant #6 mirrored
  at the scenario level),
- and that every `reason_codes[]` entry is a member of the closed
  `ReasonCode` set — kebab-case wire form: `drift`, `probe-unknown`,
  `trust-class-heuristic-caps-at-likely-valid`,
  `trust-class-volatile-caps-at-likely-valid`, `ttl-expired`,
  `coverage-deficit`, `no-dependencies-observed`,
  `probe-trust-demoted`, `producer-missing-from-coverage`. See
  `schemas/scenario/v0.1.json`.

Once the engine exists, the same directory becomes the source of
integration tests: `freshdag test fixtures/scenarios/*` will execute
each scenario end-to-end and assert against `expected`.

## Adding a scenario

1. Pick a directory name that describes the invariant under test.
2. Author `scenario.json` following the schema.
3. Ensure it is deterministic — no wall-clock reads, no randomness,
   no uncontrolled network.
4. Run `cargo test -p freshdag-core --test scenario_wellformed` to
   confirm the file parses and satisfies the well-formedness rules.
