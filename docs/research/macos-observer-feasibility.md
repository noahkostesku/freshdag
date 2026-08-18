# macOS observer feasibility

- **Date:** 2026-08-18
- **Author:** spike run in the owner's session, on the owner's direction
- **Status:** research memo. Decides nothing; recommends one thing.
- **Question:** `.claude/rules/architecture.md` says *"unless you have new
  information invalidating the observer memo, do NOT add native macOS
  observation."* Is there new information?

## Answer

**No. The memo stands, and the evidence for it is now reproducible
rather than asserted.**

One thing *is* new, and it is not about macOS: a delivery mechanism for
subprocess observation exists that `docs/contracts/observer-contract.md
§Platform Matrix` never contemplated. It is buildable on Linux today.
See §The new information.

## Why this was asked now

`docs/DOGFOOD.md` session 1 measured the Claude Code adapter observing
**8–14%** of a real session, with the variance explained by working
style. The residual is `bash`, where the adapter is structurally blind.
At that ratio an fs-covering observer stops being an optimization and
starts being the difference between a tool that decides and one that
abstains — so "can we even build one here?" gates the next wave.

## What was tested, on this machine

macOS 15.0.1 (24A348), Apple Silicon, SIP **enabled**.

### 1. `DYLD_INSERT_LIBRARIES` — the fsatrace strategy. Dead.

The Linux observer works by `LD_PRELOAD`ing an interposer. The macOS
equivalent is `DYLD_INSERT_LIBRARIES`, and SIP strips `DYLD_*` from the
environment of protected binaries. Everything in `/bin` and `/usr/bin`
is protected.

```
$ DYLD_INSERT_LIBRARIES=/tmp/fake.dylib /bin/sh   -c 'echo ${DYLD_INSERT_LIBRARIES:-<STRIPPED>}'
<STRIPPED>
$ DYLD_INSERT_LIBRARIES=/tmp/fake.dylib /bin/bash -c 'echo ${DYLD_INSERT_LIBRARIES:-<STRIPPED>}'
<STRIPPED>
```

This is decisive, not incidental. The harness shell is `/bin/zsh`, also
root-owned in `/bin`. So even a perfectly good interposer would never be
inherited by the children of a `Bash` tool call — which is the entire
population we need to observe. Disabling SIP is not an option we may ask
a user for.

### 2. Endpoint Security — the only sanctioned path, and it is gated.

ES is Apple's supported answer and would genuinely work. It requires the
restricted `com.apple.developer.endpoint-security.client` entitlement:
an approval request to Apple, a paid Developer account, a provisioning
profile, notarization, and the client must run as root.

That is fatal for this project's distribution shape specifically. A user
cannot `cargo install` an ES client and run it — the binary must be
signed with an entitlement only Apple grants to a named team. An
open-source observer that only works when built by us, signed by us, and
notarized by us is a different product.

### 3. BSM audit (OpenBSM) — deprecated and disarmed.

Worth testing because it predates ES and carries process attribution.
It does not survive contact:

```
/dev/auditpipe             exists, crw------- root:wheel   (root only)
/etc/security/audit_control   absent
auditd                     present but not running
```

Apple deprecated OpenBSM in favour of ES. Building on a subsystem that
ships unconfigured, requires root, and has an announced removal is not a
foundation.

### 4. DTrace / `opensnoop` — present, SIP-blocked.

Both binaries exist. SIP prevents tracing protected binaries, which is
again exactly the shell processes we care about.

### 5. FSEvents — wrong shape.

Directory-granularity notifications with no read events and no process
attribution. It cannot answer "which computation read this file", which
is the only question the graph asks.

### 6. Linux VM tunnel — viable, absent, and it relocates the problem.

`docs/contracts/observer-contract.md` already names this as the macOS
ceiling. Nothing is installed here: no `docker`, `lima`, `colima`,
`orbstack`, `podman`, `vagrant`, or `qemu`.

