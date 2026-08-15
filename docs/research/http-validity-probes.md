# HTTP validity probes — semantics recommendation for W5.2

**Status:** research memo (not implementation guidance yet).
**Owner:** probe-engineer.
**Governs:** future implementation of the `https://` probe scheme.
**Related contract:** `docs/contracts/probe-contract.md`.

## Summary

HTTP endpoints in the wild support the full range of FreshDAG trust
classes. Servers that expose strong `ETag` or a native version token
warrant `versioned`. Weak `ETag` warrants `heuristic` — not
`versioned` — because weak equivalence is defined at the *entity*
level, not the octet level, and FreshDAG's downstream comparators may
depend on octet-identity. Servers with only `Last-Modified` warrant
`heuristic` at best. Servers with no freshness metadata are
`volatile` or require content-hash fallback (opt-in) to earn `exact`.

The probe MUST never promote its trust class beyond what the server
actually proved. Anti-thrash rules from probe-contract.md §Anti-thrash
Protocol apply: a probe that has been returning `versioned` and now
sees no ETag emits `probe.trust_demoted` and forces re-observation.

## Decision matrix

| HTTP situation | Recommended trust class | Rationale | Notes |
| --- | --- | --- | --- |
| Strong `ETag` returned | `versioned` | Server asserts octet-identity when ETag matches. | Use `If-None-Match` on subsequent checks. |
| Weak `ETag` (`W/"..."`) | `heuristic` | RFC 7232 weak equivalence is entity-level, not octet-level. Two responses with matching weak ETag may differ in bytes. | Do not treat as versioned. Document loudly. |
| Only `Last-Modified` | `heuristic` | 1s resolution; clock skew; server-side caches lie. | Use `If-Modified-Since` as a cheap check but never promote. |
| Neither `ETag` nor `Last-Modified` | `volatile` (with TTL) or `exact` (via content-hash fallback) | No freshness signal from server. | Fallback is opt-in per dependency because it flips cost from O(headers) to O(body). |
| `Cache-Control: no-store` or `must-revalidate` with max-age=0 | `volatile` | Server explicitly forbids caching assumptions. | Respect it; do not attempt to cache the probe result. |
| Endpoint that previously returned `ETag` now returns none | Force `Unknown`, emit `probe.trust_demoted` | Anti-thrash protocol. Do not silently fall back to `heuristic`. | The engine forces re-observation before the next certificate promotion. |
| `304 Not Modified` via `If-None-Match` | `ProbeResult::Match` | Server explicitly asserts unchanged. | Preserve the recorded ETag on the observed fingerprint. |
| `200 OK` with unchanged body but changed ETag (server churn) | `ProbeResult::Drift` but log an ETag-instability diagnostic | The version token disagrees with content — trust the change signal, flag the endpoint. | Repeated churn should demote the endpoint to `heuristic`. |
| Redirect (`3xx`) | See §Redirect handling below | The probe records the redirect chain but the dependency key remains the original URL. | Limit to 5 hops; refuse infinite redirects. |
| Content-hash fallback (full GET + hash) | `exact` | Only trust class that justifies re-fetching the body. | Opt-in per dependency; cost-affecting. |
| Endpoint requires auth | `Unknown { retryable: false }` if credentials absent | Probes are read-only, unauthenticated by default. | Auth-configured probes are a separate variant of the HTTP probe. |
| Rate-limited endpoint | `Unknown { retryable: true }` on 429 | Never fabricate a `Match`. | Emit a `probe.cost` diagnostic; back off with server-provided `Retry-After`. |

## Weak ETags

RFC 7232 §2.3 defines two ETag equivalence relations:

- **Strong equivalence** — the two representations are byte-for-byte
  identical. Required for range requests and byte-serving.
- **Weak equivalence** — the two representations are "semantically
  equivalent." Weak ETags are marked `W/"..."`. The server is
  asserting only that a client can treat them as the same *entity*,
  not the same *bytes*.

FreshDAG cannot in general treat weak-equivalent responses as
identical, because downstream comparators may be `exact` on the
artifact bytes. Compressed variants, whitespace-normalized HTML,
transcoded images — all of these are weakly equivalent per RFC 7232
but not byte-identical.

Recommendation: **weak ETag maps to `heuristic`**. The probe records
the weak ETag as the fingerprint; a `Match` on a weak ETag is a
`heuristic` match, which per invariant #7 aggregates to `likely-valid`
on the certificate — never `valid`.

If a downstream user needs `versioned` treatment for a specific
endpoint that only exposes weak ETag, they can opt in to
content-hash fallback, which upgrades to `exact` at the cost of
downloading the body.

