# Architecture rules

Applies to all agents. Read alongside `ARCHITECTURE.md`.

## The 16 invariants are non-negotiable

Every PR must honor `ARCHITECTURE.md §5`. If your change strains an
invariant, escalate rather than proceed.

Two invariants agents violate most often:

- **Unknown is not fresh.** Never let a code path produce `valid` from
  `unknown` evidence.
- **Adapters do not leak.** No `freshdag-adapter-*` type or concept in
  `freshdag-core`.

## Contract-change process

Modifying any of the following requires the process below:

- `docs/contracts/*.md`
- Corresponding types in `freshdag-core` (Dependency, Fingerprint,
  Validity, Artifact, Computation, Comparator, IR event enums)
- `schemas/*`

Process:

1. Label the PR `contract-change`.
2. In the PR description, answer explicitly:
   - Why is the existing contract insufficient?
   - Who is affected (crates + agents)?
   - What migration is required for downstream consumers?
   - What tests are affected or added?
   - What novelty implications does this have (see `docs/NOVELTY.md`)?
3. Wait for the `architect` review.
4. Merge only after every affected owner in `docs/OWNERSHIP.md`
   acknowledges.

No implementation agent may silently redesign a contract while
solving a local problem. If you're tempted, stop and file an issue.

## Adding a new adapter

- Create `crates/freshdag-adapter-<name>/`.
- Implement the adapter contract in
  `docs/contracts/adapter-contract.md`.
- Publish a coverage manifest.
- Add at least one fixture under
  `fixtures/adapter-conformance/<name>/`.
- Do NOT modify `freshdag-core` types to accommodate the adapter — if
  you need to, follow the contract-change process instead.

## Adding a new observer backend

- Land the platform-specific backend behind an existing trait in
  `freshdag-observer`; do not invent a new abstraction.
- Publish a coverage manifest naming the platform and its
  limitations.
- macOS: unless you have new information invalidating the observer
  memo, do NOT add native macOS observation. Document the gap; don't
  fake it.

## Adding a new probe

- Register against a scheme (`file://`, `https://`, `attio://`, …).
- Honor trust-class semantics from
  `docs/contracts/probe-contract.md`.
- Failure returns `Unknown`, not `Match` or `Drift`.
- Add a fixture in `fixtures/probe-conformance/<scheme>/`.

## Deferring vs. deleting

Deferred features (Windows observer, LangGraph adapter, remote store,
UI) live in `docs/BUILD_PLAN.md §7`. If you find yourself tempted to
delete a deferred item because it "isn't real yet," don't — deletions
here need `architect` approval.
