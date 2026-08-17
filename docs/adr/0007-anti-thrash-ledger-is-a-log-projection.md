# ADR 0007: The anti-thrash trust ledger is a projection of the log, not persistent state

- **Status:** accepted
- **Date:** 2026-08-16
- **Deciders:** architect
- **Consulted:** Wave 2 completion review (`graph-engineer` W4,
  `integration-engineer` W7, `probe-engineer` W5.2 by their merged code
  and comments); `store-engineer` to be consulted on the `derived/`
  layout addition before implementation.
- **Supersedes:** nothing. Resolves Wave 1 open question #1, carried
  through `docs/prompts/wave-2.md §7`.

## Context

`docs/contracts/probe-contract.md §Anti-thrash Protocol` requires N=2
consecutive higher-trust observations before a dependency's trust class
escalates, keyed per `(dependency_key, probe_identity)`. Wave 2 shipped
`crates/freshdag-engine/src/antithrash.rs`, which holds that state in a
`BTreeMap` on the `Engine`, in memory, for the lifetime of one process.

Three facts make that arrangement worse than "incomplete":

1. **`freshdag check` is a short-lived process.** The ledger is
   constructed empty and dropped at exit. N=2 can therefore never be
   reached across two invocations, which is the only way it would ever
   be reached in the CLI's actual usage pattern. The protocol is inert.

2. **The evidence that would rebuild it is discarded.**
   `Engine::check` returns the `probe.checked` and `diagnostic` events
   it generated and its own documentation says "the caller appends these
   to the log." `crates/freshdag-cli/src/check.rs` does not, and
   correctly explains why: the engine synthesizes its own
   `observation_coverage` entry in memory
   (`engine.rs::engine_coverage_entry`) but nothing registers a coverage
   manifest for `freshdag-engine` in the store, so replaying a log
   containing engine events yields `producer-missing-from-coverage` and
   caps every subsequent check at `unknown`. The CLI's choice is right;
   the underlying gap is that **the engine is a producer that cannot be
   registered.**

3. **Even with those events in the log, the ledger is not
   reconstructable.** It is keyed on `(dependency_key, probe_identity)`,
   and `probe_identity` never reaches the `probe.checked` payload. This
   is a live strain on invariant #5: derived state that no replay can
   rebuild.

The framing question ("persistent or in-memory?") is a false choice. It
invites a third storage class — mutable derived state with its own
durability story — next to an append-only log and a disposable
projection. FreshDAG does not need one and should not acquire one.

## Decision

**The trust ledger is a projection of the canonical log, in exactly the
sense `DerivedGraph` is.** It is neither persistent mutable state nor
per-process scratch. Concretely, Wave 3 lands four changes:

1. **The engine publishes a coverage manifest.** `freshdag-engine`
   exposes its `CoverageManifest` (role `probe`, emitting
   `probe.checked` and `diagnostic`) as public API, and the CLI
   registers it with the store's `CoverageRegistry` before appending
   engine events. `Engine::check` stops synthesizing a `CoverageEntry`
   in memory and reads its own entry from the registry like every other
   producer. This removes a self-attestation from the certificate: today
   one row of `observation_coverage` is written by the same party that
   evaluates the coverage deficit against it.

2. **`freshdag check` appends the engine's events.** The `--record`
   behaviour dropped in `7e6b8bd` returns, unblocked by (1). Whether it
   is the default or opt-in is `integration-engineer`'s call; a
   read-only checkout must still be able to run a pure query.

3. **`probe.checked` gains an additive `probe_identity` payload field.**
   The IR contract states that "adapters extend by adding payload
   fields, not by inventing kinds", and `retryable` set the precedent
   for an additive field on this kind without a schema bump. Without it,
   the anti-thrash key is unrecoverable from the log.

4. **`TrustLedger::replay(&[IrEvent]) -> TrustLedger`.** The ledger is
   folded from the log's `probe.checked` sequence in canonical order.
   The in-memory ledger a single `check` mutates becomes the tail of
   that fold, discarded at exit like any other projection. If the ledger
   is materialized on disk it lives under `derived/`, is covered by
   `DerivedManifest::source_digest`, and is deletable at any moment.

## Consequences

- The N=2 protocol becomes live. Two `freshdag check` runs against the
  same store now accumulate, which is what the contract always meant.
- Anti-thrash state inherits invariant #5 for free: it is auditable,
  diffable, and rebuildable, and a disagreement between the engine's
  behaviour and the ledger is a replay diff rather than a debugging
  session.