## Redirect handling

- Follow up to 5 redirect hops. Above that, return
  `Unknown { retryable: false }` with reason "redirect chain too
  deep."
- Record the full chain in the `probe.checked` diagnostic payload for
  auditability.
- The `depends_on[].key` on the certificate remains the ORIGINAL URL.
  The redirect target is metadata, not the dependency identity.
- The trust class is derived from the FINAL response's freshness
  headers, not the intermediate `3xx` responses.
- Cross-scheme redirects (`https://` → `http://`) MUST NOT follow
  (downgrade attack); return `Unknown { retryable: false }`.
- Cross-origin redirects follow but emit a diagnostic; the caller may
  want to know their dependency URL has been rewritten upstream.

## HEAD vs GET

- **`HEAD` is preferred for `versioned` and `heuristic` probes** —
  cheaper, no body transfer, still returns freshness headers.
- **Servers that lie via HEAD** (returning stale headers cached
  differently from GET responses) do exist. If FreshDAG detects that
  a HEAD response's ETag differs from a subsequent GET response's
  ETag under otherwise-identical conditions, demote the endpoint to
  `heuristic` for HEAD and prefer GET going forward.
- **GET with `If-None-Match`** is the fallback when the endpoint
  doesn't support HEAD (returns `405 Method Not Allowed`).
- Content-hash fallback MUST use GET; HEAD gives no body to hash.
- Cost model: HEAD is O(headers); GET-conditional-hit is O(headers);
  GET-conditional-miss is O(body); content-hash fallback is O(body)
  every time.

## Conditional requests

Recommended shape:

1. Recorded fingerprint carries an ETag or Last-Modified value.
2. On `check`, the probe issues:
   - `HEAD` (or `GET`) with `If-None-Match: <recorded-etag>` if ETag
     is present.
   - Otherwise `If-Modified-Since: <recorded-last-modified>` if only
     Last-Modified is present.
3. Interpret the response:
   - `304 Not Modified` → `ProbeResult::Match { observed_fp: <same> }`.
   - `200 OK` with new ETag → `ProbeResult::Drift { observed_fp: <new> }`.
   - `200 OK` with SAME ETag as recorded → suspicious; emit a
     diagnostic and treat as `Match`.
   - Any 5xx → `Unknown { retryable: true }`.
   - Any 4xx other than 429 → `Unknown { retryable: false }`.
   - 429 → `Unknown { retryable: true }` with backoff hint.
   - Timeout / network error → `Unknown { retryable: true }`.

The probe MUST NOT under any circumstance return `Match` when the
network attempt failed. This is the probe-contract §Failure Modes
rule and the primary source of invariant-#7 violations.

## Version-lost transitions (anti-thrash relevance)

The probe-contract §Anti-thrash Protocol governs this. Concretely,
for HTTP:

- The recorded trust class is per-`(url, probe_identity)`.
- If a `versioned` recorded dependency (had ETag) now sees a response
  with no ETag, the probe returns
  `Unknown { retryable: false, reason: "version signal lost" }` and
  the engine emits `probe.trust_demoted`.
- The engine forces re-observation of every artifact edge depending
  on this URL before the trust class on any certificate changes.
- Recovery requires N=2 consecutive observations at a higher trust
  class before the engine adopts an escalation.

Implementation note: the probe does not itself track history — that's
engine responsibility per the probe contract. The probe reports the
current observation; the engine applies the stability policy.

## Content-hash fallback

- **Opt-in per dependency.** A configuration flag on the recorded
  dependency (`fallback_to_content_hash: true`) tells the probe to
  fully GET and hash the body when header-based checks are
  inconclusive.
- **Cost visibility.** The probe emits a `probe.cost` metric so
  operators can see when fallback is happening.
- **Canonicalization decision (open question — escalate).** Body
  canonicalization for content hashing crosses the probe/comparator
  boundary (ARCHITECTURE §4, §8). The probe cannot hash arbitrary
  content without a canonicalization choice; but comparators are the
  layer that owns canonicalization. Recommendation: v0 hashes raw
  bytes without canonicalization, records the media type as metadata,
  and defers canonical-form hashing to when the comparator subsystem
  ships. This means two responses that differ only in HTTP-level
  compression (`Content-Encoding: gzip` vs identity) will be seen as
  different — accept this as pessimistic in v0.
- **Streaming.** Hash while streaming; do not buffer the full body
  in memory.
- **Size limits.** Refuse content-hash fallback above a configurable
  `max_fetch_bytes` (default 64 MiB); return `Unknown` for larger
  bodies with a clear reason.

