# Gap: paths are not canonicalized and symlinks are not resolved

**Fails:** observer-contract §Required Behavior #2 — "canonicalize paths
at observation time. Resolve symlinks via `AT_SYMLINK_NOFOLLOW` +
`readlinkat` and record both the requested path (`raw_path`) and the
resolved identity `(dev, ino, generation)`" — and §Correctness Pitfalls
#3 (symlink TOCTOU) and #4 (case-insensitive filesystems, bind mounts).

**Trace:** a read through a symlink. The trace shows the path the
process asked for, not the file it got.

**What this backend emits today:** `path` and `raw_path` set to the same
raw trace string, with no resolution and no inode identity. Neither is
canonical, and `raw_path` carries no information `path` does not.

**Why it matters beyond tidiness:** the dependency is keyed on a name
that can point somewhere else by the time anything re-checks it. That is
the TOCTOU race Pitfall #3 names — the observer records
`link-to-secret`, an attacker repoints the link, and a later freshness
check compares against a different file entirely. The graph would report
`valid` on evidence about a file that is no longer the dependency.

**Undeclared, and that is a second defect.** Unlike the rename and mmap
gaps, this one appears in no `partial` entry. It lives only in
`capabilities["symlink_resolution"]`, and the contract's §Coverage
Manifest says `capabilities` "declares nothing" and is never read by
`covers`. It is inert today only because `fs.read` is already
non-discharging — it would ride along silently if that ever changed.

**To close it:** resolve at observation time, record `(dev, ino)`, and
keep `raw_path` as the requested path. Add a `partial` entry until then.
