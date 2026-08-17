# Gap: reads are not marked mmap-pessimistic and carry no hash

**Fails:** observer-contract §Required Behavior #4 — "emit `fs.read`
with the file's hash at `mmap` time, marked
`read_kind: \"mmap-pessimistic\"`" — and §Correctness Pitfalls #2.

**Trace:** a read of a file the process will `mmap`. Note what the trace
does and does not show: fsatrace hooks the `open`/`openat`/`fopen`
family, and every mmap is preceded by an open, so the *path* does
appear. The file is not invisible.

**What this backend emits today:** `fs.read` with
`read_kind: "direct"`, `size: 0`, and no hash — for every read, mmapped
or not. Two things are wrong:

1. `read_kind` is asserted as `direct` when the backend cannot tell.
2. No content hash, so nothing downstream can compare this dependency
   against a later state. The read is recorded as *having happened*
   without recording *what was read*.

**Direction:** this is the over-approximating side — coarser than
reality, never missing an event. That distinction matters for the
coverage classification: it is emphatically NOT the reason this
backend's `fs.read` is `blind-in-scope`. That reason is `LD_PRELOAD`
evasion (setuid, static linking, raw syscalls), which no fix here
touches. Closing this gap does not make the observer dischargeable.

**To close it:** hash at open time and mark reads the process may map
`read_kind: "mmap-pessimistic"`.
