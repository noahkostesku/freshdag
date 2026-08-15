# freshdag-adapter-claude

The Claude Code adapter: a hook binary that compiles Claude Code hook
payloads into FreshDAG's canonical execution IR.

Claude Code is **adapter #1, not the architecture** (invariant #2).
Nothing in this crate is re-exported into `freshdag-core`.

Contracts implemented: `docs/contracts/adapter-contract.md`,
`docs/contracts/execution-ir.md`.

---

## Installation as a hook

Claude Code runs a fresh process per hook event and feeds it one JSON
payload on stdin. Register `freshdag-claude-hook` for every event with
matcher `.*`:

```json
{
  "hooks": {
    "PreToolUse":       [{ "matcher": ".*", "hooks": [{ "type": "command", "command": "freshdag-claude-hook" }] }],
    "PostToolUse":      [{ "matcher": ".*", "hooks": [{ "type": "command", "command": "freshdag-claude-hook" }] }],
    "UserPromptSubmit": [{ "matcher": ".*", "hooks": [{ "type": "command", "command": "freshdag-claude-hook" }] }],
    "Stop":             [{ "matcher": ".*", "hooks": [{ "type": "command", "command": "freshdag-claude-hook" }] }],
    "SubagentStop":     [{ "matcher": ".*", "hooks": [{ "type": "command", "command": "freshdag-claude-hook" }] }],
    "SessionStart":     [{ "matcher": ".*", "hooks": [{ "type": "command", "command": "freshdag-claude-hook" }] }],
    "SessionEnd":       [{ "matcher": ".*", "hooks": [{ "type": "command", "command": "freshdag-claude-hook" }] }],
    "PreCompact":       [{ "matcher": ".*", "hooks": [{ "type": "command", "command": "freshdag-claude-hook" }] }],
    "Notification":     [{ "matcher": ".*", "hooks": [{ "type": "command", "command": "freshdag-claude-hook" }] }]
  }
}
```

Set the sink: `FRESHDAG_SINK=/path/to/ir.jsonl` (or `--sink PATH`).

**Never pass `--stdout` to a registered hook.** Claude Code interprets
a hook's stdout as control output; IR events there would corrupt the
session. `--stdout` exists for debugging only. The binary always exits
0 and writes every problem to stderr — an adapter must not block the
runtime (adapter contract §Errors and Backpressure).

## Mapping

| Claude Code hook | Canonical IR |
|---|---|
| `SessionStart` (`source: startup`) | `session.started` + `computation.started` |
| `SessionStart` (resume/clear/compact) | `session.started` + `computation-bracket-skipped` diagnostic |
| `SessionEnd` | `computation.ended` + `session.ended` |
| `PreToolUse` | `tool.invoked` (+ synthesized `fs.*`) |
| `PostToolUse` | `tool.completed` |
| `UserPromptSubmit`, `Stop`, `SubagentStop`, `PreCompact`, `Notification` | `diagnostic` (`unmapped-hook-event`, info) |
| anything else | `diagnostic` (`unknown-hook-event`, warning) |

| Claude Code tool | `tool_kind` | `tool_name` | synthesized `fs.*` |
|---|---|---|---|
| `Read` | `builtin` | `Read` | `fs.read` |
| `Write` | `builtin` | `Write` | `fs.write` (real size + BLAKE3 of `content`) |
| `Edit`, `MultiEdit`, `NotebookEdit` | `builtin` | as-is | `fs.write` (no size, no hash) |
| `Bash` | `bash` | `bash` | **none** |
| `Task` | `task` | `task` | **none** |
| `mcp__<server>__<tool>` | `mcp` | `mcp/<server>/<tool>` | none |
| `Skill` | `skill` | `skill/<name>` | none |
| everything else | `builtin` | as-is | none |

### Why `Bash` and `Task` emit no filesystem events

