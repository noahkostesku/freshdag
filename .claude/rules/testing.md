# Testing rules

## Baseline commands

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Every PR must pass all three before it can merge. CI enforces.

## Fixture tests

The v0 fixture set lives in `fixtures/`. Once the engine and CLI can
run them, they become part of CI via `freshdag test fixtures/*`.

Adding a fixture is preferred over adding a unit test for
end-to-end behavior. Unit tests exist for pure logic (parsers,
canonicalization) inside individual crates.

## Determinism

Every fixture MUST be deterministic. Any test that emits `flaky` or
requires retry loops is broken; fix the test, don't add retries.

If a test depends on time, mock it. If it depends on randomness, seed
it. If it depends on the network, use a recorded fixture (VCR-style).

## The verifier agent

`verifier` is a separate agent that reviews correctness claims. It is
NOT the same agent that implemented the change under review. When you
add a subsystem, invite the verifier to run its checks; do not
self-certify complex claims.
