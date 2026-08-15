# Fixtures

Deterministic evaluation scenarios used to prove FreshDAG correctness.

See `docs/EVALUATION.md §2` for the current fixture set and its
principles.

## Layout

Each fixture is a self-contained directory:

```
fixtures/<name>/
    inputs/            source files the agent reads
    recipe.{py,sh}     the agent that produces the artifact
    mutate.sh          simulates a world change
    expected.json      the expected freshdag verdict + reasons
```

## Adding a fixture

1. Pick a name that describes the invariant under test (e.g.,
   `heuristic-probe-failure`, not `test_42`).
2. Keep it under 50 lines and under 5 seconds.
3. Ensure determinism: no time-dependence, no randomness, no
   uncontrolled network. Use recorded HTTP fixtures where needed.
4. Register the fixture's `expected.json` schema per the current
   evaluation harness.
5. Run twice locally and diff; any non-determinism is a bug.

Conformance fixtures for individual subsystems live in:

- `fixtures/adapter-conformance/`
- `fixtures/observer-conformance/`
- `fixtures/probe-conformance/`
- `fixtures/comparator-conformance/`

These are governed by the same rules and land alongside each
subsystem's implementation.
