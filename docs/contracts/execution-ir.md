# Contract: Canonical Execution IR

**Status:** provisional (v0.1). Load-bearing enough to design against;
minor breaking changes expected before v0.

**Owner:** `architect`.

**Governs:** every producer (adapters, observers) and consumer (store,
engine) of runtime observations. If you emit or consume an observation,
you implement this contract.

**Invariants relied on:** #1, #2, #3, #4, #5, #10, #13, #14.

---

## Purpose

The execution IR is the stable, runtime-agnostic vocabulary that
FreshDAG's core reasons about. Runtime-specific concepts (Claude Code
hook payload shapes, MCP resource URIs, subprocess syscall enums) MUST
be compiled into this IR at the adapter/observer boundary. No downstream
subsystem may branch on runtime identity.

## Event Envelope

Every event carries:

```
{
  "event_id":     "uuid-v7 (monotonic-ish per producer)",
  "producer":     "freshdag-adapter-<runtime>" | "freshdag-observer-<backend>" | "freshdag-probe-<scheme>",
  "producer_version": "semver",
  "session_id":   "opaque, stable across an execution",
  "computation_id?": "opaque, stable across a computation (may span sessions)",
  "parent_id?":   "event_id of the causal parent event",
  "ts":           "RFC3339 with nanosecond precision",
  "kind":         "<one of the event kinds below>",
  "payload":      { ... kind-specific ... }
}
```

- `event_id` is UUIDv7 so ordering is derivable without a central clock.
- **`producer` is matched by exact string, so the shape above is
  load-bearing rather than decorative.**
  `Certificate::check_coverage_deficit` builds a set of the producers
  named in `observation_coverage` and rejects any event whose `producer`
  is not literally in it — `producer-missing-from-coverage`, and the
  computation's silences become uninterpretable. An earlier revision of
  this block wrote `"adapter-claude" | "observer-fsatrace" |
  "probe-http"`, none of which is a string any in-tree producer emits;
  a reader copying it would have produced events that fail to attribute.
  The real values are the crate names: `freshdag-adapter-claude`,
  `freshdag-observer-fsatrace`, `freshdag-probe-file`. The placeholders
  above are angle-bracketed so they cannot be mistaken for literals
  (ADR 0011, Amendment, Ruling 5).
- `session_id` is defined by the adapter; consumers treat it as opaque.
- `computation_id` may be absent for infrastructural events (e.g., a
  session-level probe) but is required for any event contributing to a
  dependency edge. **`computation_id` is a deterministic function of
  `(recipe_id_or_hash, canonicalized_declared_inputs,
  adapter_identity_rule_version)` computed in `freshdag-core`, not
  minted opaquely by the adapter.** This prevents two adapters
  observing the same runtime from forking the graph. Adapters that
  cannot supply a `recipe_id_or_hash` MUST synthesize one from a
  session-scoped stable identifier and record the rule used.
- `parent_id` is optional but strongly encouraged for causally-linked
  events (a `tool.completed` naming its `tool.invoked` as parent).
  Prefer the plural `causal_inputs` on new emitters; see below.

## Event Kinds (v0)

The set is small on purpose. Adapters extend by adding payload fields,
not by inventing kinds.

### Session lifecycle

- `session.started` — `{ agent_kind, cwd, source }`
- `session.ended` — `{ reason }`

### Computation lifecycle

- `computation.started` — `{ recipe_id?, inputs_declared?: [...] }`
- `computation.ended` — `{ status: "ok"|"error"|"aborted" }`

`computation` is FreshDAG's unit of "one agent-produced artifact".
Adapters MAY treat an entire session as one computation; more granular
adapters may treat each turn or sub-goal as a computation.

### Tool interaction

- `tool.invoked` — `{ tool_name, tool_kind: "builtin"|"mcp"|"skill"|"task"|"bash", tool_input, cwd }`
- `tool.completed` — `{ tool_output, is_error, duration_ms }`

Naming convention: MCP tools use `mcp/<server>/<tool>`. Skills use
`skill/<name>`. Bash subprocesses use `bash` (with `tool_kind = "bash"`).

`bash` is a distinct `tool_kind` — not a `tool_name` under `builtin` —
because the coverage-deficit rule in `docs/contracts/certificate-contract.md`
treats `bash|task` as invocations whose I/O the adapter cannot fully
see. The engine uses the `tool_kind` to decide whether an observer
producer must be present in `observation_coverage` before a
computation's status may become `valid`. `task` is likewise distinct:
subagent invocations expose only their prompt and final text to the
parent, so they carry an observation-coverage obligation.

