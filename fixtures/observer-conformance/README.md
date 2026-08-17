# Observer conformance fixtures

`docs/contracts/observer-contract.md §Testing` defines observer
conformance in terms of this set:

> An observer is considered contract-conformant when its output on the
> `fixtures/observer-conformance/` set matches the golden IR streams,
> the coverage manifest passes machine validation against actual output,
> and adversarial fixtures (rename dance, mmap read, symlink swap)
> produce the correct synthesized IR events.

The set did not exist until 2026-08-17. Its absence is how a real defect
shipped: the `m|<destination>|<source>` move line was parsed as a single
path, emitting `fs.write { path: "/dst|/src" }` — a fabricated write at
a path that cannot exist — while the backend's `partial` note claimed
the opposite behaviour. **A fixture is evidence; a note is a claim.**

## Layout

```text
fsatrace/
  conformant/<case>/
    trace.txt        raw fsatrace output, read as bytes
    expected.jsonl   canonical IR stream, compared byte for byte
  known-gap/<case>/
    trace.txt
    expected.jsonl   what this backend emits TODAY
    gap.md           REQUIRED: which contract clause it fails, and why
```

Harness: `crates/freshdag-observer/tests/observer_conformance.rs`.
Adding a case requires no test-code changes.

## `known-gap/` — the load-bearing idea

All three adversarial fixtures the contract names exercise Required
Behavior clauses this backend **does not implement**. There were three
options and only one is honest:

- Golden them under `conformant/` → asserts conformance that does not
  exist.
- Leave them out → the contract's own test list stays unimplemented and
  the gaps live only in prose.
- **Golden them as-is under `known-gap/`, each naming the clause it
  fails.**

The third makes each gap *executable*. A passing `known-gap/` fixture
means "still broken, still known." A **failing** one means someone
implemented the clause — the golden gains or changes events, the suite
goes red, and the fix is to move the directory into `conformant/` and
re-bless. The gap cannot be quietly forgotten, and it cannot be quietly
closed either.

`every_known_gap_names_the_clause_it_fails` enforces the `gap.md`, so
`known-gap/` cannot become a parking lot for anything inconvenient.

## Current gaps

| Case | Fails | Effect today |
| --- | --- | --- |
| `rename-dance` | Required Behavior #3 | The artifact the computation actually produced never appears in the IR stream — only the temp path does. |
| `mmap-read` | Required Behavior #4, Pitfall #2 | Every read is asserted `read_kind: "direct"` with no content hash, so the read is recorded as having happened without recording what was read. |
| `symlink-swap` | Required Behavior #2, Pitfalls #3 and #4 | Paths are neither canonicalized nor resolved; `raw_path` duplicates `path`. Undeclared in `partial` — a second defect. |

`rename-dance` and `mmap-read` are declared in the backend's coverage
manifest, so they reach the certificate. `symlink-swap` is not; it lives
only in `capabilities`, which §Coverage Manifest says "declares nothing"
and which `covers` never reads.

## Determinism

`.claude/rules/testing.md` requires every fixture to be deterministic.
`IrEvent::event_id` and `ts` are ambient non-determinism, so the parser
takes an injected `Clock` and `IdGen`
(`freshdag_observer::determinism`). The harness wires `FixedClock` +
`SeededIdGen`; production wires the real ones. `session_id` and
`producer_version` are pinned to fixture constants so goldens do not
move when a caller or the crate version does.

`rendering_a_fixture_twice_is_byte_identical` guards the injection
itself — if the parser ever reaches for a wall clock again, that test
fails even if the goldens happen to still match.

## Regenerating

```bash
FRESHDAG_BLESS=1 cargo test -p freshdag-observer
```

Review the diff. A golden that moves without a deliberate behaviour
change is a regression — except under `known-gap/`, where a golden that
moves is the good news.

## Not yet covered

- **The coverage-manifest half of §Testing.** "The coverage manifest
  passes machine validation against actual output" — i.e. cross-checking
  that every kind the manifest claims in `emits` actually appears, and
  that nothing outside it does. The manifest is asserted in unit tests
  today, not against these streams.
- **Backends other than fsatrace.** The tree is `fsatrace/` precisely so
  a second backend gets a sibling directory rather than a rewrite.
- **`proc.*` and `net.*`.** This backend emits neither; the fixture
  names should not imply otherwise.
