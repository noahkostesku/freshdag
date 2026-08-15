# Implementation wave prompts

Version-controlled execution plans for FreshDAG implementation waves.
A fresh Claude Code session acting as the release-manager can load a
stored prompt from here and execute it end-to-end without depending
on prior conversation history.

## Contents

- `wave-2.md` — current next implementation wave.

Completed-wave prompts may later be retained in this directory for
historical / reproducibility purposes. When a wave completes, its
prompt is not deleted; it is superseded by the next wave's prompt.

## Authority

Repository state and contracts are authoritative. If a stored prompt
becomes stale relative to the actual code — because commits landed
between when the prompt was written and when it is executed — the
prompt is a guide, not a source of truth. Trust the code, the
contracts under `docs/contracts/`, and the invariants in
`ARCHITECTURE.md §5`.

> A fresh Claude Code session should inspect the repository before
> executing a stored wave prompt. The prompt is an execution plan,
> not a replacement for the current code, contracts, or architecture.

If reality diverges from the prompt's assumed base state, the first
task is diagnosing the delta and reporting to the human. Do not
proceed with wave work until you understand why.

## Loading a wave prompt

The recommended one-line instruction for a fresh session:

```
Read CLAUDE.md and docs/prompts/wave-<N>.md, verify the actual repository state, then execute Wave <N> as the release-manager.
```

Do not skip the "verify the actual repository state" step. Every
wave prompt assumes a specific base commit and a specific test
count; if either has drifted, the delta must be understood first.
