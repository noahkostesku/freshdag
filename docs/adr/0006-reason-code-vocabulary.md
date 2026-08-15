# ADR 0006: A closed reason-code vocabulary for certificates

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** architect
- **Consulted:** core-engineer, probe-engineer, observer-engineer,
  eval-engineer (Wave 2 Phase A)

## Context

Invariant #6 requires that every skip/reuse decision be explainable to a
user. Invariant #13 requires that public contracts be testable. Until
Wave 2, `ValidityReason.reason` was a free-text `String`, which
satisfied neither in any checkable sense:

- **"Explainable" was unverifiable.** Nothing stopped two producers
  spelling the same condition differently — `"probe failed"`,
  `"probe_unknown"`, `"HTTP 500 from pricing endpoint"` — so no test
  could assert that a given world-state produced a given explanation.
  A contract you cannot test is a convention.
- **Consumers could not exhaustively handle the set.** The CLI
  renderer, the scenario harness, and any future integration had no way
  to enumerate the conditions they must handle, and no way to fail
  loudly on one they had not seen. Every consumer would have grown its
  own substring matching against prose, which is the same
  silently-wrong-answer failure mode invariant #7 exists to prevent,
  relocated into the presentation layer.
- **A prose reason cannot carry a scope.** "Which dependency is this
  about, and is it about a dependency at all?" was answerable only by
  reading the string.

Separately, the Wave 2 review of `check_coverage_deficit` found a live
invariant #7 hole. The rule discharged the observation obligation
created by a `bash`/`task` tool invocation for *any* producer declaring
`fs.*` coverage. An adapter legitimately declares `fs.read`/`fs.write`
— it synthesizes those events from tool inputs it can see — but is
blind inside subprocesses by construction. So a Claude-adapter-only run
on macOS, with no observer at all, could discharge the obligation and
report `valid` on a computation whose subprocess reads nobody watched.
The certificate had no field capable of expressing the distinction:
`partial` is about *fidelity*, not *vantage point*, and the fsatrace
observer carries legitimate partial notes, so a partial-based rule
would mean nothing could ever discharge the obligation.

## Decision

**`ValidityReason.reason` is a closed `ReasonCode` enum** with a
kebab-case wire form, mirrored in `schemas/certificate/v0.1.json` and
`schemas/scenario/v0.1.json`, with a test asserting all three agree. A
certificate carrying a code outside the set fails to deserialize; a
consumer that encounters one MUST treat the certificate as unreadable
rather than guess.

The vocabulary is ten codes, each either edge-scoped (explains one
`depends_on[]` entry) or artifact-scoped (explains the certificate as a
whole, with `dependency_key: ""`):

| Code | Scope |
| --- | --- |
| `drift` | edge |
| `probe-unknown` | edge |
| `no-probe-available` | edge |
| `trust-class-heuristic-caps-at-likely-valid` | edge |
| `trust-class-volatile-caps-at-likely-valid` | edge |
| `ttl-expired` | edge |
| `probe-trust-demoted` | edge |
| `coverage-deficit` | artifact |
| `producer-missing-from-coverage` | artifact |
| `no-dependencies-observed` | artifact |

`probe-unknown` and `no-probe-available` are deliberately distinct:
the first asserts a probe ran and could not decide, the second that
none ran. Collapsing them makes `freshdag why` state something false.

**An optional `detail: Option<String>` carries human context** — the
probe's failure text, an HTTP status — so the code vocabulary does not
have to grow to make `freshdag why` specific. `detail` is
**non-normative**: `status.value` MUST be a function of reason codes,
trust classes, and probe verdicts alone, and no consumer may branch on
`detail`. It is inside the `cert_id` preimage, so it MUST be
deterministic (no elapsed times, PIDs, ports, retry counters) and MUST
NOT carry secrets. The full rules are in
`docs/contracts/certificate-contract.md §The detail field`.

**`ProducerRole { Adapter, Observer, Probe }` is a required field** on
`CoverageManifest` and `CoverageEntry`. The coverage-deficit rule now
discharges the `bash`/`task` obligation only for a producer with
`role: "observer"` declaring `fs.*`. The field is required, with no
serde default, because a defaulted role is a silent-wrong-answer
generator sitting directly on the invariant-#7 path.

