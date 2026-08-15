# `https://` probe conformance fixtures

Executable form of `docs/contracts/probe-contract.md` and the decision
matrix in `docs/research/http-validity-probes.md`, for the `https://`
probe.

Run by `crates/freshdag-probes/tests/https_conformance.rs`. **Adding a
fixture requires no test-code change.** Any directory under this root
containing a `scenario.json` is a fixture, at any depth, so new
categories are free to add.

## No network

Every fixture drives `ScriptedTransport`: an in-memory queue of
pre-decided turns. No socket is opened, no name is resolved, nothing is
timed. This is not only about CI hygiene — several cases here (DNS
failure, TLS failure, an `https:` origin redirecting to `http:`) cannot
be produced by a loopback server at all. That is why the transport seam
exists.

The real `reqwest` transport is covered separately in
`crates/freshdag-probes/tests/https_loopback.rs`, which binds
`127.0.0.1:0` and also never leaves the machine.

## Layout

```
https/
  decided/      checks that reach a verdict (Match or Drift)
  unknown/      checks that must refuse to decide
  adversarial/  endpoints that are actively unhelpful
```

The split is documentation, not semantics: the walker recurses and does
not care which directory a fixture lives in.

## Schema

`scenario.json`:

| field | required | meaning |
| --- | --- | --- |
| `description` | yes | Why this fixture exists. Not asserted on. |
| `key` | yes | The dependency key. Constant across checks: a redirect target is metadata, never identity. |
| `config` | no | Probe tunables; each field defaults to `HttpsProbeConfig::default()`. |
| `checks[]` | yes | One or more independent checks, run in order. |

`config` fields: `max_redirects`, `max_fetch_bytes`,
`allow_plaintext_http`, `prefer_head`, and `content_hash_fallback`,
which is `{"mode": "never" | "always" | "for_keys", "keys": [...]}`.

Each `checks[]` entry:

| field | required | meaning |
| --- | --- | --- |
| `name` | yes | Names the check in failure output. |
| `recorded_fingerprint` | yes | Wire form, e.g. `etag:"v1"`, `mtime:Sun, 06 Nov 1994 08:49:37 GMT`, `blake3:<64 hex>`, `custom:immutable`. |
| `turns[]` | yes | The endpoint's scripted replies, consumed in request order. |

A turn is either

```json
{ "kind": "respond", "status": 200, "headers": [["ETag", "\"v2\""]], "body": "..." }
```

or

```json
{ "kind": "fail", "error": "timeout" }
```

`headers` is a **list of pairs**, not a map: wire order is preserved and
duplicates are significant (see `unknown/ambiguous-duplicate-etag`).
`body` is either a UTF-8 string or `{"repeat_byte": "x", "len": 65536}`,
which lets a fixture describe an oversized body without shipping one.
`error` is one of `timeout`, `dns`, `connect`, `tls`, `body`,
`invalid-request`, `other`.

`expected.json` mirrors `checks[]` one-for-one:

| field | required | meaning |
| --- | --- | --- |
| `result` | yes | `match`, `drift`, or `unknown`. |
| `observed_fingerprint` | no | Wire form; asserted when present. |
| `trust_class` | no | `exact`, `versioned`, `heuristic`, `volatile`. |
| `reason` | no | **Exact bytes** of `Unknown::reason`. |
| `retryable` | no | `Unknown::retryable`. |
| `diagnostics` | no | Full set of diagnostic codes, order-insensitive. Asserting `[]` asserts *no* diagnostics. |
| `requests` | no | HTTP requests the check should have issued. |

Two properties are worth stating explicitly because they are the
reason several fields exist:

- **`reason` is asserted byte-for-byte.** It becomes a certificate
  reason's non-normative `detail`, and `detail` sits inside the
  `cert_id` preimage. A reason that drifts between runs makes
  certificates unreproducible with nothing looking wrong. A separate
  test re-runs every fixture and diffs the verdicts to catch exactly
  that.
- **`requests` is asserted where refusal must precede the request.**
  `unknown/cross-scheme-downgrade` expects `requests: 1` — the
  downgraded hop must never be issued, not merely ignored. The
  malformed-recorded-fingerprint and `exact`-without-fallback fixtures
  expect `requests: 0`.

## What the fixture set is for

Invariant #7 says an unverifiable dependency is `Unknown`, never fresh.
At the probe boundary that is one sentence: **no error path may produce
`Match`**. The `unknown/` directory is that sentence enumerated, and the
walker additionally asserts, for every fixture expecting `unknown`, that
the result is not a `Match` — independently of the `result` string
comparison that already covers it.

## The honest boundary: ETag churn

`docs/contracts/probe-contract.md` §Testing asks for "an endpoint that
flip-flops its ETag with no content change."
`adversarial/etag-flip-flop/` is that endpoint, and it is also where
this probe's limit has to be written down rather than papered over.

**The probe is stateless by contract.** `Probe::check` receives
`(key, recorded_fp, ttl_hint)` and nothing else — no history, no prior
observations, no per-URL memory. Consequently:

- Checks 1 and 2 (`"v1"` → `"v2"`, then `"v2"` → `"v1"`) are `Drift`.
  From inside a single check they are **indistinguishable** from an
  endpoint that legitimately changed twice. Nothing in either response
  says "this is the same bytes you already had." Reporting anything
  other than `Drift` would be inventing information.
- Recognising the *round trip* — that we are back at a validator we have
  seen before, with no evidence of intervening change — requires
  comparing observation N against observations N−1 and N−2. That is
  cross-check state, and the probe contract explicitly assigns it to the
  engine: "the probe does not itself track history — that's engine
  responsibility." §Anti-thrash Protocol is where the stability policy
  lives.
- Check 3 IS statelessly detectable: we sent `If-None-Match: "v1"` and
  the origin answered `200 OK` with `ETag: "v1"`. The origin has
  contradicted itself inside one exchange — it served a full
  representation while simultaneously asserting the validator we
  conditioned on. The probe returns `Match` (the validator genuinely is
  unchanged) and emits an `etag-instability` diagnostic saying the
  endpoint ignores conditional requests.

So: the fixture proves the *detectable* half and documents the
undetectable half. The undetectable half is not a bug in this probe and
should not be fixed here; the correct place is the engine's anti-thrash
policy, fed by the `etag-instability` and `probe.trust_demoted`
diagnostics this probe emits. If a future change gives probes a history
channel, this README is the note that says what to revisit.

`adversarial/etag-echo-with-no-store/` is the same shape one step
further: the endpoint both ignores conditional requests and forbids
caching. The verdict is `Match` at `volatile`, which cannot aggregate to
`valid` on a certificate. The probe is saying "the tag is the same, and
I do not think that means much."

## Adding a fixture

1. `mkdir fixtures/probe-conformance/https/<category>/<name>/`
2. Write `scenario.json` and `expected.json`.
3. `cargo test -p freshdag-probes --test https_conformance`

Write the `description` for a reader who does not already know why the
case is interesting. A fixture that only asserts is worth less than a
fixture that also explains.
