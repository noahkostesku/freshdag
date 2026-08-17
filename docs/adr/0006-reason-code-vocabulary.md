# ADR 0006: A closed reason-code vocabulary for certificates

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** architect
- **Amended:** 2026-08-17 — see §Amendment. **The ten-row table in
  §Decision is a historical record of the vocabulary on 2026-08-15 and
  is no longer a normative statement of membership.** The vocabulary now
  has fourteen members; the normative list is `ReasonCode` in
  `freshdag-core::dependency::validity`, mirrored in
  `docs/contracts/certificate-contract.md §Reason Codes`. The closedness
  argument below is unchanged and still governs. See ADR 0015.
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

> **Historical as of 2026-08-17 — do not cite this table for
> membership.** It records the vocabulary as decided on 2026-08-15. Four
> members have since been added (ADR 0009, ADR 0014, and commit
> `560151e`); the current list is `ReasonCode` in
> `freshdag-core::dependency::validity`, mirrored in
> `docs/contracts/certificate-contract.md §Reason Codes`. The table is
> left as written because an ADR records what was decided when, and the
> growth from ten is itself part of the record. See §Amendment and ADR
> 0015.

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
value a test can assert on, not a sentence a human reads.

**That §2 sentence has since been falsified as written.** The Wave 2
novelty review found **EA-Graph** (arXiv:2608.04278), which encodes a
machine-checked never-promote rule ("no model output enters at
`PROVEN`") over an agent-artifact graph, with an anchor-completeness
check that is a coverage-deficit analog. See `docs/NOVELTY.md` §5.7,
an open escalation to `architect`. The §2 *conjunction* still has no
match — EA-Graph re-reads local repository state by content hash and
has no cross-session probing of external mutable state, no
scheme-registered probes, no TTL/`volatile` class, and no portable
certificate — but this ADR must not be read as evidence that the
"machine-checked" clause is unprecedented on its own.

Likewise `ProducerRole`: it makes the coverage-deficit rule checkable
from the certificate alone, but the *genus* — provenance that declares
its own blind spots so a verifier can refuse to over-trust it — is
occupied by **SLSA v0.2 `metadata.completeness`**, where materials are
incomplete by default unless the builder asserts otherwise. Our
differentia is narrow and should be stated narrowly: the discharge
condition is derived from a role-typed producer registry rather than
self-asserted by the builder. SLSA lets the builder grade its own
homework; FreshDAG does not.

**Honest acknowledgement of drift toward adjacent prior art.** A closed
predicate vocabulary attached to a derived artifact moves the
certificate marginally closer in *shape* to in-toto / SLSA attestations
and W3C Verifiable Credentials' `credentialStatus`, both already
tracked in `docs/NOVELTY.md` §1 at High and Medium collision risk. We
acknowledge this and do not claim it as ours: "signed machine-checkable
predicates on derived artifacts" is explicitly on the §3 firewall list.
What remains ours is narrower and unchanged by this ADR — the trust
classes the codes range over, the heuristic-cap rule, and the
coverage-deficit rule grounded in producer vantage point.

The closed-vocabulary-plus-non-normative-sidecar shape this ADR ships
also has a direct precedent we had not tracked: **OpenVEX**, whose
`justification` field is a closed vocabulary and whose
`impact_statement` is free text the spec discourages consumers from
parsing — the same design, for the same reason. **RFC 5280
`CRLReason`** is the older ancestor. Both, along with EA-Graph and
SLSA, were added to `docs/NOVELTY.md` §1 by the Wave 2 novelty review,
and corresponding rows were added to the §3 firewall. An earlier
revision of this ADR asserted that no new collision was discovered;
that was wrong, and the update it said was unnecessary has been made.

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

---

## Amendment (2026-08-17): the vocabulary grew 40% in a day, and this ADR could not tell anyone whether that was allowed

Ruled by `architect` in the retrospective review of the 2026-08-17
merges. **Nothing in §Decision's reasoning is withdrawn.** What changes
is what this document may be cited for.

**What happened.** Ten codes on 2026-08-15; fourteen on 2026-08-17. ADR
0009 added two and declared `Extends: ADR 0006`. ADR 0014 added a third
and did not. The fourteenth,`unproven-dependency`, landed in commit
`560151e` with no ADR, as part of a verifier remediation that closed a
live invariant-#7 hole. All four are adjudicated in ADR 0015 and all
four are warranted on the merits.

**What this ADR got wrong.** Not the closedness argument — that holds,
and every widening honoured it: the set stayed finite, enumerable,
schema-enforced, with no `Other(String)` escape hatch, and a certificate
carrying an unknown code still fails to deserialize.

Two things:

1. **It conflated *closed* with *frozen*.** "The vocabulary is ten
   codes" reads as a membership claim with no expiry, in a document
   that stays `accepted` forever. Closedness is a property of the set at
   any instant; stability over time is a different property this ADR
   neither established nor scheduled. ADR 0015 Decision 1 separates
   them.
2. **It stated a membership rule and no admission test.** ADR 0009 said
   the process for adding a member "is exactly the one ADR 0006
   anticipated" — but this ADR anticipates only the contract-change
   process, which is a *procedure*, not a test of whether a code is
   warranted. Nothing here would have let a reviewer say no. ADR 0015
   Decision 2 supplies the four-part test, generalising ADR 0012's
   amended test for a warranted vocabulary member.

**Also corrected:** §Consequences claims a test "asserting all three
agree" keeps the Rust enum and both schemas in sync. It does not. A
verifier added a variant to the enum, omitted it from
`ALL_REASON_CODES` and both schemas, and the entire suite passed — the
count assertion is hand-maintained and the schema test compares against
the hand list rather than the enum. The only real guard is the
compiler's exhaustive `match` in `freshdag-cli`. The contract table is
guarded by nothing. ADR 0015 Decision 4 makes the guard mandatory and
specifies it.

**This ADR is no longer the place to look up the vocabulary.** ADR 0015
Decision 3 names two normative registries — the Rust enum and the
certificate-contract table — and forbids every other document from
carrying a copy.
