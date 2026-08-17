# Dogfooding log

`BUILD_PLAN.md` W13: *"Ten ordinary sessions with the hook installed.
Mutate the world. Run `check`. Record the outcome here, **including the
sessions where FreshDAG saw nothing useful**."*

This file is the record. It is owned by `architect` + `eval-engineer`.
Entries are appended, never edited to look better: a session where
FreshDAG saw nothing is the most informative kind of entry this document
can hold, and `EVALUATION.md`, not `NOVELTY.md`, is where the claim is
won or lost.

## How to add an entry

1. Work normally with `freshdag-claude-hook` registered.
2. `freshdag coverage --store .freshdag --json` and record the figures
   verbatim. Do not round in your favour.
3. Note what you were actually doing, because working style dominates
   the result.
4. Record what `check` said about anything you marked, including
   nothing.

---

## Session 1 — 2026-08-17 — building FreshDAG itself

**Status: 1 of 10.** One session, one agent, one working style. Nothing
here is a finding yet.

### What the session was

A long implementation session on this repository: merging the Wave 2
architect-review PRs, then building W11 (hook → store, `freshdag mark`),
read fingerprinting, and `freshdag coverage` itself. Heavy `git`, `gh`,
`cargo`, and `python3` usage; almost all file access through `Bash`.

The hook was registered mid-session via `.claude/settings.local.json`
for all nine event types, so the log covers roughly the last third of
the work.

### Measured

Snapshot at `2026-08-17T20:30:24Z`, from `freshdag coverage --json`:

| | |
|---|---|
| events | 140 |
| computations | 1 |
| replay | deterministic |
| registered producers | `freshdag-adapter-claude/0.0.0` |
| unregistered producers | none |
| tool calls | `bash` 54, `builtin` 6 |
| **observable fraction** | **6 of 60 (10%)** |
| obligations raised | 54 |
| obligations dischargeable | **no** |
| dependency edges | 2 |
| excluded observations | none |
| artifacts | 0 |
| coverage silence rate | 100% |

An earlier snapshot the same session read 5 of 55 (9%). The ratio is
stable as the session grows, which is itself mildly informative: the
blindness is not a warm-up artifact.

### What FreshDAG saw

**Two dependency edges, from 140 events.** Both came from `Read` tool
calls, both correctly fingerprinted by the content source landed earlier
today.

**Fifty-four `bash` calls produced nothing but obligations.** Every one
raised a `bash`/`task` observation obligation, and no registered
producer can discharge them — there is no `Observer`-role producer in
this store, because the fsatrace observer is Linux-only and this session
ran on macOS.

`coverage silence rate` reads 100%: both edges come from a producer that
declares `partial` coverage on `fs.read`. That is honest — the adapter
genuinely under-approximates reads — but with n=2 edges the percentage
carries almost no information.

### What `check` said

Nothing was marked in this store, so `check` was never run against it.
The end-to-end behaviour was exercised separately by `scripts/demo.sh`
against a synthetic session, where the loop works: editing an input
yields `stale` at exit 1 naming the drifted dependency; unchanged yields
`unknown` at exit 2 (no recipe identity); marking an unrecorded file is
refused at exit 5.

**That gap is itself the entry's main finding.** In a real session there
was no obvious moment to run `freshdag mark`, and nothing prompted for
one. The command exists; the workflow around it does not.

### Correction, 2026-08-17 (verifier review)

**The 10% above is an upper bound, and the figures are miscounted in the
optimistic direction.** Three defects found after this entry was
written:

1. **Subagent delegations were counted as observable.** This runtime
   emits the tool as `Agent`; the adapter recognized only `Task`, so a
   delegation was classified `builtin`, raised **no** observation
   obligation, and counted as a tool FreshDAG can see into. Fixed;
   historical events in this store keep their original classification,
   because the log is append-only.
2. **`observable_fraction` measures the weaker quantity.** It counts
   tool calls the adapter can see into, not calls able to yield a
   dependency. Only `Read` can produce an edge — `fs.write` is an
   output. At this session's cutoff that is **2 of 60 (3.3%)**, not
   6 of 60.
3. `mcp`, `skill`, and a `tool.invoked` carrying no `tool_kind` at all
   are likewise counted as observable.

The arithmetic in the table below was independently re-derived from the
raw JSONL by a verifier and agreed exactly; what was wrong is the
*classification feeding it*, not the counting. Corrected figures are not
back-filled here — the entry stands as recorded, with this correction
appended, because rewriting a measurement after the fact is how a
dogfood log stops being evidence.

**The direction of every error was the same: reported coverage was
better than reality.** That matters because this number argues about
whether to build an observer, and the bias ran against building one.

### Honest reading

`BUILD_PLAN` §6.2 pre-committed to the risk: *"The honest outcome may be
'we saw 20% of it.' That is the point."* The first measurement came in
at **10%**, half the anticipated figure.

Three caveats that cut against over-reading it:

1. **n = 1**, and this agent's style is unusually `bash`-heavy. An agent
   that used `Read`/`Edit` for file access would score far better.
2. **The session was building FreshDAG**, not using it on a normal
   workload. Repository work skews toward `git`/`cargo` shell calls.
3. **The hook joined late**, so the log is a tail, not a whole session.

What survives those caveats: on macOS, with no observer, an entire class
of file access is structurally invisible, and the system correctly
reports that it cannot see it rather than guessing. The
coverage-deficit machinery works. The coverage does not.

### What it suggests, pending nine more sessions

An fs-covering observer looks less like an optimization and more like
the difference between a tool that decides and a tool that abstains. But
that is a hypothesis from one data point, and the honest next step is
sessions 2–10 rather than a build decision made on this entry alone.

### Open, and not fixed by this entry

- `valid` is unreachable through this adapter (no recipe hash), so the
  reachable states are `stale` and `unknown`. Note this is the *only*
  thing preventing several latent freshness defects from becoming live;
  it is masking, not safety.
- No `verifier` pass has run on any of the day's work.
- The thirteenth reason code, for capping on missing recipe identity,
  is still owed as a contract change.
