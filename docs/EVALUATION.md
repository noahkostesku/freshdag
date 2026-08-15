# Evaluation

FreshDAG's correctness claims are cheap to make and expensive to verify.
This document defines how we prove — before and while — the system does
what it says.

Two audiences:

- Contributors: use the fixture set below as your regression bar.
- Skeptics: use the metrics below to decide whether to trust us.

---

## 1. Principles

- **Correctness beats cache hit rate.** A wrong `fresh` is worse than a
  cautious `stale`. All evaluation metrics tolerate over-staleness;
  none tolerate missed-staleness.
- **Metrics fall out of real runs; benchmarks are last resort.** If we
  can't measure it from a natural workload, we probably shouldn't be
  optimizing it yet.
- **Determinism first.** Every fixture runs identically every time or
  it doesn't ship. Non-determinism inside FreshDAG is a bug.

## 2. Fixture Set (v0)

Ten deterministic scenarios covering the invariants that matter most.
Each fixture is a directory: `inputs/`, `recipe.{py,sh}` (the agent),
`mutate.sh` (change the world), `expected.json` (what should happen).
Runnable as `freshdag test fixtures/<name>`.

| # | Fixture | What it proves |
| --- | --- | --- |
| 1 | `file-dep` | `brief.md` depends on `notes.md`. Mutate `notes.md`. Expect `stale`. |
| 2 | `irrelevant-file` | Same recipe; mutate a file the agent never read. Expect `valid`. |
| 3 | `fan-out` | One source feeds three artifacts. Mutate source. Expect all three `stale`. |
| 4 | `early-cutoff` | Whitespace-only mutation to a dependency; recompute; new brief materially equivalent; downstream score does NOT rerun. |
| 5 | `hidden-subprocess-dep` | Agent shells out to `curl example.com/config.json`. Expect FreshDAG catches via observer. |
| 6 | `versioned-external-dep` | Agent calls `mcp://attio/company/acme@v42`. No version change → `valid`. Bump version → `stale`. |
| 7 | `volatile-external-dep` | Agent calls `time.now()`. Expect: refused as cacheable, or `unknown` after TTL. |
| 8 | `heuristic-probe-failure` | Pricing page returns 500 during check. Expect `unknown`, never `valid`. |
| 9 | `undeclared-dep` | Agent reads `~/.env`, not declared. Expect observer catches it and adds it to the certificate. |
| 10 | `reproducibility` | Run twice with identical inputs. Bytes identical (or within declared comparator tolerance). |

Each fixture is under 50 lines and runs in under 5 seconds. Total
suite in CI under one minute.

Additional conformance fixtures for individual subsystems live in
`fixtures/adapter-conformance/`, `fixtures/observer-conformance/`,
`fixtures/probe-conformance/`, `fixtures/comparator-conformance/`,
and — added after the eval-adversary review — a **negative
certificate suite** at `fixtures/certificate-conformance/`. The
negative suite hand-crafts illegal certificates (heuristic dep with
`status: valid`; `recipe_hash: null` with `status: valid`; missing
`observation_coverage`; `status: valid` with a coverage deficit) and
asserts the checker rejects all of them. This is the only mechanism
that turns the certificate contract's "Anti-patterns" section from
prose into a test.

### Reason-pinned assertions (invariant #6 enforcement)

An `expected.json` for any fixture whose expected `status.value !=
valid` MUST pin `status.reasons[]` — specifically the reason codes
and the dependency keys they refer to, not just the status value.
Rationale: a broken FreshDAG that returned `stale` on every check
would pass every non-`valid` fixture unless we pin the *why*. Fixtures
that only assert on `status.value` are non-load-bearing.

### Fixtures 4/7/8/10 — rewordings

The eval-adversary review flagged four fixtures as unbuildable as
originally specified. The current spec:

- **`early-cutoff-exact`** — whitespace-only mutation to a text
  dependency; comparator=`exact` on canonicalized whitespace; asserts
  no downstream rerun. (The original `early-cutoff` with `judge()` is
  deferred to the recomputation workstream.)
- **`volatile-refused-at-record`** — agent calls `time.now()`;
  adapter records the dependency as `volatile`; certificate emission
  asserts the value cannot promote to `valid`.
- **`volatile-unknown-after-ttl`** — advance a fake clock past the
  TTL; assert `status.value = unknown` and the reason names the
  volatile edge.
- **`heuristic-probe-failure`** — ships an in-process HTTP server
  bound to `127.0.0.1:0`; the server returns 500 during check; assert
  `status.reasons[0].reason == "probe-unknown"` and the dependency key.
  No CI-level network access.
- **`reproducibility`** — the "recipe" is a scripted `recipe.sh`
  writing fixed bytes; the fixture asserts certificate emission is
  bit-identical across two runs. Real-LLM reproducibility (temperature
  effects) belongs in Tier-2 metrics; it is not a v0 correctness bar.

