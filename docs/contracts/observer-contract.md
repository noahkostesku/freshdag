# Contract: Systems Observer

**Status:** provisional (v0.1).

**Owner:** `observer-engineer` (see `.claude/agents/observer-engineer.md`).

**Governs:** every implementation in `freshdag-observer/` and any future
subprocess/filesystem/network tracer.

**Invariants relied on:** #3, #4, #7, #10.

---

## Purpose

Observers see below the agent-tool layer. They exist because agents
delegate work to subprocesses (Bash, MCP servers running out-of-process)
whose I/O is invisible to the adapter. Observer output is compiled into
the same canonical IR (`docs/contracts/execution-ir.md`) as adapter
output.

## Coverage Manifest (mandatory)

Every observer publishes:

```json
{
  "observer": "freshdag-observer-fsatrace",
  "version": "0.1.0",
  "role": "observer",
  "platforms": ["linux-x86_64", "linux-arm64"],
  "capabilities": {
    "fs.read":  true,
    "fs.write": true,
    "fs.rename": true,
    "fs.stat": false,
    "fs.dirlist": false,
    "proc.spawn": true,
    "proc.exit": true,
    "net.connect": false,
    "net.fetch": false,
    "symlink_resolution": "at-observation-time",
    "mmap_reads": "pessimistic (hash at mmap time, assume full read)"
  },
  "known_limitations": [
    "glibc only; musl targets require strace fallback",
    "cannot observe processes that fork before LD_PRELOAD attaches"
  ]
}
```

Consumers use this manifest to know what "no event" means (invariant
#7: absence of an event from a producer that does not cover it does
NOT mean nothing happened).

## Required Behavior

An observer MUST:

1. Emit `fs.*`, `proc.*`, `net.*` IR events for the effects it covers.
2. Canonicalize paths at observation time. Resolve symlinks via
   `AT_SYMLINK_NOFOLLOW` + `readlinkat` and record both the requested
   path (`raw_path`) and the resolved identity `(dev, ino, generation)`
   where available.
3. Handle **rename-atomic writes**: correlate `write(foo.tmp)` +
   `rename(foo.tmp, foo)` and emit a synthetic `fs.write { path: foo }`
   on `close` at the rename target.
4. Handle **mmap** pessimistically: emit `fs.read` with the file's hash
   at `mmap` time, marked `read_kind: "mmap-pessimistic"`. Do not
   attempt to observe faulted pages.
5. Emit `Unknown` (via `probe.checked { result: "unknown" }` where
   applicable) rather than silence when observation degrades.
6. Never modify observed subprocesses. Landlock-style enforcement is a
   separate, opt-in feature and MUST NOT be enabled by default.

An observer MUST NOT:

1. Emit fabricated events for I/O it did not directly observe.
2. Aggregate observations across sub-processes into a single event.
3. Silently drop events on backpressure. Buffer to disk with the same
   append-only guarantee adapters have.

## Platform Matrix (v0)

| Platform | Coverage in v0 | Ceiling | Notes |
| --- | --- | --- | --- |
| Linux ≥ 5.13 | `fsatrace` (LD_PRELOAD, glibc) | `eBPF LSM + landlock` | Recommended default. |
| Linux < 5.7 or musl | `strace -f` | `strace -f` | Higher overhead; correct. |
| macOS | *no native syscall observation* | Linux VM tunnel (Lima/OrbStack) | Documented explicitly. Rely on adapter-declared inputs + user declarations. |
| Windows | `Detours` DLL (planned) | Minifilter + ETW | Not v0. |
| WSL2 | Treat as Linux | Treat as Linux | — |

On macOS, FreshDAG must clearly report "systems observation unavailable
on this platform" from `freshdag check` when the subprocess observation
would matter to the answer. Silent partial coverage is a bug.

## Correctness Pitfalls (observer implementers must handle)

1. **Partial-write-then-rename.** Covered above; correlate.
2. **mmap reads bypass `read()`.** Hash at mmap time; document
   pessimism.
3. **Symlink races (TOCTOU).** Resolve at observation time; record
   both requested and resolved paths.
4. **Case-insensitive filesystems and bind mounts.** Store inode
   identity where available.
5. **Fork/exec races** where a child touches files before the observer
   attaches. Use `PTRACE_O_TRACEEXEC` or the equivalent to close the
   gap.
6. **`/dev/urandom`, clock reads, PID/host-based non-determinism.**
   Emit `fs.read`/`proc.spawn` but mark payload `impure: true` so the
   engine treats it as volatile.

## Configuration

Each observer accepts:

- A sink URL for IR events.
- A path allowlist/denylist to reduce noise (e.g., exclude `/tmp/pipe*`).
- A coverage-override file to disable specific event kinds.

## Testing

An observer is considered contract-conformant when:

- Its output on the `fixtures/observer-conformance/` set matches the
  golden IR streams.
- The coverage manifest passes machine validation against actual output.
- Adversarial fixtures (rename dance, mmap read, symlink swap) produce
  the correct synthesized IR events.
