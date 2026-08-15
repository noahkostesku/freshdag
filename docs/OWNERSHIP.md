# Ownership

Ownership is per file tree and per contract. Ownership does not mean
gatekeeping every PR touching a file — it means the owner is the one
who resolves ambiguity and signs off on contract changes.

Where an agent name is used below, see `.claude/agents/<name>.md` for
that agent's charter, allowed tools, and completion-report format.

---

## Crates

| Crate | Owner | Reviewers | Notes |
| --- | --- | --- | --- |
| `freshdag-core` | `core-engineer` | `architect` | Domain vocabulary; no I/O. Contract changes require `architect` sign-off. |
| `freshdag-store` | `store-engineer` | `core-engineer` | Append-only log + derived graph. |
| `freshdag-engine` | `graph-engineer` | `core-engineer`, `verifier` | Validity, invalidation, equivalence. |
| `freshdag-cli` | `integration-engineer` (main.rs, argparse, exit codes, error surfacing) + `graph-engineer` (owns `freshdag-engine::PublicApi` trait consumed by CLI) | mutual signoff required for public-API-shape changes | User-facing exit codes are stable ABI. |
| `freshdag-adapter-claude` | `claude-adapter` | `architect` | Reference adapter; sets precedent for future adapters. |
| `freshdag-observer` | `observer-engineer` | `architect` | Platform-specific implementations behind trait boundaries. |
| `freshdag-probes` | `probe-engineer` | `core-engineer` | One probe crate; probes register by scheme. |

## Documentation

| Path | Owner | Notes |
| --- | --- | --- |
| `README.md` | `architect` | Reviewed by `novelty-reviewer` on every change. |
| `ARCHITECTURE.md` | `architect` | Invariant changes go through ADR. |
| `CLAUDE.md` | `architect` | Constitutional; discuss before editing. |
| `docs/NOVELTY.md` | `novelty-reviewer` | Update in the same PR that discovers a new collision. |
| `docs/EVALUATION.md` | `eval-engineer` | Fixture format changes here first. |
| `docs/BUILD_PLAN.md` | `architect` | Update as workstreams unlock. |
| `docs/OWNERSHIP.md` | `architect` | This file. |
| `docs/contracts/*.md` | per contract; see below | Contract-change process applies. |
| `docs/adr/*.md` | proposer | Once merged, ADRs are immutable except for superseding. |

## Contracts

| Contract | Owner |
| --- | --- |
| `docs/contracts/execution-ir.md` | `architect` |
| `docs/contracts/adapter-contract.md` | `architect` |
| `docs/contracts/observer-contract.md` | `observer-engineer` |
| `docs/contracts/probe-contract.md` | `probe-engineer` |
| `docs/contracts/comparator-contract.md` | `core-engineer` |
| `docs/contracts/certificate-contract.md` | `core-engineer` |

## Repository-Level

| Concern | Owner |
| --- | --- |
| Root `Cargo.toml`, `[workspace.dependencies]`, `Cargo.lock` | `release-manager` |
| Per-crate `Cargo.toml` | that crate's owner |
| CI / build (`.github/workflows/`) | `integration-engineer` |
| `.claude/` config and agents | `architect` |
| `.github/` templates and labels | `integration-engineer` |
| `.github/CODEOWNERS` | `architect` |
| `schemas/execution-ir/**` | `architect` (semantic contract) + `core-engineer` (Rust encoding) |
| `schemas/certificate/**` | `core-engineer` |
| `schemas/coverage-manifest/**` | `architect` |
| `fixtures/**` | `eval-engineer` |
| `fixtures/adapter-conformance/**` | adapter author authors; `eval-engineer` reviews for correctness |
| `fixtures/observer-conformance/**` | `observer-engineer` authors; `eval-engineer` reviews |
| `fixtures/probe-conformance/**` | `probe-engineer` authors; `eval-engineer` reviews |
| `fixtures/certificate-conformance/**` (negative suite) | `core-engineer` authors; `verifier` reviews |
| `scripts/**` | `release-manager` |
| `apps/web/**` | **unowned pending v0 completion** — do not touch |
| Swarm coordination (S1 assignment, provisional-to-stable transitions, verifier bootstrapping) | `release-manager` |

## When Ownership Is Ambiguous

- Cross-cutting refactors → `integration-engineer`.
- New adapter → owned by the person who adds it; must follow adapter
  contract, `architect` review required.
- New probe → owned by the person who adds it; `probe-engineer`
  review required.
- New observer backend (a new platform or approach) → `observer-engineer`.

## Escalation

- Contract disagreement → `architect`.
- Novelty concern → `novelty-reviewer`, then `architect`.
- Correctness concern → `verifier`, then `architect`.
- Product / scope concern → `architect`.
