# ADR 0008: IR decoding is lenient; strictness lives in schema conformance

- **Status:** accepted
- **Date:** 2026-08-16
- **Deciders:** architect (owner of `docs/contracts/execution-ir.md`)
- **Consulted:** `core-engineer` (the S0 decoder and the two tests this
  ADR renames); `claude-adapter` (W1 already depends on the answer).
- **Resolves:** Wave 1 open question #3, carried through
  `docs/prompts/wave-2.md §7`.

## Context

Should `IrEvent` carry `#[serde(deny_unknown_fields)]`?

Wave 2 left the question open behind two documenting tests in
`crates/freshdag-core/src/ir/tests.rs`:
`unknown_envelope_fields_are_currently_tolerated` and
`payload_leniency_unknown_fields_tolerated`. The first says the stricter
policy "is a candidate ADR — if adopted, this test flips to `is_err()`."

Two facts constrain the answer.

**On payloads, the contract already decided.**
`docs/contracts/execution-ir.md §Event Kinds (v0)` says: "The set is
small on purpose. Adapters extend by adding payload fields, not by
inventing kinds." W1's Claude adapter relies on this today —
`recipe_hash` rides on `computation.started` — and the probe contract's
`retryable` was landed the same way, explicitly noted as "additive to an
existing kind and therefore does not bump `schemas/execution-ir/`."
Adopting `deny_unknown_fields` on payloads would break shipped code and
contradict shipped contract text.

**On the envelope, the argument is about failure direction.** This is
the part that was genuinely open.

## Decision

**No `deny_unknown_fields`, on the payload or the envelope. Permanently,
not provisionally.** Strictness belongs at the schema-conformance layer,
never in the runtime decoder.

The reasoning is the trust model, not convenience. Refusing to parse an
event does not produce a loud error at the place that matters: the store
records a malformed line, the event is absent from the replay, and the
absence is indistinguishable from the event never having happened.
"Nothing was observed" is precisely the silence invariants #5 and #7
exist to distrust. A strict decoder therefore converts a
forward-compatibility problem into **evidence loss**, and evidence loss
fails in the unsafe direction: a computation whose `fs.read` events were
all rejected looks like a computation with no dependencies, and the only
thing standing between that and a `valid` certificate is the
`no-dependencies-observed` reason code. Leniency fails in the safe
direction — an unrecognized field is ignored, the event still counts,
the producer is still attributed.

The mechanism that *does* police producers is already built and is the
right one: **coverage manifests**. A producer declares what it emits; an
event from a producer with no registered manifest surfaces as
`ReasonCode::ProducerMissingFromCoverage` and caps the status at
`unknown`. That is strictness applied to the party responsible, with an
explanation, at the point of decision — not a parse failure that
destroys the record.

Consequently:

- `IrEvent` and every payload type keep serde's default leniency.
- Strict validation of `schemas/execution-ir/v0.1.json` (including
  `additionalProperties` policy, should we ever add one) is a
  conformance-test concern under `fixtures/adapter-conformance/`. A
  conformance suite may be as strict as it likes; the runtime decoder
  may not.
- `unknown_envelope_fields_are_currently_tolerated` is renamed to drop
  "currently" and its docstring is rewritten to cite this ADR as the
  decision rather than as an open question.

## Consequences

- Adapters and observers running a newer minor version of the IR remain
  readable by an older FreshDAG. This is the property that makes the
  adapter boundary (ADR 0002, ADR 0003) survive independent release
  cadences.
- Typos in field names are silently ignored at decode time. This is the
  real cost. It is mitigated by conformance fixtures, not by the
  decoder: `fixtures/adapter-conformance/<name>/` is where a
  misspelled `recipie_hash` must be caught, and every new adapter owes
  at least one fixture per `.claude/rules/architecture.md`.
- `source_digest` is computed over re-serialized events, so an unknown
  envelope field a producer wrote does not survive into the digest. This
  is pre-existing and unchanged by this ADR, but it means a third party
  recomputing the digest from raw bytes can disagree with us. If
  portable digest verification becomes a requirement, the fix is a
  captured-unknown-fields map on the envelope, not strict rejection.
  Recorded, not scheduled.

## Rejected alternatives

- **Strict envelope, lenient payload.** Superficially attractive — the
  envelope is small and fully specified. Rejected for the same failure
  direction: a rejected envelope loses the whole event, including the
  payload we would have accepted.
- **Strict decoding behind a `--strict` flag.** Rejected for v0: two
  decode paths means two sets of behaviour to reason about at exactly
  the boundary where FreshDAG's claims are made. If it returns, it
  returns as a `freshdag validate` subcommand that reports without
  affecting any verdict.
