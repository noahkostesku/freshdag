# CLAUDE.md — FreshDAG Project Constitution

This file is the project's constitution for Claude Code agents (and
humans, though it addresses agents primarily). It is intentionally
short. Deep detail lives in the documents this file points to.

If you are an agent reading this: read the linked docs before touching
code. Do not skip.

---

## Thesis (one paragraph)

FreshDAG dynamically infers the runtime dependencies of unmodified agent
computations, attaches machine-checkable validity certificates to
artifacts they produce, and uses that graph to explain freshness and
eventually perform minimal recomputation as the world changes. Its
first surface is **detection** (`freshdag check`) — recomputation and
equivalence-driven early cutoff come later.

## Architectural Invariants (must obey)

The 16 invariants in `ARCHITECTURE.md §5` are load-bearing. Two that
break agents most often:

- **Unknown is not fresh.** If you cannot prove a dependency is
  unchanged, do not report the artifact as valid.
- **Adapters do not leak into the core.** No `freshdag-adapter-*`
  concept enters `freshdag-core`. If you need to, stop and file an ADR.

## Repo Boundaries

- `freshdag-core` — domain model, no I/O, no runtime knowledge.
- `freshdag-adapter-*` — one adapter per external runtime; the
  Claude Code adapter is #1 but not privileged in the design.
- `freshdag-observer` — sub-agent-layer observation. Platform-specific
  implementations live behind an interface.
- `freshdag-probes` — external-state freshness queries.
- `freshdag-store` — append-only observations + derived state.
- `freshdag-engine` — validity, invalidation, equivalence.
- `freshdag-cli` — the primary v0 surface.
- `apps/web` — future UI; not part of v0.

Ownership is documented in `docs/OWNERSHIP.md`.

## Required Reading (before non-trivial work)

1. `README.md` — one-page framing.
2. `ARCHITECTURE.md` — full architecture; invariants; layer model.
3. `docs/NOVELTY.md` — what we are and are not.
4. `docs/BUILD_PLAN.md` — what is serial, what is parallel, what is next.
5. The contract for your subsystem in `docs/contracts/`.
6. `.claude/rules/` for the topic you are touching (architecture, git,
   testing, novelty).

## Contract-change Policy

Modifying anything in `docs/contracts/` or the corresponding types in
`freshdag-core` requires:

- Attaching the `contract-change` label to the PR.
- Stating in the PR description:
  1. Why the existing contract is insufficient.
  2. Who is affected (list crates + agents).
  3. Migration required for downstream consumers.
  4. Tests affected or added.
  5. Novelty implications (see `docs/NOVELTY.md`).
- Architect (or `architect` agent) sign-off before merge.

Implementation agents may NOT silently redesign contracts while solving
local problems. If you're tempted, stop and escalate.

Full policy: `.claude/rules/architecture.md`.

## Test Commands

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs all three. Do not merge red.

Fixture-level evaluation (`freshdag test fixtures/*`) lands with the
engine; see `docs/EVALUATION.md`.

## Definition of Done

A change is done when:

1. It compiles cleanly (`cargo build --workspace`).
2. It passes `cargo fmt --check` and `cargo clippy … -D warnings`.
3. It passes `cargo test --workspace`.
4. Any new public API is either documented or explicitly marked
   experimental (`#[doc(hidden)]` acceptable for scaffolding).
5. Any behavior change is covered by a test or fixture.
6. If it touched a contract, the contract-change process above ran.
7. The PR description names the invariants relied on and any it strains.

## Git Expectations

- Small commits with imperative subject lines.
- Feature branches; no direct pushes to `main`.
- Rebase, don't merge, unless the PR is genuinely a bundle.
- Never `--force-push` shared branches.

Full rules: `.claude/rules/git.md`.

## Worktree Isolation

Implementation agents running in parallel MUST use `git worktree`
isolation. Two agents in the same clone clobber `target/`,
`Cargo.lock`, and each other's uncommitted work.

Full rules: `.claude/rules/worktrees.md`.

## Verifier Bootstrapping

The `verifier` agent must NOT be the same agent that authored the
change under review. Until the harness enforces this mechanically,
the `release-manager` audits verifier assignments manually. If you
are asked to verify a change you authored, decline and route to a
different verifier.

## Escalation

If your task requires you to violate any of the following, stop and
report rather than proceeding:

- An architectural invariant.
- A contract, without following the contract-change process.
- The novelty firewall.
- The trust/correctness model (specifically, silently returning
  `fresh` where the evidence is `unknown`).

## What This File Is Not

- Not a knowledge dump. Deep material lives in `docs/`.
- Not a settings file. Runtime settings live in `.claude/settings.json`.
- Not agent instructions. Per-agent behavior lives in `.claude/agents/`.
