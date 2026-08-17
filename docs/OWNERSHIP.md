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
| `freshdag-cli` | `integration-engineer` (main.rs, argparse, exit codes, error surfacing) + `graph-engineer` (owns `freshdag-engine::PublicApi` trait consumed by CLI) | mutual signoff required for public-API-shape changes | User-facing exit codes are stable ABI. **Also an IR producer** — see below. |
| `freshdag-adapter-claude` | `claude-adapter` | `architect` | Reference adapter; sets precedent for future adapters. |
| `freshdag-observer` | `observer-engineer` | `architect` | Platform-specific implementations behind trait boundaries. |
| `freshdag-probes` | `probe-engineer` | `core-engineer` | One probe crate; probes register by scheme. |

## IR Producers

A **producer** is any crate that emits canonical IR events into a store
and publishes a coverage manifest. Producer obligations
(`docs/contracts/adapter-contract.md §Responsibilities`, §Testing;
`docs/contracts/execution-ir.md`) attach to the **declared `role` in the
manifest, not to the crate name** — ADR 0016. A crate can be a producer
without being named `freshdag-adapter-*`, and one is.

| Producer string | Crate | Role | Owner | Conformance fixtures |
| --- | --- | --- | --- | --- |
| `freshdag-adapter-claude` | `freshdag-adapter-claude` | `adapter` | `claude-adapter` | `fixtures/adapter-conformance/claude/` |
| `freshdag-observer-fsatrace` | `freshdag-observer` | `observer` | `observer-engineer` | `fixtures/observer-conformance/` |
| `freshdag-cli` | `freshdag-cli` (`mark`) | `adapter` | `integration-engineer` | **owed** — ADR 0016 Decision 3 |

`freshdag-engine` becomes a producer when ADR 0007 / W10 lands. Build it
with injected `Clock`/`IdGen` and conformance fixtures from the first
commit rather than retrofitting them (ADR 0016 §Consequences).

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
| `docs/adr/*.md` | proposer | Once merged, an ADR's decision text is immutable. It may be **superseded** by a later ADR, or **amended** — see below. |

### Amending a merged ADR

The rule above used to read "immutable except for superseding," and
practice diverged from it immediately: ADRs 0009, 0011, 0012, 0013 and
0014 all carry post-merge amendments. The practice is better than the
rule, so the rule changes to match it — with limits.

- An amendment is **appended** under a dated `## Amendment` heading,
  and says what it changes and who required it. The original decision
  text stays as written.
- A correction inside the body is permitted only when the body asserts
  something **false**, must be marked in place (`**Corrected
  <date>:** …`), and must say what the false claim was. ADR 0014's
  correction of its own test-guard claim is the model.
- A membership table or list that has gone stale is annotated as
  historical, **never rewritten** — ADR 0006's ten-row table is the
  model. An ADR records what was decided when.
- **An ADR whose own text says review is owed merges as `proposed`.**
  ADRs 0012 and 0013 both merged as `accepted` while saying sign-off had
  not happened. Also in `.claude/rules/architecture.md`.

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

## The CLI Exit-Code ABI

`ARCHITECTURE.md §11` fixes `0 = fresh`, `1 = stale`, `2 = unknown`,
`>2 = tool error`, and the table above calls these stable ABI requiring
mutual sign-off between `integration-engineer` and `graph-engineer`.
What that means was never written down, and on 2026-08-17 a change moved
a condition from exit 3 to exit 2 (ADR 0014) with no sign-off. Ruled
2026-08-17:

**A change is ABI-affecting, and needs the sign-off, if it moves any
input across a code boundary** — including between two non-zero codes,
and including a deletion of a public error variant that carried one.
"Both are non-zero" is not an exemption; `>2` and `2` mean opposite
things to a CI job (*ignore this result* versus *do not reuse it*).

**Two directions, two bars.**

- **Toward the conservative** — anything that previously exited `0` now
  exits non-zero, or a `>2` becomes a `1`/`2`. Sign-off is required and
  is expected to be granted; the change costs a recomputation, never a
  wrong answer (invariant #15). ADR 0014's move is this direction, which
  is why it was ratified after the fact rather than reverted.
- **Toward the permissive** — anything reaching `0` that did not
  before. Sign-off is required, plus a test that the new `0` is
  reachable **only** through an explicit operator opt-in. `Exit::code`
  already carries three such tests
  (`valid_is_the_only_status_reaching_zero_unaided` among them); a
  permissive change adds one.

Retrospective ratification is available for the conservative direction
and **is not available** for the permissive direction. There is no
circumstance in which a path silently starts exiting `0`.

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
