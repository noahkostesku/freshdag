# ADR 0010: Trust demotion requires an observed trust class; `retryable` is a scheduling hint only

- **Status:** accepted
- **Date:** 2026-08-16
- **Deciders:** architect
- **Consulted:** `probe-engineer` (owns
  `docs/contracts/probe-contract.md`; must author the contract edit),
  `graph-engineer` (owns the engine branch), `core-engineer` (owns
  `ReasonCode` semantics).
- **Requires:** a `contract-change`-labelled PR against
  `docs/contracts/probe-contract.md §Anti-thrash Protocol` and
  §Failure Modes. No schema change; no new reason code.

## Context

`docs/contracts/probe-contract.md §Anti-thrash Protocol` says:

> If a probe returns `Unknown { retryable: false }` **or** subsequent
> observations at a strictly lower trust class, the engine emits a
> `probe.trust_demoted` diagnostic and forces re-observation …

Wave 2's engine implements this faithfully: an unretryable `Unknown`
routes to `TrustLedger::note_unretryable_failure` and the edge reports
`ReasonCode::ProbeTrustDemoted`.

The contract is wrong, and the file probe demonstrates it in the most
common case there is. `FileProbe::check` returns
`Unknown { retryable: false }` when the file is missing (`NotFound` is
not `Interrupted` or `TimedOut`), when the recorded fingerprint is
malformed, and when the fingerprint kind is one it cannot verify. A user
who deletes a dependency and runs `freshdag check` is told the reason is
`probe-trust-demoted`, with the detail
`could not read /x: No such file or directory`.

That is a true detail attached to a false code. The trust class of
nothing was demoted; a file is gone. Invariant #6 requires every
decision be explainable, and ADR 0006's whole argument is that the code
— not the prose — is the explanation. A code that names the wrong cause
is worse than a generic one, because downstream consumers key off it:
`freshdag watch` would see a demotion event and force re-observation of
every artifact edge depending on a file that simply does not exist.

The root cause is an overload. `retryable` answers a **scheduling**
question — should the daemon try again? Demotion answers an
**evidentiary** question — did the source's validator get weaker? These
are independent:

| | retryable | carries trust information |
| --- | --- | --- |
| File deleted | no | none |
| Malformed recorded fingerprint | no | none (it is *our* record that is bad) |
| Endpoint stopped serving `ETag` | yes | a lot |
| Network timeout | yes | none |

The one row where demotion is the right conclusion is already covered by
a different mechanism: the HTTPS probe returns `Match`/`Drift` with an
`observed_trust_class`, which `TrustLedger::observe` folds and demotes
on correctly. `note_unretryable_failure` adds nothing the observed-class
path does not already do — and it does it wrongly, since it returns
`Demoted { to: Volatile }` without writing that class into the entry, so
the "forces re-observation" half never happens anyway.

## Decision

**Demotion is triggered by exactly one thing: an observation at a
strictly lower trust class.** `ProbeResult::Unknown` carries no
trust-class information by construction and MUST NOT trigger demotion,
at any value of `retryable`.

Specifically:

1. `docs/contracts/probe-contract.md §Anti-thrash Protocol` drops the
   `Unknown { retryable: false }` clause from the demotion trigger. The
   §Certificate consequences paragraph is amended: `ProbeTrustDemoted`
   is emitted only when a probe observed a strictly lower class.
2. `ProbeResult::Unknown { .. }` maps to `ReasonCode::ProbeUnknown`
   regardless of `retryable`. This is already the code's documented
   meaning — "a probe **ran** and could not decide" — and is true of
   every case listed above.
3. `retryable` keeps its existing home: the `probe.checked` payload, for
   the scheduler. `docs/contracts/probe-contract.md`'s existing sentence
   — "`retryable` does NOT appear on the certificate … Certificates
   explain; the log schedules" — becomes true without exception.
4. `TrustLedger::note_unretryable_failure` is deleted.

Nothing about verdicts changes: `Unknown` was and remains `Unknown`, and
the artifact status is unaffected. **Only the reported reason changes**,
which is precisely the property invariant #6 governs.

## Consequences

- `freshdag why` on a deleted file now says `probe-unknown` with the
  read error as detail. Correct and boring.
- `ProbeTrustDemoted` becomes rare and meaningful. A demotion event now
  always corresponds to a source whose validator actually got weaker,
  which is what a human triaging one wants.
- A real gap is left open on purpose: a probe that discovers it can no
  longer verify at the recorded class (an `exact`-recorded dependency
  whose endpoint stopped supporting content hashing) has no way to say
  so, because it must return `Unknown` and `Unknown` no longer demotes.
  **Deferred with a trigger:** the first probe that genuinely needs it
  gets an explicit capability signal — e.g.
  `Unknown { reason, retryable, verifiable_at: Option<TrustClass> }` —
  as its own contract change. Do not resurrect the `retryable` overload
  to serve it.
- One scenario/fixture assertion may move if any pins
  `probe-trust-demoted` on a file-probe failure. `eval-engineer` checks
  as part of the contract-change PR.

## Rejected alternatives

- **Fix `FileProbe` to return `retryable: true` for a missing file.**
  Rejected: it is a lie in the other direction (retrying a deleted file
  is pointless) and it leaves the contract's category error in place for
  the next probe to fall into.
- **Add a `ProbeUnretryable` reason code.** Rejected: `ProbeUnknown`
  already means exactly this, and ADR 0006's vocabulary is closed for a
  reason. `retryable` is scheduling metadata and belongs in the log, not
  in a code the certificate carries.
- **Keep the clause and document the false positive.** Rejected. The
  contract is the artifact this project claims makes its statements
  checkable; a knowingly-false clause in it is worse than a bug in code.
