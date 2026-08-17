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

## What W4 added

The engine that consumes these now exists. Two test layers run over this
directory:

- `crates/freshdag-engine/src/tests/scenarios.rs` executes every
  scenario end to end against a real `Engine` and asserts
  `expected.dependency_graph`, `expected.certificate_status`, and both
  halves of `expected.invalidation`. Run it with
  `cargo test -p freshdag-engine`. (`freshdag test fixtures/scenarios/*`
  becomes the same thing once the CLI lands in W7.)
- `crates/freshdag-core/tests/scenario_wellformed.rs` keeps validating
  the files' shape, independently of the engine.

W4 also extended each scenario's `input_observations` so that the
store's documented derivation table actually yields the expected edges.
The originals recorded only the tool-layer events; the additions are:
a `computation_id` on every event (edge-bearing events require one),
`computation.started` carrying the `recipe_hash` the certificate
contract requires before a status may be `valid`, content hashes on
`fs.read` (without which the store records `NoFingerprint` and derives
no edge), `probe.checked` events for external dependencies (a tool call
creates no edge — assigning a trust class to a tool result is the probe
contract's job), and an `artifact.produced` naming the artifact the
check is asked about.

Four scenarios also gained `invalidation.after_mutation_reason_codes`,
closing the residual gap ADR 0006 recorded.

### Well-formedness rules

`crates/freshdag-core/tests/scenario_wellformed.rs` validates that every
scenario file:

- deserializes into the expected shape,
- has a non-empty `input_observations`,
- has a `expected.certificate_status.value` that is one of the four
  legal statuses,
- for any expected `stale` / `unknown` / `likely-valid` status,
  carries at least one `reason_codes[]` entry (invariant #6 mirrored
  at the scenario level),
- and that every `reason_codes[]` entry is a member of the closed
  `ReasonCode` set, in kebab-case wire form.

  **The members are deliberately not listed here.** Read them from
  `docs/contracts/certificate-contract.md §Reason Codes` or from
  `ReasonCode` in `freshdag-core::dependency::validity`, which is the
  source of truth; `schemas/scenario/v0.1.json` is the mirror this test
  validates against.

  The list that used to be here was stale at 11 of 14 members within a
  day of the vocabulary changing. ADR 0015 Decision 3 makes every
  non-normative mention a pointer rather than a copy.

## `input_probes`

Scripted probe answers, keyed by dependency key. Recognized fields:

| Field | Meaning |
| --- | --- |
| `no_change_yields` | `match` \| `drift` \| `unknown` before mutation |
| `after_mutation_yields` | the same, after mutation (`version_bump_yields` is an accepted synonym) |
| `trust_class` | trust class of the probe's observation; defaults to the recorded class |
| `observed_fingerprint` | fingerprint returned on `match`; defaults to the recorded one |
| `mutated_fingerprint` | fingerprint returned on `drift`; falls back to `version:<recorded_version + 1>` |
| `recorded_version` | source version token, used for the drift fallback |
| `retryable` | for `unknown` results only |

A scenario with no `input_probes` entry for a dependency exercises the
real "no probe is registered" path, which is `no-probe-available` and
therefore `unknown` — except for a `volatile` edge inside its TTL, which
`ARCHITECTURE.md §7` defines as `likely-valid` without a probe.

## Mutation model

- `mutated_dependency_keys` non-empty → each named key's scripted probe
  switches to its `after_mutation_yields` result. A key with no scripted
  probe changes nothing, which is what `irrelevant-file` tests.
- `mutated_dependency_keys` empty and `after_mutation` differing from
  `before_mutation` → the harness advances its injected clock past every
  declared TTL. This is `volatile-external-dep`.

## Adding a scenario

1. Pick a directory name that describes the invariant under test.
2. Author `scenario.json` following the schema.
3. Ensure it is deterministic — no wall-clock reads, no randomness,
   no uncontrolled network. The engine's clock is injected, so a
   scenario never observes real time.
4. Name producers `freshdag-adapter-*`, `freshdag-observer-*` or
   `freshdag-probe-*`; the harness derives each producer's
   `observation_coverage` role from that prefix and will refuse a name
   it cannot classify.
5. Run `cargo test -p freshdag-core --test scenario_wellformed` and
   `cargo test -p freshdag-engine`.
