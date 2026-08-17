# Gap: no synthetic write at the rename target

**Fails:** observer-contract §Required Behavior #3 — "correlate
`write(foo.tmp)` + `rename(foo.tmp, foo)` and emit a synthetic
`fs.write { path: foo }` on `close` at the rename target."

**Trace:** the ordinary atomic-write dance. A tool writes
`brief.md.tmp`, then renames it over `brief.md`.

**What this backend emits today:** one `fs.write` at
`/repo/brief.md.tmp`. The move line emits nothing, so **the artifact the
computation actually produced — `/repo/brief.md` — appears nowhere in
the IR stream.**

**Why that is the safe direction.** Missing the write is
under-approximation: a downstream consumer sees no evidence the artifact
was produced and cannot claim it fresh. The alternative this replaced
was worse — the parser split the move line on the first `|` and emitted
`fs.write { path: "/repo/brief.md|/repo/brief.md.tmp" }`, a fabricated
write at a path that cannot exist. Inventing an edge violates invariant
#7 from the opposite side, and no coverage note can excuse it.

**Declared:** the `fs.write` entry of this backend's coverage manifest
says exactly this, so the gap is visible to the certificate.

**To close it:** correlate the `w|<tmp>` and `m|<dst>|<tmp>` lines and
emit the synthetic write at `<dst>`. When you do, this fixture's golden
gains an event and this test fails — that is the signal to move this
directory to `conformant/`.
