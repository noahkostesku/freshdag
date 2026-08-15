# ADR 0002: Canonical Execution IR as the adapter/observer boundary

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** architect
- **Consulted:** agent-runtime research memo (Workstream C)

## Context

FreshDAG will have multiple runtime adapters (Claude Code first;
LangGraph, OpenAI Agents SDK, Anthropic Agent SDK, MCP-native runtimes
plausible) and multiple observers (fsatrace, strace, eBPF, Detours).
Without a stable intermediate representation, engine and store code
would branch on runtime identity and observer identity — architectural
death.

## Decision

Adapters and observers emit into a single canonical execution IR
defined in `docs/contracts/execution-ir.md`. Downstream subsystems
(store, engine, CLI) read only the IR and never runtime-specific
payloads.

The IR is:

- JSON-encoded (JSONL on disk) at v0. No custom binary format until
  the engine is real.
- Append-only per producer.
- Versioned via `schemas/execution-ir/v0.1.json` and a
  `producer_version` field.

## Consequences

- Adapters and observers have a single translation contract to
  implement. Their internals can vary wildly.
- The engine is testable purely from recorded IR streams; no live
  agent required in fixtures.
- Adding a new adapter is a small, well-scoped project.

## Rejected Alternatives

- **OpenTelemetry GenAI spans as the primary event vocabulary.**
  Rejected because OTel GenAI has no provenance / dependency fields.
  FreshDAG will align field names where they overlap
  (`gen_ai.tool.name`, `gen_ai.conversation.id`) but defines its own
  IR.
- **Per-adapter engine plugins.** Rejected — would push runtime
  concepts into the engine.