## Cache-Control relevance

- `Cache-Control: max-age=N` gives a natural TTL. FreshDAG treats
  this as a *scheduling hint*, not a validity proof:
  - For a `heuristic` dependency, `max-age` sets the recheck cadence.
  - For a `volatile` dependency, `max-age` is the TTL used by the
    `volatile-unknown-after-ttl` rule.
- `Cache-Control: no-store` → `volatile`; refuse to cache the probe
  result.
- `Cache-Control: must-revalidate` → same as `no-store` for our
  purposes; every check is a fresh request.
- `Cache-Control: immutable` → treat as `versioned` (server is
  asserting the response will never change for this URL). Rare but
  useful for CDN-fingerprinted static assets.

## Anti-patterns (what the HTTP probe MUST NOT do)

- Return `Match` on network failure, DNS failure, or any 5xx.
- Trust a HEAD response's ETag blindly if past observations show
  HEAD/GET header divergence.
- Follow arbitrary redirect depth without a cap.
- Follow HTTPS-to-HTTP redirects.
- Store cookies or perform any auth-changing side effect.
- Promote a `heuristic` result to `versioned` because a single
  request happened to include an ETag.
- Silently downgrade trust when a version signal disappears — must
  emit `probe.trust_demoted`.
- Fabricate a canonicalization scheme for content hashing without
  coordination with the comparator subsystem.

## Recommended semantics for the W5.2 implementation

Behavioral rules the implementer must honor (API shape left to the
implementer):

1. Support `HEAD` and `GET` with conditional headers.
2. Return `ProbeResult::Match`/`Drift`/`Unknown` per the decision
   matrix and failure modes above.
3. Never return `Match` on any error.
4. Emit a `probe.checked` IR event with the observed fingerprint and
   trust class on every check.
5. Emit a `probe.trust_demoted` diagnostic when a version signal
   disappears; do not silently demote.
6. Redirect handling: max 5 hops, refuse cross-scheme downgrade,
   preserve original URL as key.
7. Content-hash fallback: opt-in, streaming, size-capped, no
   canonicalization in v0 (pessimistic).
8. Rate limiting: honor `Retry-After` on 429; emit `probe.cost`.
9. Coverage manifest declares `emits: ["probe.checked"]`,
   `capabilities: { "conditional_requests": true, "content_hash_fallback": true, "auth": false }`.
10. Support a per-dependency `max_fetch_bytes` cap; reject larger
    bodies with `Unknown`.

## Open questions

1. **Canonicalization for content hashing.** ARCHITECTURE §4 puts
   canonicalization inside comparators; §8 puts equivalence there
   too. The content-hash fallback path forces the probe to compute
   *some* hash before returning a `ProbeResult`. Options: (a) probe
   hashes raw bytes and records media type (v0 recommendation);
   (b) probe delegates to a per-scheme "content normalizer" that is a
   new abstraction; (c) probe returns raw bytes and the engine hashes.
   Option (a) is cheapest and preserves layer separation; option (c)
   is architecturally purest but doubles bandwidth for large bodies.
   **Escalate to architect.**
2. **Multi-representation URLs.** A single URL can return different
   representations depending on `Accept`, `Accept-Language`,
   `Accept-Encoding`. Does the fingerprint include the negotiated
   representation? Recommendation: yes — record the effective
   `Content-Type` in the observed fingerprint. Needs explicit
   confirmation.
3. **Auth-scoped probes.** How does the probe access credentials
   without becoming a secrets manager? Deferred; auth-configured
   probes are a v0.5 concern.
4. **Origin-level trust escalation.** Some origins consistently
   support ETag while others in the same certificate don't. Should
   trust-class discovery be origin-level, URL-level, or both? The
   anti-thrash protocol works per-URL today.

## Sources

- RFC 7232 — Conditional Requests (ETag, If-None-Match,
  If-Modified-Since). <https://datatracker.ietf.org/doc/html/rfc7232>
- RFC 7234 — HTTP/1.1 Caching (Cache-Control, max-age, no-store).
  <https://datatracker.ietf.org/doc/html/rfc7234>
- RFC 9110 — HTTP Semantics (current umbrella for the above).
  <https://datatracker.ietf.org/doc/html/rfc9110>
- Fielding, "ETag misuse" — recurring practitioner discussions on
  weak ETags across CDN caches (specific 2025-2026 citations should
  be re-verified by a researcher with web access before public use).
- Related contract: `docs/contracts/probe-contract.md`.
- Related invariants: `ARCHITECTURE.md §5` #7, #8; §6, §7.
