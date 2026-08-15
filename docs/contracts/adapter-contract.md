# Contract: Runtime Adapter

**Status:** provisional (v0.1).

**Owner:** `architect`.

**Governs:** every crate named `freshdag-adapter-*`. The Claude Code
adapter is the reference implementation.

**Invariants relied on:** #1, #2, #3, #14.

---

## Purpose

An adapter translates a specific agent runtime's telemetry into
FreshDAG's canonical execution IR
(`docs/contracts/execution-ir.md`). Adapters are the only place
runtime-specific concepts (hook payload shapes, callback trees, span
attribute names) are allowed to appear.

## Responsibilities

An adapter MUST:

1. **Emit canonical IR events** with stable, deduplicated `event_id`s.
2. **Bracket computations.** Emit exactly one `computation.started`
   before any dependency-observing event, and exactly one matching
   `computation.ended`.
3. **Preserve causal parents.** Set `parent_id` on any event that has a
   causal predecessor in the same producer (e.g., `tool.completed`
   references its `tool.invoked`).
4. **Publish a coverage manifest** naming which IR event kinds it emits
   and any partial coverage:

   ```json
   {
     "producer": "freshdag-adapter-claude",
     "version": "0.1.0",
     "role": "adapter",
     "emits": ["session.*", "computation.*", "tool.*", "fs.read", "fs.write"],
     "partial": {
       "fs.read":  "only from Read tool; subprocess reads via observer",
       "net.fetch": "only from WebFetch tool"
     }
   }
   ```

5. **Fail loudly on unknown runtime events.** If the runtime emits a
   payload the adapter cannot classify, emit a diagnostic event; do
   NOT silently drop it. Silent drops corrupt the dependency graph.

An adapter MUST NOT:

1. Introduce runtime-specific concepts into `freshdag-core` types.
2. Reinterpret events after emission (append-only).
3. Merge two runtime events into a single IR event without a
   deterministic rule that the observer contract permits.
4. Emit `Valid`-classed observations for anything it did not directly
   observe.

## Identity Model

Adapters define:

- **`session_id`** — the runtime's session/run identifier, opaque to
  the core.
- **`computation_id`** — the boundary of "one agent-produced artifact".
  The Claude Code adapter's default is *one Claude Code session = one
  computation*; more granular slicing is permitted.

The adapter documents its identity rule in its README. Changes to
`computation_id` semantics are a contract change.

## Ordering Guarantees

Adapters emit events in the runtime's causal order. If the runtime
provides parallel branches (e.g., Claude Code parallel Task
subagents), the adapter tags events with `parent_id` so a consumer can
reconstruct the DAG; it does NOT serialize them.

## Errors and Backpressure

Adapters MUST NOT block the underlying runtime on IR emission. If the
downstream sink is unavailable, the adapter buffers to a local
append-only file and drops the newest events with a diagnostic if the
buffer is exhausted (never the oldest — invariant #4).

## Configuration

Each adapter accepts:

- A sink URL (file path or unix socket) for IR events.
- An optional coverage-override file so users can suppress noisy event
  kinds (e.g., `fs.stat` in directories with millions of small files).

## The Claude Code Adapter (concrete)

Implementation notes for `freshdag-adapter-claude`:

- Registered as a Claude Code hook binary for the events
  `PreToolUse`, `PostToolUse`, `UserPromptSubmit`, `Stop`,
  `SubagentStop`, `PreCompact`, `SessionStart`, `SessionEnd`,
  `Notification` with matcher `.*`.
- Reads hook payloads from stdin (JSON per Claude Code hook spec).
- Optionally tails `transcript_path` to reconstruct `tool_use_id` →
  parent chains that individual hook payloads do not expose.
- Maps:
  - `PreToolUse` → `tool.invoked`
  - `PostToolUse` → `tool.completed`
  - `Read` tool → additional `fs.read` events synthesized from
    `tool_input`.
  - `Write`/`Edit` tools → additional `fs.write` events.
  - `Bash` tool → `tool.*` events plus a coverage note that filesystem
    effects INSIDE the subprocess are observer territory.
  - `mcp__<server>__<tool>` → `tool.invoked/completed` with
    `tool_kind: "mcp"` and normalized name `mcp/<server>/<tool>`.
  - `Task` tool → `computation.started` for the subagent (if the
    adapter's identity rule treats subagents as sub-computations).
- Coverage declaration in
  `crates/freshdag-adapter-claude/coverage.json`.

## Testing

An adapter is considered contract-conformant when:

- Its recorded event stream, when replayed into an in-memory
  consumer, reconstructs the fixture's dependency graph deterministically.
- Its coverage declaration is machine-checked against a golden set of
  emitted events.
- It survives adversarial fixtures in `fixtures/adapter-conformance/`
  (to be authored — see `docs/BUILD_PLAN.md`).
