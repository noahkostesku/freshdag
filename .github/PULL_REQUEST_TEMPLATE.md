## Summary

<!-- One or two sentences on what changed and why. -->

## Invariants relied on

<!-- Cite by number from ARCHITECTURE.md §5. State any invariant this change
strains, and how it does not violate it. Example:
- #7 (Unknown is not fresh) — preserved; new probe returns `Unknown` on all
  failure modes.
- #14 (Adapter concepts do not leak) — preserved; hook payload shapes are
  contained inside freshdag-adapter-claude. -->

## Contracts touched

<!-- If none, delete this section. If any of these are touched, the PR must
be labeled `contract-change` and follow .claude/rules/architecture.md:
- docs/contracts/execution-ir.md
- docs/contracts/adapter-contract.md
- docs/contracts/observer-contract.md
- docs/contracts/probe-contract.md
- docs/contracts/comparator-contract.md
- docs/contracts/certificate-contract.md
- schemas/**
- Corresponding types in freshdag-core -->

## Novelty implications

<!-- If this PR expands FreshDAG's public surface, cite the row in
docs/NOVELTY.md that governs it and explain why the change does not become
the adjacent system. If a new collision was discovered, this PR updates
docs/NOVELTY.md too. -->

## Testing

<!-- What was tested and how. Include any fixtures added or updated. -->

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Relevant fixtures under `fixtures/*` pass (or N/A)

## Related

<!-- Links to issues, ADRs, or discussions. -->