A hook payload exposes a Bash command line and a Task prompt — never
the syscalls they perform. Synthesizing an `fs.*` event from either
would be fabricating an observation (invariant #7, adapter contract
§MUST NOT #4).

Emitting nothing is what lets `Certificate::check_coverage_deficit`
see the gap: it keys off `tool_kind == "bash" | "task"` and forces a
non-`valid` status unless an fs-covering **observer** is present.
Filesystem effects inside those invocations are observer territory.

## Identity rule

**`claude/session-as-computation/v1`** — one Claude Code session is one
computation.

`docs/contracts/execution-ir.md §Event Envelope` requires
`computation_id` to be a deterministic function of
`(recipe_id_or_hash, canonicalized_declared_inputs,
adapter_identity_rule_version)` computed in `freshdag-core`. Claude
Code exposes no recipe, so this adapter uses the contract's documented
fallback ("adapters that cannot supply a `recipe_id_or_hash` MUST
synthesize one from a session-scoped stable identifier and record the
rule used"):

| argument | value |
|---|---|
| `recipe_id_or_hash` | `claude-code-session:<session_id>` |
| `canonicalized_declared_inputs` | `""` (the adapter observes, it does not declare) |
| `adapter_identity_rule_version` | `claude/session-as-computation/v1` |

The hash itself is `freshdag_core::computation::ComputationId::derive`
— **not** minted locally — so two producers observing the same runtime
under the same rule cannot fork the graph. The rule string is recorded
on every `computation.started` payload and in `coverage.json`'s
`capabilities`, so a consumer can always tell which rule produced a
given id.

Changing this rule is a contract change. Bump the version string; do
not mutate it, or ids minted under the old rule become
indistinguishable from ids minted under the new one.

### Known gap

`Computation::recipe_hash` cannot be populated from hook payloads, and
`computation.started` carries no `inputs_declared`. By invariant #9
this caps such computations below `valid` — correctly, but it means
this adapter alone cannot certify anything as fresh.

## Honesty markers on synthesized events

The adapter writes two extension fields onto payloads. Both exist
because the IR's typed fields are non-optional and a fabricated `0` is
still a claim. Extension by payload field is the mechanism
`execution-ir.md §Event Kinds` prescribes.

- **`observation: "pre-execution-intent"`** — on every synthesized
  `fs.read`/`fs.write`. These come from `PreToolUse`, i.e. before the
  tool ran. A tool call denied by permissions or failing at runtime
  still produces one.
- **`size_observed: false`** / **`duration_observed: false`** — the
  adjacent required field is a placeholder, not a measurement. Absence
  of the marker means the value was genuinely derived from the payload
  (as `Write`'s `size` and `hash` are, from its `content`).

`is_error: false` on `tool.completed` means "no error signal was
present in `tool_response`", not "the tool succeeded".

## Never silent

Every input produces at least one event. A payload the adapter cannot
classify becomes a `diagnostic` rather than a silent drop (adapter
contract §Responsibilities #5). Codes:

| code | severity | when |
|---|---|---|
| `malformed-payload` | warning | stdin is not a JSON object |
| `unknown-hook-event` | warning | unrecognized `hook_event_name` |
| `unmapped-hook-event` | info | recognized event, no IR kind for it |
| `missing-required-field` | warning | no `session_id` / `tool_name` / … |
| `unparseable-tool-input` | warning | no `file_path`, unresolvable path, … |
| `tool-name-normalization-failed` | warning | MCP/skill name could not be split |
| `computation-bracket-skipped` | info | resumed session, bracket already open |
| `coverage-override-suppressed` | warning | user config withheld events |
| `sink-backpressure-drop` | warning | newest events dropped at the byte cap |

Diagnostics carry the **keys** of the offending payload, never the
values: `tool_input` routinely contains file contents and prompts.
`diagnostic` itself is never suppressible.

## Determinism

`Compiler::compile_str` is a pure function of `(payload, clock, id
generator, config)`. Nothing in the compile path calls
`OffsetDateTime::now_utc()`, `Uuid::now_v7()`, or touches the
filesystem. Production wires `SystemClock` + `UuidV7Gen`; the
conformance harness wires `FixedClock` + `SeededIdGen`, which is what
makes `fixtures/adapter-conformance/claude/` byte-stable.

## Coverage manifest

`coverage.json` and `coverage::coverage_manifest()` are two spellings
of one fact; a test asserts they agree. Regenerate the JSON with:

```bash
cargo run -p freshdag-adapter-claude --example emit_coverage
```

## Tests

```bash
cargo fmt --check -p freshdag-adapter-claude
cargo clippy -p freshdag-adapter-claude --all-targets -- -D warnings
cargo test -p freshdag-adapter-claude
FRESHDAG_BLESS=1 cargo test -p freshdag-adapter-claude   # regenerate goldens
```
