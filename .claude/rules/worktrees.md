# Worktree isolation for implementation agents

Implementation agents (core-engineer, store-engineer, graph-engineer,
claude-adapter, observer-engineer, probe-engineer, ui-engineer,
integration-engineer, eval-engineer) MUST use `git worktree` isolation
when running in parallel.

## Why

Two agents editing the same clone clobber `target/`, `Cargo.lock`,
and each other's uncommitted work. Branches are not sufficient —
switching branches in one clone forces both agents to see the same
working tree state.

## How

- If your agent tool supports `EnterWorktree` (Claude Code does),
  invoke it before your first write. This creates a new git worktree
  under `.claude/worktrees/<name>/` on a new branch.
- If you must operate manually: `git worktree add
  .claude/worktrees/<agent-name>-<topic> -b <agent-name>/<topic>` and
  `cd` into it.
- On completion, submit a PR from the worktree branch. Do NOT merge
  from within the worktree; that is the `release-manager`'s job.
- After the PR is merged, remove the worktree:
  `git worktree remove .claude/worktrees/<name>` (or the `ExitWorktree`
  tool with `action: "remove"`).

## What NOT to do

- Do not run two implementation agents in the same clone at the same
  time.
- Do not share a worktree between two logical agents.
- Do not delete another agent's worktree without confirmation.

## Exemptions

- `architect`, `novelty-reviewer`, `verifier` are read-mostly and
  short-lived; they may operate in the main clone if not blocking an
  implementation session.
- `release-manager` operates in the main clone (that's where merges
  happen).
