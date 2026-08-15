# FreshDAG

**Agent outputs go stale.**

FreshDAG learns what they depend on, tracks when those dependencies
change, and tells you what needs to be recomputed.

```
$ freshdag run research_agent.py --account acme
  wrote acme-brief.md
  wrote .freshdag/acme-brief.cert.json  (5 file deps, 2 MCP calls)

$ freshdag check acme-brief.md
  FRESH   all 7 inputs unchanged  (checked in 1.2s)

# [pricing page changes]

$ freshdag check acme-brief.md
  STALE   pricing.acme.com content hash changed
          icp.md, notes.md, attio:acme record — unchanged
```

Every artifact FreshDAG produces gets a **validity certificate** — a
portable, human-readable manifest of what it depended on, how each
dependency was fingerprinted, and how strongly its freshness can be
trusted.

```text
briefs/acme.md

produced by  research-account@<fingerprint>
depends on
  ICP.md                exact       sha256:1a2b…
  attio.company(acme)   versioned   version:42
  acme.com/pricing      versioned   etag:"abc…"
  web.search(...)       volatile    ttl:3600s
status  VALID
```

## Status

FreshDAG is in its founding phase. This repository exists to hold the
architecture, contracts, evaluation plan, and coordination scaffolding
before large-scale implementation begins.

- **Vision.** Explain, invalidate, and minimally recompute agent-generated
  artifacts as the world changes.
- **Implemented today.** Repository scaffold, Rust workspace,
  documentation, contracts, agent topology. No dependency engine yet.
- **Planned next.** See `docs/BUILD_PLAN.md`.

## Why FreshDAG

Today, when an agent-produced artifact might be out of date, the answer
is almost always "rerun the agent to be safe." That costs real money on
workflows like sales research (Clay, Attio-style enrichment) where the
same account is re-processed weekly and 80% of inputs haven't changed.

FreshDAG's v0 wedge is **detection with receipts**: given an artifact,
say whether it is fresh, stale, or unknown — and show which inputs
justify the answer. Everything downstream (selective recomputation,
equivalence-based early cutoff, cross-artifact blast-radius reasoning)
builds on that primitive.

## Design Principles

FreshDAG's architectural invariants live in `ARCHITECTURE.md`. Three that
shape most decisions:

- **Unknown is not fresh.** If FreshDAG cannot prove a dependency is
  unchanged, it must not report the artifact as valid.
- **Heuristic freshness is never represented as exact freshness.** Trust
  classes are surfaced to consumers.
- **Correctness beats cache hit rate.** A wrong "fresh" is worse than a
  cautious "stale."

## Repository Layout

```
crates/
  freshdag-core/            domain model — dependencies, fingerprints, validity, artifacts
  freshdag-store/           append-only observation log + derived graph state
  freshdag-engine/          validity evaluation, invalidation, equivalence
  freshdag-cli/             freshdag CLI
  freshdag-adapter-claude/  Claude Code adapter (adapter #1, not the architecture)
  freshdag-observer/        systems-level observers (subprocess, filesystem)
  freshdag-probes/          external-state probes (HTTP ETag, versioned APIs, ...)
apps/web/                   future graph UI
docs/
  contracts/                stable interfaces subsystems implement against
  adr/                      architecture decision records
  concepts/                 background material
schemas/                    machine-readable versions of the contracts
fixtures/                   deterministic scenarios used for evaluation
.claude/                    Claude Code project configuration and agent topology
```

## Contributing

- Read `CLAUDE.md` first — it is the project's constitution.
- Load the relevant contract in `docs/contracts/` before implementing.
- Contract changes require a `contract-change` PR label and the process
  described in `.claude/rules/architecture.md`.

## Why Not…

- **LangSmith / Langfuse / AgentOps.** Tracing records what happened.
  FreshDAG uses traces to decide what is still true. It is a consumer of
  traces, not another trace store.
- **Bazel / Nix / Buck2.** They assume hermetic, declared inputs.
  Agent computations read freeform files, call live APIs, and produce
  non-deterministic outputs. FreshDAG borrows their fingerprint and
  early-cutoff patterns while replacing the hermeticity assumption with
  runtime observation and trust classes.
- **Dagster / Airflow with LLM operators.** They orchestrate a DAG you
  author. FreshDAG infers the DAG from an agent's actual runtime
  behavior; you do not rewrite your workflow to adopt it.
- **LangGraph checkpoints and time-travel.** They give you replay of an
  authored graph. FreshDAG gives you invalidation of an inferred one,
  across sessions, as external state changes.
- **FreshLLMs and temporal RAG.** They answer "is this factual answer
  still true?" via re-retrieval. FreshDAG answers "is this artifact
  still valid?" via its dependency graph.
- **Scheduled reruns / cron.** They recompute unconditionally.
  FreshDAG recomputes only what changed.

See `docs/NOVELTY.md` for the full collision table and novelty firewall.

## License

MIT. See `LICENSE`.