### Diagnostic

- `diagnostic` — `{ message: string, ...producer-defined fields }`

Producers emit `diagnostic` when they encounter a runtime event they
cannot classify (see `docs/contracts/adapter-contract.md
§Responsibilities #5`), when back-pressure forces them to drop the
newest events (`§Errors and Backpressure`), or when a probe declares
`probe.trust_demoted` (`docs/contracts/probe-contract.md §Anti-thrash
Protocol`). Silence is a bug; diagnostics are how producers surface
it.

### Filesystem effects (adapter- or observer-emitted)

- `fs.read` — `{ path, size, hash?, follow_symlink_target? }`
- `fs.write` — `{ path, size, hash?, mode: "create"|"append"|"truncate" }`
- `fs.stat` — `{ path, existed }` (negative dependencies matter)
- `fs.rename` — `{ from, to }`
- `fs.unlink` — `{ path }`
- `fs.dirlist` — `{ path, entries_hash }`

Paths MUST be canonicalized to absolute paths at the emitter; the raw
observed path may be included as `raw_path`.

### Process effects

- `proc.spawn` — `{ parent_pid, child_pid, argv, envp_hash, cwd, exe }`
- `proc.exit` — `{ pid, exit_code, signal? }`

### Network effects

- `net.connect` — `{ family, addr, hostname? }`
- `net.fetch` — `{ url, method, response_hash?, status?, etag? }`

### External-state artifacts (probe-emitted)

- `probe.checked` — `{ scheme, key, observed_fingerprint, trust_class, result: "match"|"drift"|"unknown", retryable? }`

`retryable` is REQUIRED when `result` is `"unknown"` and MUST be absent
otherwise. It is the append-only record of `ProbeResult::Unknown {
retryable }`, which the certificate deliberately does not carry
(certificates explain; the log schedules). Without it, the certificate
would depend on evidence not reconstructable from the canonical log,
straining invariant #5. This field is additive to an existing kind and
therefore does not bump `schemas/execution-ir/`.

### Artifact production

- `artifact.produced` — `{ artifact_id, path?, content_hash, kind, produced_by: computation_id, comparator: "exact"|"json-structural"|... }`

## Ordering

Events within a producer are totally ordered by `event_id` (UUIDv7
carries a monotonic timestamp). Across producers, a partial order is
defined by `causal_inputs` when set; otherwise consumers use the
canonical linearization below.

Adapters and observers MUST NOT reorder events they emit. Consumers
MUST tolerate mild reordering across producers (races between a
`fs.read` from an observer and a `tool.completed` from an adapter).

**Canonical linearization for deterministic replay.** For any set of
events being materialized into derived state, the total order is
`(ts, producer, event_id)` — lexicographic on the tuple, ties broken
first by producer name then by event UUID. This is the ordering
invariant #5 relies on. Replay under this rule is deterministic
regardless of the physical order events landed on disk.

## Causal Predecessors

An event MAY carry `causal_inputs: [event_id]` naming the events whose
outputs it consumed. Unlike the earlier single-parent shape,
`causal_inputs` is a *list* — Task subagents that join, tool calls
that depend on both a prior tool call and a filesystem write, and any
DAG-shaped causality is representable.

If an emitter can only identify a single causal parent, it MAY emit
`causal_inputs: [<one>]`. Consumers reconstructing the graph treat a
missing or empty list as "no known causal predecessor" — never as "no
causal predecessor exists."

`parent_id` is retained for backwards-compatibility as a shorthand:
setting `parent_id` is equivalent to `causal_inputs: [parent_id]`.
New adapters and observers should prefer `causal_inputs`.

## Coverage Declarations

Each producer publishes a static coverage manifest (see
`docs/contracts/observer-contract.md` for observers,
`docs/contracts/adapter-contract.md` for adapters). Consumers use this
manifest to know what "no event" means: silent because the producer
does not cover this signal is different from silent because nothing
happened.

## Non-goals for v0

- No streaming compression scheme; JSONL on disk is fine.
- No cross-machine event routing beyond a local unix socket / file.
- No schema versioning beyond `producer_version` and the file location
  (`schemas/execution-ir/v0.1.json`).

## Change Policy

This contract falls under the contract-change policy
(`.claude/rules/architecture.md`). Non-additive changes require an ADR
and a version bump on `schemas/execution-ir/`.
