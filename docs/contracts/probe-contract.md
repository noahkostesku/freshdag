# Contract: Freshness Probe

**Status:** provisional (v0.1).

**Owner:** `probe-engineer` (see `.claude/agents/probe-engineer.md`).

**Governs:** every implementation in `freshdag-probes/`.

**Invariants relied on:** #7, #8.

---

## Purpose

A probe answers "is this dependency still at the recorded fingerprint?"
cheaply — cheaply enough that we would rather run all the probes for an
artifact's dependencies than rerun the agent that produced it.

## Probe Interface

Each probe registers against one or more dependency schemes
(`file://`, `https://`, `attio://`, `mcp://`, `postgres://`, ...) and
implements:

```
fn check(
    scheme:      &str,
    key:         &DependencyKey,        // scheme-specific opaque
    recorded_fp: &Fingerprint,          // trust class + bytes
    ttl_hint:    Option<Duration>,
) -> ProbeResult

enum ProbeResult {
    Match       { observed_fp: Fingerprint },
    Drift       { observed_fp: Fingerprint },
    Unknown     { reason: String, retryable: bool },
}
```

## Trust-class Semantics

The probe MUST respect the recorded trust class:

- **`exact`** — verify by fetching enough content to reproduce the
  content hash. If the endpoint does not support cheap content hashing,
  the probe returns `Unknown { retryable: true }`, not a bare `Drift`.
- **`versioned`** — verify by comparing the version token. Probes MAY
  use `If-None-Match`, `If-Modified-Since`, or the source's native
  version query.
- **`heuristic`** — verify by any cheap signal; the probe's `Match`
  result never promotes the trust class.
- **`volatile`** — return `Unknown` if the TTL has expired; `Match`
  inside TTL.

A probe MAY escalate trust (e.g., discover that a `heuristic`-recorded
dependency now has a `versioned` endpoint) by emitting a
`probe.checked` event whose `observed_fingerprint.trust_class` is
higher than the recorded one. The engine uses this to migrate the
dependency's trust class over time. A probe MUST NOT silently demote
trust.

### Anti-thrash Protocol

Trust-class transitions are per-`(dependency_key, probe_identity)` and
governed by these rules:

- **Escalation requires stability.** A single higher-trust observation
  is not sufficient; the engine records the proposed escalation and
  requires N=2 consecutive higher-trust observations (or one
  observation older than the previous trust class's recorded TTL,
  whichever is stricter) before adopting the new class.
- **Demotion is explicit, never silent.** If a probe returns
  `Unknown { retryable: false }` or subsequent observations at a
  strictly lower trust class, the engine emits a
  `probe.trust_demoted` diagnostic and forces re-observation of every
  artifact edge depending on this dependency before its trust class
  changes on any certificate.
- **Probe removal.** If a probe registered for a scheme is uninstalled
  or fails registration, the engine treats dependencies previously
  observed by that probe as `Unknown` on their next check. It does
  NOT silently fall through to a lower-trust probe for the same
  scheme.

The purpose is to prevent flap between two probes handling the same
scheme (e.g., a generic HTTPS probe and a GitHub-specific probe)
producing thrashing trust classes on the same dependency.

### Probe Arbitration for a Scheme

Probes register with `(scheme, host_pattern, priority)`. The
highest-priority match wins. Ties are contract violations that fail
loudly (with a `diagnostic` event) rather than silently pick either
probe.

## Failure Modes

- **Network / permission failure** → `Unknown { retryable: true }`.
- **Endpoint returns malformed data** → `Unknown { retryable: false }`.
- **Probe implementation bug detected via invariant check** →
  `Unknown { retryable: false }`, with a diagnostic event.

Silent "the endpoint didn't respond, so I'll say fresh" behavior is a
correctness bug that violates invariant #7. Enforce this in code
review.

## Registration

Probes register by scheme; when multiple probes handle the same scheme,
the engine picks the highest-declared trust-class-capability probe
first, falling back on lower classes on `Unknown { retryable: false }`.

## Rate Limiting / Cost

Probes SHOULD:

- Batch requests where the backing service supports it.
- Cache probe results for the TTL they declare (e.g., an HTTP probe
  may cache `ETag` responses for `Cache-Control: max-age`).
- Emit a `probe.cost` metric for observability.

Probes MUST NOT:

- Rewrite external state as part of a `check`. Probes are read-only.

## Testing

A probe is considered contract-conformant when:

- Its output on `fixtures/probe-conformance/` (scripted fake endpoints)
  matches the golden results.
- It correctly reports `Unknown` on simulated failures without
  fabricating a `Match`.
- Trust-class semantics are honored under adversarial fixtures (e.g.,
  an endpoint that flip-flops its ETag with no content change).
