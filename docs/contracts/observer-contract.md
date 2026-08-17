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

> **Every example payload in this document is illustrative, never
> descriptive** (ADR 0011, Amendment, Ruling 5). It shows the *shape* a
> conformant observer must satisfy and is **not a factual claim about
> anything in `crates/`**. No ADR, engine branch, test, or review may
> cite it as evidence of what a shipped producer declares — cite the
> source file. Where a shipped observer and an example here diverge,
> that is a conformance gap in the observer, not a contradiction in this
> contract. Example producers are named `*-example` so they cannot be
> mistaken for one.

Every observer publishes:

```json
{
  "producer": "freshdag-observer-example",
  "version": "0.1.0",
  "role": "observer",
  "platforms": ["linux-x86_64", "linux-arm64"],
  "emits": [
    "fs.read", "fs.write", "fs.rename",
    "proc.spawn", "proc.exit"
  ],
  "partial": {
    "fs.write": {
      "reason": "over-approximates",
      "note": "rename-atomic writes are correlated at close; see §Required Behavior 3"
    },
    "fs.read": {
      "reason": "over-approximates",
      "note": "mmap reads are pessimistic: hashed at mmap time"
    }
  },
  "capabilities": {
    "symlink_resolution": "at-observation-time",
    "mmap_reads": "pessimistic (hash at mmap time, assume full read)"
  },
  "known_limitations": [
    "glibc only; musl targets require strace fallback",
    "cannot observe processes that fork before LD_PRELOAD attaches"
  ]
}
```

**What this example is demonstrating, and what it is not.** It shows an
observer that has *met* §Correctness Pitfalls #2 and §Required Behavior
#3 — hence `over-approximates` on both kinds, the one reason that
discharges an observation obligation. **No observer shipped in this
repository currently qualifies.** `crates/freshdag-observer/src/linux.rs`
declares `fs.read` as `blind-in-scope` and `fs.write` as
`under-approximates`, and the gaps behind those are goldened in
`fixtures/observer-conformance/fsatrace/known-gap/`. Read this block as
a target, never as a status report.

`partial` values are `{reason, note}` objects drawn from the closed
vocabulary in `docs/contracts/certificate-contract.md §Partial Coverage`
(`over-approximates` | `under-approximates` | `blind-in-scope`). A bare
string is the pre-ADR-0011 shape; it still deserializes and decodes as
`under-approximates`, the conservative answer, but new manifests MUST
use the object form. Where several `partial` patterns match one kind,
**every** match must discharge before the obligation is discharged.

Consumers use this manifest to know what "no event" means (invariant
#7: absence of an event from a producer that does not cover it does
NOT mean nothing happened).

**Coverage is declared in `emits`, never in `capabilities`.**
`CoverageManifest::covers()` reads only `emits`, so a kind that is
absent from `emits` is uncovered — that absence is exactly how an
observer honestly says "I cannot see this." `capabilities` is a
free-form map for claims that are not event-kind coverage (symlink
resolution strategy, mmap pessimism); putting `"fs.read": true` there
declares nothing, and a manifest that states its coverage only in
`capabilities` will fail to discharge the `bash`/`task` observation
obligation in `docs/contracts/certificate-contract.md
§Coverage-Deficit Rule`.

`role` is likewise load-bearing rather than descriptive: only a
producer with `role: "observer"` can discharge that obligation, because
only an observer sees below the agent-tool layer.

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

The set lives at `fixtures/observer-conformance/<backend>/`, split into
`conformant/` and `known-gap/`. A `known-gap/` case is goldened to what
the backend emits **today** and carries a `gap.md` naming the clause it
fails, so a non-conformance is executable rather than prose: a passing
case means "still broken, still known", and a **failing** one means
someone implemented the clause and the case should be promoted to
`conformant/`.

All three adversarial fixtures above currently sit in `known-gap/` for
the fsatrace backend. **No observer in this repository is
contract-conformant today**, and the fixture set says so out loud rather
than being tuned until it passes. The second bullet — machine-validating
the manifest against actual output — is not yet implemented at all.
