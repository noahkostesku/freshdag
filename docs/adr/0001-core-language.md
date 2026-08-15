# ADR 0001: Rust for the core

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** architect
- **Consulted:** build-systems research memo (Workstream B)

## Context

FreshDAG's core is a correctness-critical, long-lived, mostly-synchronous
system that will run inside CI, cron, and long-lived local daemons. It
also needs to be embeddable (Claude Code hook binaries, subprocess
wrappers) with low startup overhead.

Candidate languages considered:

- **Rust** — memory safety, performance, single-binary deploy, strong
  ecosystem for parsing / hashing / CLI tooling, precedent
  (Salsa, rust-analyzer, Buck2 core).
- **Go** — deploy simplicity, but weaker type system for a
  domain-model-heavy core.
- **Python** — best library ecosystem for LLM/agent integrations, but
  wrong tool for a correctness-critical evaluator that must run inside
  every hook.
- **TypeScript** — good for the future UI, wrong for the core.

## Decision

The FreshDAG core, engine, store, observer, probes, adapters, and CLI
are Rust. Only crate `apps/web` is exempt (it is a browser UI).

Rust `edition = "2021"`, MSRV `rust-version = "1.80"` at v0. `forbid`
`unsafe_code`. Clippy pedantic on by default via workspace lints.

## Consequences

- All workstreams share one language, one lint config, one test
  runner.
- Integrators wanting a Python-side API get one via a Rust-based
  Python binding later (PyO3); it is not in v0 scope.
- Agent-facing scripting (recipes) is language-neutral: an agent can
  be written in any language; FreshDAG wraps its execution.

## Alternatives Rejected

- **Go**: chosen against because the domain model is
  algebraic-data-type-heavy (`Fingerprint`, `Validity`, `Comparator`
  variants). Rust's enums fit this shape better than Go's
  interfaces-plus-tagged-structs.
- **Python core**: chosen against because FreshDAG must run in every
  Claude Code hook and must be embeddable. Python startup latency
  would be user-visible.
