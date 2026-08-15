# ADR 0003: Claude Code is adapter #1, not the architecture

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** architect
- **Consulted:** novelty memo (Workstream A), agent-runtime memo (Workstream C)

## Context

FreshDAG is being built at a time when Claude Code is the most widely
used agent runtime among the target audience. It is tempting to treat
Claude Code as *the* runtime and bake its concepts into the core.

Doing so would guarantee two problems:

1. **Platform-owner risk.** Anthropic can ship a competing
   "staleness for artifacts" feature backed by hooks + transcripts in
   a week. If we are Claude-Code-shaped, we are absorbed.
2. **Integration ceiling.** Real GTM/coding pipelines use multiple
   runtimes. Being Claude-Code-only is a hard cap on adoption.

## Decision

Claude Code is treated as adapter #1 — reference implementation, first
to ship, first to prove out the adapter contract — but the core has
zero Claude Code concepts. This is architectural invariants #1, #2,
#14.

Practically:

- `freshdag-core` MUST NOT depend on `freshdag-adapter-claude`.
- `PreToolUse` / `PostToolUse` / hook JSON shapes MUST NOT appear
  outside `freshdag-adapter-claude`.
- The adapter contract (`docs/contracts/adapter-contract.md`) is
  designed for at least two implementations in mind before v1.

## Consequences

- More upfront design cost.
- Slower "Claude Code experience polish" in v0.
- Novelty firewall (`docs/NOVELTY.md`) has a defensible answer to
  "why can't Anthropic just ship this?" — because we're not
  Claude-Code-shaped.

## Rejected Alternatives

- **Claude Code-first, refactor later.** Every refactor of a
  runtime-coupled core in history has failed or taken years.
- **Adapter-agnostic but no reference adapter in v0.** Rejected as
  vaporware risk. We need one adapter proven end-to-end.
