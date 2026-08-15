# Git rules

## Branching

- Never push directly to `main`.
- Feature branches: `<owner-agent>/<short-topic>`, e.g.,
  `probe-engineer/http-etag`.
- Rebase, don't merge, unless the PR is genuinely a bundle.

## Commits

- Imperative subject line under 70 characters.
- Body wraps at 72 characters; explains *why*, not *what*.
- One logical change per commit where practical.

## PR hygiene

- Include a filled-out `.github/PULL_REQUEST_TEMPLATE.md`.
- Reference the invariants your change relies on.
- If the change touches a contract, apply the `contract-change`
  label and follow `.claude/rules/architecture.md`.
- Do not merge with red CI. Do not skip hooks.

## Force-push and destructive operations

Never, unless explicitly instructed by the human owner of the branch:

- `git push --force` (or `-f`)
- `git reset --hard`
- `git branch -D` on shared branches
- `gh pr close --delete-branch` on someone else's branch

Reversible destructive-adjacent operations (rebase, amend) on your own
in-progress branch are fine.
