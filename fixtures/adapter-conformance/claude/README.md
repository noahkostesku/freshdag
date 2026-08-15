# Claude Code adapter conformance fixtures

Golden hook-payload → canonical-IR pairs. Exercised by
`crates/freshdag-adapter-claude/tests/adapter_conformance.rs`, which
replays each payload through the adapter with a deterministic clock and
id generator and compares the emitted JSONL **byte for byte**.

Adding a fixture requires ZERO test-code changes.

## Layout

```
<fixture-name>/
    payload.json     one raw Claude Code hook payload, exactly as it
                     arrives on stdin. Read as BYTES, never parsed by
                     the harness — a fixture may hold invalid JSON.
    expected.jsonl   the canonical IR event stream, one event per line.
```

## Determinism

`event_id` is a UUIDv7 and `ts` is wall-clock; both would make goldens
unstable. The adapter injects them (`FixedClock` + `SeededIdGen`), so
every golden starts at `2026-01-01T00:00:00Z` with ids counting from
`…-000000000001`. If a golden ever contains a real timestamp, an
ambient `OffsetDateTime::now_utc()` or `Uuid::now_v7()` has crept into
the compile path — that is the bug, not the fixture.

`producer_version` is the crate version (`0.0.0`). Bumping the crate
version requires re-blessing.

## Regenerating

```bash
FRESHDAG_BLESS=1 cargo test -p freshdag-adapter-claude
```

Review the diff. A golden that changes without a deliberate behavior
change is a regression, not a refresh.

## Coverage today

| fixture | pins |
|---|---|
| `read-tool-synthesizes-fs-read` | `Read` → `tool.invoked` + causally-linked `fs.read`; relative path resolved against `cwd` with `raw_path` retained |
| `write-tool-hashes-content` | `Write` → `fs.write` with a real size and BLAKE3 digest derived from `content` (no `size_observed` marker) |
| `bash-tool-no-fs-events` | `Bash` → `tool.invoked` with `tool_kind: "bash"` and **no** fs events, however file-shaped the command is |
| `task-tool-no-fs-events` | `Task` → `tool.invoked` with `tool_kind: "task"` and **no** fs events |
| `mcp-tool-name-normalization` | `mcp__linear__create_issue` → `mcp/linear/create_issue`, `tool_kind: "mcp"` |
| `unclassifiable-payload-diagnostic` | unknown `hook_event_name` → `diagnostic` (`unknown-hook-event`), never a silent drop |
| `malformed-json-payload` | truncated JSON on stdin → `diagnostic` (`malformed-payload`), no panic, no attribution to a computation |
| `read-tool-missing-file-path` | `Read` without `file_path` → `tool.invoked` is still recorded plus an `unparseable-tool-input` diagnostic |
| `post-tool-use-completed` | `PostToolUse` → `tool.completed` with `duration_observed: false`; no duplicate fs synthesis |
| `session-start-brackets-computation` | `SessionStart` (`startup`) → `session.started` + `computation.started` carrying the identity rule |
| `session-end-unknown-reason` | an unrecognized end reason yields `status: "aborted"`, not `"ok"` — invariant #7 at the session boundary |
| `unmapped-stop-hook` | `Stop` is recognized but unmapped → info-severity `unmapped-hook-event` diagnostic |

## The load-bearing rules

1. **`bash-tool-no-fs-events` and `task-tool-no-fs-events` must never
   grow an fs event.** The certificate contract's coverage-deficit rule
   keys off `tool_kind == "bash" | "task"` with no fs-covering observer
   present. An adapter that synthesized fs events there would fabricate
   an observation of a subprocess it cannot see and mask the deficit.
2. **No fixture may produce an empty event stream.** Silence is the one
   output the adapter contract forbids.
