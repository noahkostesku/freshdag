# Novelty rules

## Read the firewall

Before writing any user-facing text (README, marketing, ADR
motivations, blog drafts), read `docs/NOVELTY.md §3` — the novelty
firewall. That list enumerates claims FreshDAG must not make.

## When to invoke the novelty-reviewer

Automatically invite `novelty-reviewer` on PRs that:

- Modify `README.md`.
- Modify `docs/NOVELTY.md`.
- Add or change ADR motivations.
- Add public marketing/positioning documents.
- Introduce a feature whose description overlaps with adjacent
  territory in `docs/NOVELTY.md §4` (generic tracing, static
  analysis, workflow authoring, semantic caching, …).

## When to update the firewall

Any PR that discovers a new collision — a system, paper, or product
we didn't know about — updates `docs/NOVELTY.md §1` in the same PR.
Do not defer this. Untracked collisions become surprise objections
at the worst moment.

## When to argue back

The novelty-reviewer's job is to argue that a proposed feature is
already known or uninteresting. When they raise an objection, the
correct responses are:

- Cite the specific narrow-thesis phrasing from
  `docs/NOVELTY.md §2` and show why the feature preserves it.
- Withdraw the feature.
- Update the novelty firewall to reflect the new information.

The wrong response is silent expansion of what we claim to have
invented.