### Missing fixtures added to the v0 backlog

Beyond the ten above, the following adversarial fixtures are required
before FreshDAG claims correctness. They are v0 blockers, not stretch
goals:

- `etag-lies` — server returns stable ETag while body BLAKE3 changes.
- `etag-304-stale` — 304 Not Modified but content actually differs.
- `toctou-artifact` — mutate the artifact between check open and hash
  finalize.
- `symlink-swap` — retarget a dependency symlink between run and
  check.
- `cert-negative-suite` — the six anti-patterns from the certificate
  contract.
- `mcp-nondeterministic-response` — identical `tool_input`, drifting
  `tool_response`.
- `empty-deps` — artifact with zero observed inputs (should be
  suspicious, not silently valid).
- `unicode-path` — NFC vs NFD normalization on paths.
- `clock-skew` — producer `observed_at` newer than consumer
  `checked`; TTL math must not go negative-fresh.
- `cycle` — cyclic dependency; detected at graph reconstruction; no
  infinite loop.

## 3. Metrics

Two tiers, on purpose.

### Tier 1 — Natural exhaust (ship in v0)

These are computed from real runs with no labels required.

| Metric | Where it comes from |
| --- | --- |
| Cache hit rate | fraction of `freshdag check` calls that returned `valid` |
| Wall time saved | `sum(t_agent) - sum(t_check)` on runs that avoided rerun |
| $ saved | same, denominated in provider-reported token cost |
| Replay determinism rate | fixture suite: `run twice, diff outputs` |
| Undeclared-dep catch count | events emitted by observer that were not in the adapter's declared inputs |
| Coverage silence rate | fraction of dependency edges backed by producers with `partial` coverage |

### Tier 2 — Requires labels (deferred to v1+)

Requires a ground-truth oracle. Not built in v0.

| Metric | What it needs |
| --- | --- |
| Invalidation precision | "should this have rerun?" labels |
| Invalidation recall | same |
| Missed-staleness rate | oracle for "output no longer valid" |
| Equivalence disagreement rate | human/judge labels for "materially equivalent" |
| Heuristic-probe false-fresh rate | ground-truth comparison against exact re-fetch |

We resist inventing a FreshDAG-bench until design partners generate the
workloads.

## 4. Regression Suite

- **CI:** the full v0 fixture set runs on every push and PR.
- **Nightly:** a live dogfood workflow (sales-brief agent against an
  Attio sandbox) runs once per day; cache hit rate is tracked as a
  time series.
- **Determinism check:** every fixture is run twice; any output diff
  fails CI. This proves the fixture is deterministic. To prove
  FreshDAG is deterministic, `freshdag check` is run twice on the
  same certificate against the same environment and byte-identical
  output is required — including reason ordering (timestamps are
  zeroed for the comparison).
- **CLI golden test:** the 30-second demo (§6) is asserted as a
  snapshot at `tests/cli-golden/demo.txt`. Any CLI output-format
  change that reflows the demo fails CI unless the golden is updated
  intentionally.
- **Mutation testing:** `cargo mutants` runs against the check
  pipeline. Any surviving mutant that flips `stale → valid` fails
  CI. This is the primary automated defense against invariant #7
  regressions and is the metric that would most raise a skeptic's
  confidence (per eval-adversary review).
- **Property tests** (see `.claude/rules/testing.md`):
  - Certificate schema round-trip (serialize → parse → BLAKE3 → assert
    `cert_id` stable).
  - Probe monotonicity: under no-mutation, repeated `Match` never
    emits `Drift` or `Unknown`.
  - Comparator symmetry and idempotence.
  - Coverage-manifest cross-check: for every declared capability, an
    operation that exercises it must produce a matching event.

## 5. Anti-goals

Do not:

- Publish precision/recall numbers computed against synthetic
  fixtures we authored.
- Optimize for cache hit rate at the expense of correctness.
- Add a "FreshDAG-bench" hall-of-fame comparison to competitors.
- Present natural-exhaust metrics as if they were labeled ground
  truth.

## 6. The 30-Second Demo

The believability bar for v0 is the following interaction:

```
$ freshdag run research_agent.py --account acme
  wrote acme-brief.md
  wrote .freshdag/acme-brief.cert.json  (5 file deps, 2 MCP calls)

$ freshdag check acme-brief.md
  FRESH   all 7 inputs unchanged (checked in 1.2s)

# [edit acme's pricing page in a sandbox]

$ freshdag check acme-brief.md
  STALE   pricing.acme.com content hash changed
          icp.md, notes.md, attio:acme record — unchanged
```

If this doesn't work end-to-end, v0 hasn't shipped.