The deeper point is that "a VM exists" is not the requirement. Claude
Code runs on the host and its `Bash` tool spawns host processes. To
observe them, **the agent's work itself has to happen inside Linux** — a
devcontainer-shaped development environment, not a VM sitting beside
one. That is a change to how the user works, and it should be costed as
such rather than as an install step.

## The new information

`PreToolUse` hooks may return `hookSpecificOutput.updatedInput`, which
**rewrites the tool input before the tool runs**. For a `Bash` call that
means the adapter can rewrite

```
<command>
```

into

```
fsatrace rwmdt <trace-file> -- sh -c '<command>'
```

and then compile the trace into `fs.*` events attributed to the
computation — closing the `bash` blindness at its source rather than
observing around it.

The platform matrix never considered this. It evaluated `LD_PRELOAD` and
`strace` as *ambient* mechanisms applied to a process tree, and asked
which platform permits them. Command rewriting is a different axis: the
adapter already sits in the path of every tool call and is already
permitted to modify it.

**This does not rescue macOS.** It is a delivery mechanism, and §1–§5
say there is nothing to deliver. What it changes is Linux, where
`fsatrace` already works and `freshdag-observer` already supports it:
there, subprocess observation becomes a hook change rather than a new
platform backend.

Costs to weigh before anyone builds it, none of them small:

- It **modifies the user's command**. A wrapper that mangles quoting,
  loses a non-zero exit code, breaks a heredoc, or interferes with a TTY
  is worse than blindness. The adapter contract's never-block rule
  applies with much more force here than it does to a hook that only
  appends to a log.
- `fsatrace` must be present, and its absence must degrade to today's
  behaviour silently and safely.
- It changes `fs.*` events from *pre-execution intent* to *confirmed
  effect* for the wrapped subset, which is a coverage-manifest change
  and touches the `partial` declarations.
- Trace volume for a `cargo build` is large; the byte cap and drop
  semantics matter.

## Recommendation

**Target Linux as the first observed surface. Do not build a macOS
observer.**

Concretely, and in order:

1. **Do nothing here yet.** This memo answers a gating question; it does
   not authorize work. `BUILD_PLAN §6.3`'s moratorium applies — the
   proposer names the session that demanded it.
2. If and when subprocess observation is built, build it **on Linux via
   `PreToolUse` command rewriting**, where the tracer exists and the
   observer crate already supports it. That is where a `valid`
   certificate could first exist at all.
3. **Record the macOS gap honestly and keep reporting it.** The platform
   matrix already says "no native syscall observation"; `freshdag check`
   already caps at `unknown` and says why. That behaviour is correct and
   should not be softened to make a demo look better.

## What would change this answer

Only one of:

- Apple grants ES entitlements to open-source projects on terms a
  `cargo install` user can satisfy, **or**
- a non-root, non-entitled, SIP-compatible file-access API appears on
  macOS, **or**
- the project accepts a signed, notarized, first-party-distributed
  binary as the macOS story, which is a distribution decision rather
  than an engineering one.

Re-run §1's two-line test before trusting this memo on a later macOS.

## Sources

- Apple Developer Forums — Endpoint Security entitlement for internal
  distribution: <https://developer.apple.com/forums/thread/759149>
- Apriorit — Collecting telemetry data on macOS using Endpoint Security:
  <https://www.apriorit.com/dev-blog/collecting-telemetry-data-on-macos-using-endpoint-security>
- macOS SIP environment sanitization:
  <https://briandfoy.github.io/macos-s-system-integrity-protection-sanitizes-your-environment/>
- Surprising consequences of macOS's environment variable sanitization:
  <https://hynek.me/articles/macos-dyld-env/>
- HackTricks — macOS library injection:
  <https://hacktricks.wiki/en/macos-hardening/macos-security-and-privilege-escalation/macos-proces-abuse/macos-library-injection/index.html>