- `freshdag check` acquires a write. This is a real semantic change: a
  check becomes an observation. That is correct — the engine *is* an
  evidence producer, and pretending otherwise is what left its coverage
  entry unregistered in the first place — but it means `check` on a
  read-only store must degrade to a pure query with a diagnostic, not
  fail.
- `freshdag watch` becomes buildable. A daemon whose observations are
  discarded has no state; this ADR is a precondition for
  `ARCHITECTURE.md §11`'s watch command.
- The store gains an optional derived artifact. `store-engineer` owns
  the layout and must sign off before implementation.

## Rejected alternatives

- **A separate mutable sidecar file (`ledger.json`).** Rejected: it is a
  third storage class, it is not covered by the log digest, and a stale
  or hand-edited sidecar could silently raise a dependency's trust class
  with no trail. Every property we want is already provided by replay.
- **Keep it in memory and document the limitation.** Rejected: the
  protocol is a contractual requirement, not a nicety. A contract clause
  that provably cannot fire in the product's primary usage pattern is a
  false statement in `docs/contracts/probe-contract.md`, and the
  cheapest honest alternative would be to delete the clause.
- **Let the ledger's adopted class raise a certificate's status.**
  Rejected, and this ADR does not change it. `antithrash.rs` is
  deliberate in using the *recorded* class on the edge. Escalation
  changes what future observations mean, never what the current
  certificate claims.

## Follow-up recorded, not decided here

`TrustLedger::note_unretryable_failure` returns
`Demoted { to: Volatile }` without writing the demoted class into the
entry, so the "forces re-observation" half of the contract clause is not
implemented. ADR 0010 removes that trigger entirely, which makes the
question moot; if ADR 0010 is rejected, this becomes a bug to fix.

---

## Amendment, 2026-08-16 — two hard preconditions on the record loop (verifier D1, D4)

Raised by the `verifier` at Wave 2 rejection. Both are blocking: the
record loop above MUST NOT land until each is closed, because each is
latent today only because `--record` was dropped in `7e6b8bd`.

### P1. `probe.checked.trust_class` records the *recorded* class, never the adopted one

`engine.rs:384-390` writes the ledger's **adopted** class into the
emitted `probe.checked`, with a comment asserting that this "cannot
escalate a dependency's trust class behind the anti-thrash protocol's
back." It does the opposite. If the in-memory ledger has escalated a
dependency `heuristic → versioned`, the event written to the log says
`trust_class: versioned`, and a replay of that log yields `Valid` where
the store recorded `heuristic` — silent promotion across a process
boundary, invariants #7 and #8.

It is also, and independently, an invariant #5 violation. This ADR makes
the ledger a **fold over `probe.checked`**. Writing the fold's own
output back into the events being folded makes the projection
non-idempotent: replaying a log whose events already encode the fold's
result re-folds on top of it, so the derived state depends on how many
times it has been derived. A projection with that property is not
reconstructable in the sense invariant #5 means.

The correction is small and makes the payload carry only inputs:

- `trust_class` — the class **recorded on the dependency** (`dep.trust_class`,
  what the store observed at production time).
- `observed_trust_class` — what the probe saw this time. Already present.

Adoption is then reconstructed by `TrustLedger::replay` from the
sequence of `observed_trust_class` values, which is what the anti-thrash
protocol was always defined over. Nothing derived is ever written to the
log.

### P2. ADR 0011 lands first

Nothing in production registers a coverage manifest, so real adapter
output currently caps at `unknown` and never reaches `valid`. That masks
the partial-coverage hole in ADR 0011: an observer declaring itself
blind discharges a `bash`/`task` obligation. Closing this record loop
removes the mask and turns a masked hole into a live one. Sequencing is
recorded in `docs/BUILD_PLAN.md §6.2`.

### Related, non-blocking: the coverage gate's guard is a lint, not an invariant

The `GateOutcome` token is sound but constrains only `seal.rs`. The
source-reading guard covering the other seven engine files is defeated
by path-qualified struct literals (`certificate::Certificate { … }`),
`serde` deserialization, and clone-then-mutate. The `verifier`'s fix
direction — a type, not a stricter regex — is correct and is the
architectural rule: **an invariant enforced by reading source text is a
lint; an invariant enforced by the type system is an invariant.** The
shape that follows is a status field constructible only through the
gate (a newtype whose sole public constructor consumes a `GateOutcome`,
with a validating `Deserialize`), so struct literals cannot set it and
deserialization cannot bypass it. That is a `freshdag-core` change and
therefore needs `architect` sign-off when proposed; `core-engineer` and
`graph-engineer` own the design. Not a precondition for the record
loop, but it should not be deferred past Wave 3.
