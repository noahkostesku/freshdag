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

### When the human owner directs a merge without steps 3–4

Added 2026-08-17, after six `contract-change` PRs merged in one day with
zero reviews, on self-approval, by the single agent that owned every
contract involved. The human owner directed each merge knowingly.

**That is legitimate.** The human owner is the principal; the escalation
path in `CLAUDE.md` terminates at them, and a rule the principal cannot
override is not a rule, it is a cage. Steps 3–4 are a *delegation
mechanism*, not a source of authority. Nothing merged that day is
reopened on process grounds — the retrospective review found the
substance overwhelmingly sound, and every material defect it did find
was found by a **verifier** reading code, not by anyone reading a PR
description.

**And the deferral is a debt, not a discount.** Three rules:

1. **A skipped review is recorded in the PR and settled later, never
   dropped.** A `contract-change` PR merging without step 3 states so in
   its description — *"merged on the owner's direction; `architect`
   review deferred"* — and the review happens. Four ADRs on 2026-08-17
   said sign-off was owed, which is what made this review possible; that
   is the standard.
2. **Self-approval is not review, and saying so is the author's job.**
   An agent reviewing its own contract change reports *"unreviewed"*,
   not *"approved."* If you are asked to verify something you authored,
   `CLAUDE.md` §Verifier Bootstrapping already tells you to decline.
3. **An ADR whose own text says review is owed merges as `proposed`,
   not `accepted`.** ADRs 0012 and 0013 both merged as `accepted` while
   stating sign-off had not happened; 0012 was corrected and the
   correction did not propagate to 0013, written hours later. This is
   the mechanical check that replaces remembering.

**What the day actually demonstrated.** Six unreviewed contract merges
produced no unsound contract. Two verifier passes produced two live
invariant violations, four false claims in the record, an aliasing bug,
and a coverage report optimistic in three ways — none of which any
amount of PR review would have caught, because all of them required
reading the other crate. The scarce resource here is **adversarial
reading of code**, not sign-off. Spend it there:

> **A `contract-change` PR that merges without step 3 owes a `verifier`
> pass on the implementing code, not just an `architect` pass on the
> contract.** The verifier must not be the agent that authored the
> change.

## Reviews that are owed even when the contract-change process does not apply

The list at the top of §Contract-change process is narrow on purpose.
Three change classes fall outside it and still require a named
sign-off. Getting one of these wrong is not a contract violation, and
it is not free either.

- **Breaking public API changes to a crate you do not own.** See
  `docs/OWNERSHIP.md` §Crates for owner and reviewers. Example: the
  `Clock` → `EvalClock` / `FixedClock` → `FrozenClock` rename in
  `freshdag-engine` (ADR 0013, correctly *not* a contract change,
  incorrectly unsigned).
- **CLI exit codes.** `docs/OWNERSHIP.md` §The CLI Exit-Code ABI. Moving
  an input between any two codes needs mutual sign-off; a change toward
  the permissive direction cannot be ratified retrospectively.
- **A crate becoming an IR producer.** ADR 0016: producer obligations
  follow the declared `role`, not the crate name. `architect` review,
  same as adding an adapter.

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