Reason ordering is contractual: edge-scoped reasons in `depends_on[]`
order, artifact-scoped reasons after them. `cert_id` hashes the
ordering and fixtures pin `reasons[0]`.

## Consequences

- **The certificate schema narrowed inside provisional `v0.1`.** Per
  the new Change Policy in the certificate contract, a non-additive
  change may land inside `v0.1` while the contract is provisional,
  given an ADR, same-PR migration of every fixture and schema, and a
  changelog entry. This ADR is that ADR. Once the contract goes
  stable, `v0.1` freezes and the same change would require `v0.2`.
- **Certificates emitted by Wave 1 code no longer validate.** Wire form
  moved snake_case → kebab-case and `observation_coverage[].role`
  became required. No certificate exists outside `fixtures/`, and there
  is no external consumer, so the migration cost is bounded to this
  repository.
- **`ProducerRole` is a breaking `freshdag-core` API change.** Every
  construction site of `CoverageManifest` and `CoverageEntry` must now
  name a role. That is the intended outcome: it forces each producer to
  state its vantage point rather than inherit a default.
- **Wave 4 owes emitters.** Four codes are currently expressible but
  unemitted: `TtlExpired`, `CoverageDeficit`, `NoProbeAvailable`, and
  `ProbeTrustDemoted`. The enum is deliberately ahead of the engine so
  the vocabulary is settled before consumers key off it, but until W4
  lands the emitters, those codes are contract surface with no
  producer. A residual gap is recorded alongside this: four v0
  scenarios assert `after_mutation` without pinning a reason code, and
  pinning them requires engine behaviour nobody has specified yet.
- **The engine has one more thing it can get wrong loudly** instead of
  one more thing it can get wrong quietly. Choosing the wrong code is a
  test failure; choosing the wrong prose was not.

## Novelty

Per `.claude/rules/novelty.md`, ADR motivations receive novelty review.

This change **strengthens** the surviving wedge in `docs/NOVELTY.md`
§2 — "trust-class-typed validity certificates" as the shareable
primitive. §2 argues the wedge holds because no trace store or lineage
graph "encodes the 'heuristic never promotes to valid' rule as a
machine-checked property on their manifest." A closed, schema-enforced
reason vocabulary is what makes *machine-checked* literal rather than
aspirational: `trust-class-heuristic-caps-at-likely-valid` is now a
value a test can assert on, not a sentence a human reads. Likewise,
`ProducerRole` makes the coverage-deficit rule — which §1 notes has no
analog in the OPA-over-OpenLineage framing — checkable from the
certificate alone.

**Honest acknowledgement of drift toward adjacent prior art.** A closed
predicate vocabulary attached to a derived artifact moves the
certificate marginally closer in *shape* to in-toto / SLSA attestations
and W3C Verifiable Credentials' `credentialStatus`, both already
tracked in `docs/NOVELTY.md` §1 at High and Medium collision risk. We
acknowledge this and do not claim it as ours: "signed machine-checkable
predicates on derived artifacts" is explicitly on the §3 firewall list.
What remains ours is narrower and unchanged by this ADR — the trust
classes the codes range over, the heuristic-cap rule, and the
coverage-deficit rule grounded in producer vantage point. No new
collision was discovered, so §1 needs no update.

## Rejected Alternatives

- **Keep free text, add a lint.** A lint over prose cannot be
  exhaustive and cannot be enforced on a certificate arriving from
  another machine. Certificates are portable; the check must live in
  the schema.
- **Open enum with an `Other(String)` escape hatch.** Every consumer
  would route unknown conditions through `Other`, and the vocabulary
  would stop being a vocabulary. The contract-change process is the
  intended friction for adding a code.
- **Encode scope as a separate field instead of a property of the
  code.** Scope is not independently variable — `coverage-deficit` is
  never edge-scoped. A separate field would admit unrepresentable
  states.
- **Express the observer/adapter distinction through `partial`.**
  Rejected in the type's own documentation: `partial` is about
  fidelity, and the fsatrace observer carries legitimate partial notes,
  so nothing would ever discharge the obligation. Vantage point is a
  role.
- **Default `ProducerRole` to `Adapter` to avoid a breaking change.**
  Rejected. The defaulted value would be the one that reintroduces the
  invariant-#7 hole this ADR closes.
