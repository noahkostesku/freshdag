# Novelty

FreshDAG operates at the intersection of five well-populated fields — build
systems, incremental computation, agent tracing, data lineage, and semantic
caching. This document exists so nobody working on FreshDAG mistakes a
well-worn primitive for our contribution.

Reviewers, funders, and skeptical users will cite the systems below in the
first minute of any conversation. We must be able to answer, in one
sentence, why each of them is not what we are building.

**Status.** This is a strong-prior first pass drawn from research memos.
It has known gaps (see §5). It should be revisited whenever we make a
public novelty claim and treated as living rather than final.

---

## 1. Collision Table

| System | What it does | Overlap with FreshDAG | What it does NOT do | Collision risk | Source |
| --- | --- | --- | --- | --- | --- |
| **LangSmith** | Records LLM/agent runs; per-run tool graph; dataset-driven eval; replay. | Captures the runtime call graph of an agent. | No dependency-based invalidation; no cross-run "this artifact is now stale." | High | https://smith.langchain.com |
| **Langfuse** | OSS agent tracing, prompt-version linkage, evals. | Records tool spans; links prompt versions to outputs. | No world-state watchers; no validity predicates. | High | https://langfuse.com |
| **AgentOps** | Session replay; cost/latency; tool-use analytics. | Captures tool call graph and step lineage. | No freshness model. | Medium | https://www.agentops.ai |
| **Arize Phoenix / OpenInference** | LLM tracing + drift detection on embeddings. | Drift is the closest analog to "stale". | Statistical, not causal-DAG; no per-artifact validity. | Medium | https://phoenix.arize.com |
| **Braintrust** | LLM eval + logging + prompt registry. | Prompt/version to output linkage. | No world-mutation-driven invalidation. | Low-medium | https://www.braintrust.dev |
| **Helicone / LangCache / GPTCache** | LLM request cache. | Cache hit reasoning is a component we consume. | TTL / semantic-similarity cache; no dependency DAG. | Low | https://www.helicone.ai |
| **OpenLineage + Marquez** | Data-pipeline lineage standard. | The canonical "make for data" prior art the tagline echoes. | Batch-data-scoped; no LLM/agent runtime introspection. | High (conceptual) | https://openlineage.io |
| **Pachyderm / DVC / LakeFS** | Data versioning + pipeline recompute on input change. | Direct spiritual ancestor. | Requires explicit pipeline definition; no dynamic dep inference from opaque agent execution. | High (conceptual) | https://dvc.org |
| **Dagster software-defined assets / Airflow data-aware scheduling** | Asset-based orchestration; recompute downstream when asset materializes. | Owns the "make for X" framing. | The DAG is authored, not inferred. | High (framing) | https://dagster.io |
| **Dagster `FreshnessPolicy` + `@observable_source_asset` + `AutoMaterializePolicy`** | Per-asset freshness SLAs; version tokens from observation-only source assets; eager auto-materialize on upstream drift. | *The* production system for cross-source-mutation-driven selective recompute on a data-asset DAG. | Requires assets to be authored; no agent-trace ingestion; no trust-class typing. | Very high (mechanism + framing) | https://docs.dagster.io/concepts/freshness-checks |
| **OpenLineage custom `facets`** | Arbitrary typed metadata attachable to a dataset run event. | The certificate format is essentially a `freshness` facet with a fixed schema. | No trust-class semantics; no cross-run probing behavior. | High (format) | https://openlineage.io/docs/spec/facets/ |
| **Marquez** | Reference OpenLineage server; stores and queries lineage. | Stores exactly the certificate-shaped data at scale. | No validity evaluation; no cross-session external probes. | High (implementation) | https://marquezproject.github.io/marquez/ |
| **in-toto attestations + SLSA provenance** | Signed, machine-checkable predicates about how an artifact was produced; Sigstore/Rekor transparency log. | Same "predicate attached to derived artifact" primitive; a security-oriented sibling of the certificate. | Not agent-aware; not designed for cross-session freshness. | High (primitive) | https://in-toto.io ; https://slsa.dev |
| **Reproducible Builds `.buildinfo`** | Portable manifest of what a build depended on, checkable by third parties. | Precedent for portable, checkable manifests. | Static declaration; not cross-session; not agent-aware. | Medium (format precedent) | https://reproducible-builds.org/docs/buildinfo/ |
| **Temporal.io / Restate durable execution** | Replays workflows from event history; skips completed steps. | Reviewers will raise "isn't this just durable execution over your agent?" | Replays authored workflows; no external-mutation invalidation; no trust classes. | Medium (misconception risk) | https://temporal.io ; https://restate.dev |
| **`nbdev` / `nbdime` cell-freshness / Jupyter cache invalidation** | Per-cell staleness given content-hash inputs. | Decade of prior art on "which cells are stale when this file changed." | Notebook-scoped; no trust classes. | Low-medium | https://nbdev.fast.ai |
| **`git-annex` / `datalad`** | Content-addressed data with derivable checkable provenance for research pipelines. | Adjacent to the certificate primitive. | Not agent-aware. | Low-medium | https://www.datalad.org |
| **Ninja `deps=gcc` / `.d` files (dynamic-inference-from-unmodified-tool angle)** | Dynamic dependency discovery from an unmodified tool (the C compiler). | Precedent for "trace an unmodified tool, discover deps." | Local static graph; not agent-aware. | Medium (direct claim collision) | https://ninja-build.org/manual.html |
| **Bazel / Buck2 / Nix** | Content-addressed build DAG with perfect incremental recompute. | Gold standard for minimal recomputation. | Requires hermetic, declared inputs; intolerant of freeform tool use and external mutable state. | High (mechanism) | https://bazel.build |
| **Ninja** | Static graph with `restat` early cutoff and `deps=gcc` dynamic discovery. | Both patterns FreshDAG needs (early cutoff, dynamic discovery). | Not agent-aware; not distributed; static graph. | Low-medium | https://ninja-build.org/manual.html |
| **Shake** | Monadic dynamic dependency discovery; verifying traces. | *The* correctness backbone for a system like FreshDAG. | Not agent-aware; not networked; assumes hermetic file-based tools. | Medium (must-cite) | https://shakebuild.com |
| **Rattle** | No dep declarations; traces `fsatrace` reads/writes of shell commands. | Closest to what FreshDAG's subprocess observer must do. | Local shell only; no agent semantics; no probes. | Medium | https://ndmitchell.com/downloads/paper-rattle-30_oct_2020.pdf |
| **Salsa (rust-analyzer)** | Demand-driven incremental computation; per-query revision + value-equality early cutoff. | The blueprint for equivalence-based propagation stopping. | In-memory pure computation only. | Medium (must-cite) | https://salsa-rs.github.io/salsa/ |
| **Adapton / Self-Adjusting Computation** | Formal from-scratch consistency for dynamic dependency graphs. | Theoretical foundation for minimality claims. | Not agent-aware; no probabilistic validity. | Medium | http://adapton.org/ |
| **Turborepo / Nx** | Task-level cache with declared inputs. | Coarse-grained precedent. | Under-declared `inputs` cause cache poisoning; static task graph. | Low | https://turbo.build |
| **Claude Code hooks + transcripts** | `PreToolUse` / `PostToolUse` / `Stop` hooks; JSONL transcripts; worktrees. | The observation substrate FreshDAG's Claude adapter consumes. | No dependency DAG, no freshness model, no cross-session artifact reasoning. | Very high (platform-owner) | https://docs.claude.com/en/docs/claude-code/hooks |
| **OpenTelemetry GenAI semconv** | Standardized spans for LLM/agent operations. | The substrate every adapter will align to. | Not a freshness system; no provenance fields. | Low (substrate) | https://opentelemetry.io/docs/specs/semconv/gen-ai/ |
| **LangGraph checkpoints + time-travel** | Explicit computation graph, checkpointing, human-in-the-loop replay. | Same substrate (graph over agent execution); LangChain's distribution muscle. | Graph is authored; no world-change invalidation. | High | https://langchain-ai.github.io/langgraph/ |
| **W3C PROV / PROV-O** | Provenance metamodel; cited in recent LLM-provenance work. | Formal vocabulary reviewers expect us to relate to. | Spec, not system. | Low (must-cite) | https://www.w3.org/TR/prov-overview/ |
| **FreshLLMs / FreshQA / temporal-RAG line** | Detect and repair stale factual answers via retrieval. | Shares "fresh" and the intuition that LLM outputs decay. | Answer-level via re-retrieval; no artifact graph; no minimal recompute. | High (naming + reviewer conflation) | https://arxiv.org/abs/2310.03214 |
| **Cognition / Devin / Factory / Poolside** | Long-lived agent sessions with artifact stores. | Same problem surface. | No public freshness model. | Medium (they could ship it) | https://cognition.ai |
| **Bauplan / Nessie / Iceberg catalogs** | Table-version-token-driven staleness on Iceberg tables. | Direct `versioned`-trust-class analog for the data-lake ecosystem. | Table-scoped; not agent-aware; no trust-class typing beyond version tokens. | High | https://www.bauplanlabs.com ; https://projectnessie.org |
| **W3C Verifiable Credentials `credentialStatus`** | Predicate-attached-to-artifact with explicit revocation/refresh mechanism. | Conceptually a `freshness` facet in a different ecosystem. | Not agent-aware; not a runtime system. | Medium | https://www.w3.org/TR/vc-data-model-2.0/ |
| **OPA / Rego policy over OpenLineage events** | Policy-driven lineage validation. | The "just write a Rego policy over your lineage store" attack — must pre-empt. | No trust-class typing; no coverage-deficit rule; policy runs after the fact. | Medium (rhetorical) | https://openlineage.io/docs/integrations/opa |
| **`uv.lock` / `pip-audit` / `npm overrides` freshness annotations** | Per-dependency staleness metadata in package ecosystems. | Not agent-aware, but reviewers will point at them. | Package-scoped; static; no probes. | Low | https://docs.astral.sh/uv/reference/settings/ |
| **EA-Graph** (2026) | Artifact-anchored verification memory for coding agents under upstream drift. Two lattices — Evidence (`UNKNOWN < PARTIAL < PROVEN`) and Freshness (`FRESH`/`STALE`); an "LLM quarantine" rule (no model output enters at `PROVEN`); anchor-completeness as a checkable property; a `STALE` fact on a path refuses the query with a rebuild obligation. | **The closest published system to §2.** A typed evidence lattice over agent-produced artifacts, bindings anchored from verification-session traces, a machine-checked never-promote rule, a coverage/completeness check, and refusal rather than promotion. | Re-reads local repository state by content hash only: no cross-session probing of external mutable state, no scheme-registered probes, no TTL/`volatile` class, no portable certificate. | **Very high (narrows §2 — see §5.7)** | https://arxiv.org/abs/2608.04278 |
| **AgentTrails** (2026) | Converts raw agent trajectories into structured provenance graphs; tool calls as computational actions, inputs/outputs as data artifacts; quotient graphs aligning recurring structure across runs. | Trace→provenance-graph inference with edge-evidence tiering (exact / semantic candidate / LLM-refined). | A viewer and sensemaking tool: no validity checking, no external probing, no invalidation propagation. | High (confirms trace-derived graphs are substrate, not contribution) | https://arxiv.org/abs/2607.18816 |
| **AgentFlow agent dependency graphs** (2026) | Builds agent dependency graphs (which agents invoke which tools, which tools touch which resources, how memory propagates) for static analysis of agent programs. | Owns the term "agent dependency graph". | Static analysis of program text; no runtime observation, no freshness. | Medium | https://arxiv.org/pdf/2607.01640 |
| **SkillDepAnalyzer / SKILL-DEP** (2026) | Models agent skills as dependency-bearing artifacts; recovers mixed skill/package/service dependency graphs; outperforms package-centric SBOM tooling. | "Agent artifacts have dependencies that must be recovered rather than declared." | Supply-chain risk analysis over skill manifests; no runtime trace ingestion, no freshness, no probes. | Medium | https://arxiv.org/abs/2607.01136 |
| **GRADE** (2026) | Two-layer graph over LLM agent execution — an execution layer for control flow, a dependency layer for what each step relies on — motivated explicitly by "values read early that go stale between steps." | Uses *staleness of a read value* as the motivating failure mode over a trace-inferred agent dependency graph. | Within-run diagnosis; no cross-session artifact validity, no probes, no trust classes, no certificate. | High (framing + naming) | https://arxiv.org/pdf/2606.22741 |
| **TVCACHE** (2026) | Stateful tool-value cache for post-training LLM agents: a tool-call graph plus sandbox snapshots, longest-prefix matching for reuse across RL rollouts. | Caching and reusing *tool results* keyed on a graph is what "don't rerun the agent" looks like from the caching side. | Reuse within RL training rollouts, keyed on call-prefix identity rather than external-state revalidation. | Medium (characterized from abstract only — reverify before any public comparison) | https://arxiv.org/pdf/2602.10986 |
| **"From Agent Traces to Trust" survey** (2026) | Survey of evidence tracing and execution provenance in LLM agents; names stale memory items and unsupported retrievals as open problems. | Establishes agent provenance as a populated field with named open problems — the literature review reviewers will hand us. | A survey, not a system. | Medium (must-cite) | https://arxiv.org/abs/2606.04990 |
| **OpenVEX / CSAF VEX** | Machine-readable exploitability statements about an artifact: a closed `status` set, a **closed `justification` vocabulary** (`component_not_present`, `vulnerable_code_not_present`, …), and a free-text `impact_statement` the spec discourages *precisely because it is not machine-readable*. | **The exact shape of ADR 0006**: a closed reason vocabulary plus a deliberately non-normative free-text sidecar, attached to a status assertion about an artifact. | Vulnerability scope; no trust classes, no probes, no freshness over time. | **High (format — direct hit on ADR 0006)** | https://github.com/openvex/spec/blob/main/OPENVEX-SPEC.md |
| **RFC 5280 `CRLReason` / OCSP revocation reasons** | Closed enum of reason codes attached to a machine-checkable validity assertion about a credential. | Oldest precedent for "closed reason-code vocabulary on a validity status." | Not artifact- or dependency-aware. | Medium (primitive precedent) | https://www.rfc-editor.org/rfc/rfc5280 |
| **SLSA v0.2 `metadata.completeness`** | The builder declares which parts of the provenance it claims complete (`materials`, `parameters`, `environment`); materials are **incomplete by default** unless the builder asserts otherwise. | **The coverage-deficit rule's direct ancestor**: provenance that declares its own blind spots so a verifier can refuse to over-trust it. | Self-declared by the builder rather than derived from a role-typed producer registry; one-shot at build time; not agent-aware. | **High (primitive — direct hit on the coverage-deficit rule)** | https://slsa.dev/spec/v0.2/provenance |
| **HTTP caching semantics — RFC 9110 §8.8.1 validators, RFC 9111 §4.2.2 heuristic freshness** | Strong vs. weak validator distinction (weak validators licensed only for weak comparison); freshness from `Cache-Control`/`Expires`, falling back to explicitly-named **"heuristic freshness"** when no explicit lifetime exists. | The `https` probe's trust mapping *is* this taxonomy renamed: strong ETag→`versioned`, weak ETag→`heuristic`, `Last-Modified`→`heuristic`, `no-store`→`volatile`. The word "heuristic" is theirs. | HTTP does not aggregate per-resource freshness into a validity verdict on a *derived* artifact, and has no notion of a producer's observation coverage. | **High (mechanism + naming)** | https://www.rfc-editor.org/rfc/rfc9110#section-8.8.1 ; https://www.rfc-editor.org/rfc/rfc9111#section-4.2.2 |
| **Event sourcing / CQRS; Datomic; Kafka log compaction + materialized views** | Append-only immutable fact log as sole source of truth; query-side state is a disposable projection rebuilt by deterministic replay. | Exactly `freshdag-store`'s shape: canonical `events.jsonl`, disposable `derived/`, a `(ts, producer, event_id)` linearization, and a digest binding a projection to its log. | Storage and architecture patterns; no dependency semantics, no validity, no external probing. | **High (mechanism — W2's store is unclaimable)** | https://martinfowler.com/eaaDev/EventSourcing.html ; https://www.datomic.com |
| **DataHub "Impact Analysis" / dbt `state:modified+` / Marquez downstream lineage** | Named, shipped features answering "this upstream changed — which downstream assets are affected?" over a stored lineage graph. | **Literally W3's reverse blast-radius index.** `DependencyId → consuming ArtifactId[]` is a flagship lineage feature, not a contribution. | Authored graphs (dbt) or ingested pipeline lineage (DataHub, Marquez); no trust classes, no per-artifact validity verdict. | **High (implementation — W3's reverse index is unclaimable)** | https://datahubproject.io/docs/act-on-metadata/impact-analysis ; https://docs.getdbt.com/reference/node-selection/methods |

Follow-up verification status (was: "memos could not confirm these
exist"). As of 2026-08-15 all but one are **confirmed real and now
tabled above**: AgentTrails, AgentFlow Agent-Dependency-Graph,
SkillDepAnalyzer, and TVCACHE all exist as named 2026 arXiv work. Only
**GroundedCache** remains unlocated; keep the adversarial default.

---

## 2. What Survives — the Defensible Wedge

**Status:** rewritten 2026-08-16 by `architect`, resolving the §5.7
escalation. The previous revision rested the wedge on the claim that no
system "encodes the 'heuristic never promotes to valid' rule as a
machine-checked property on their manifest." EA-Graph does, over an
agent-artifact graph. That argument is retired, not repaired.

The wedge, in one sentence:

> FreshDAG decides whether an artifact is still valid over a dependency
> set that was **discovered rather than declared**, types each dependency
> by validator strength, re-checks it against **external mutable state
> after the producing process is gone**, refuses to promote the verdict
> on missing evidence, and emits the whole judgment as a portable
> manifest a third party can re-check.

**Every conjunct in that sentence is somebody else's.** This is not a
concession extracted under review; it is the honest state of the art,
and §3 firewalls each one:

| Conjunct | Prior art that owns it |
| --- | --- |
| Discovered, not declared, dependency sets | Ninja `deps=gcc`; Rattle; Shake. For agents: LangSmith `parent_run_id`, AgentTrails. |
| Validator-strength typing (`exact`/`versioned`/`heuristic`/`volatile`) | RFC 9110 §8.8.1 strong vs. weak validators; RFC 9111 §4.2.2, which coins the term "heuristic freshness". Our four classes are that taxonomy plus content addressing. |
| A typed evidence lattice over agent artifacts, with a never-promote rule | EA-Graph (`UNKNOWN < PARTIAL < PROVEN`; "LLM quarantine"). |
| Cross-session observation of external mutable sources | Dagster `@observable_source_asset` + `FreshnessPolicy`; Iceberg/Nessie version tokens. |
| Aggregating per-input freshness into a verdict on a derived thing | Make, Bazel, Salsa, Dagster, dbt. |
| Provenance declaring its own blind spots so a verifier can refuse | SLSA v0.2 `metadata.completeness`; EA-Graph anchor-completeness. |
| A portable, third-party-checkable manifest about a derived artifact | in-toto / SLSA; Reproducible Builds `.buildinfo`. |
| Closed reason vocabulary plus a non-normative free-text sidecar | OpenVEX `justification` + `impact_statement`; RFC 5280 `CRLReason`. |
| Append-only log with a disposable replayed projection | Event sourcing / CQRS; Datomic; Kafka log compaction. |
| Reverse blast-radius index | DataHub Impact Analysis; dbt `state:modified+`. |

**What is unoccupied is the conjunction, and nothing smaller.** The
useful form of the claim is the gap each nearest neighbour leaves:

- **HTTP** has the type system, one resource at a time. It never
  aggregates a set of validators into a verdict about something
  *derived from* those resources, and has no notion of a producer's
  observation coverage.
- **Dagster** has the aggregation *and* the cross-session observation of
  external mutable sources — but the dependency set is authored. You
  cannot put a freshness policy on a dependency you did not know your
  agent had.
- **EA-Graph** has the lattice, the refusal, and the agent-artifact
  scope — but its anchors are local repository content re-read by
  content hash. It has no model of a world outside the repository that
  moves on its own.
- **SLSA** has the blind-spot declaration and the portable manifest —
  but self-declared by the builder, one-shot at build time, and never
  re-evaluated as the world moves.

No system evaluates typed, re-probed, cross-session validity over a
dependency set it did not ask the user to declare. That is the whole
claim. It is smaller than the previous revision's claim, and it is the
part that is true.

**This is an engineering claim, not a research claim, and we say so.**
The correct public sentence is *"nobody has assembled this"*, never
*"nobody has thought of this."* A reviewer who says "these are all known
primitives" is right, and agreeing costs us nothing: the interesting
question is whether the assembly is useful, which is an evaluation
question (`docs/EVALUATION.md`), not a literature question. Any
positioning document that argues the literature question has already
lost.

**Falsification conditions.** Any of these retires the wedge. We name
them so they are a watch item rather than a surprise:

1. Dagster, dbt, or DataHub ingesting agent traces as asset definitions.
   This is the shortest path — they already own every other conjunct.
2. EA-Graph or a successor adding an external-probe interface for
   non-repository anchors.
3. Anthropic shipping cross-session artifact freshness inside Claude
   Code (`docs/BUILD_PLAN.md §8`).

None of these is hard for its owner. Our defense is therefore not the
idea. It is (a) adapter-agnosticism, (b) the certificate as a portable
artifact other tools can consume rather than a proprietary status, and
(c) refusing correctness shortcuts as coverage grows.

**Naming and framing.**

- Keep: "Know when agent outputs go stale."
- Never: "make for X" as a novelty claim. Dagster owns it; it is
  evocation only.
- Do **not** elevate trust-class typing. RFC 9111 got there first and
  restated it in 2022, and EA-Graph did the agent-artifact version.
  Elevate **the certificate**: a machine-checkable statement about an
  artifact whose dependency list nobody had to write down.

---

## 3. Novelty Firewall

We MUST NOT claim to have invented, nor allow the marketing/docs to imply
we invented, the following. Anyone editing README, marketing copy, or
public talks must reread this list.

| Claim we must never make | Why |
| --- | --- |
| Dependency-graph-driven minimal recomputation | Make, 1976. |
| Content-addressed build caching | Bazel, Nix. |
| Dynamic dependency tracking | Adapton, self-adjusting computation, Salsa, differential dataflow. |
| Dynamic dependency discovery from an unmodified tool | Ninja `deps=gcc`, Rattle for shell commands. |
| Trace-based dependency discovery for shell commands | Rattle. |
| Dependency inference from agent traces | LangSmith `parent_run_id`, Langfuse observations, OTel GenAI spans — the graph is already in the trace store. |
| Monadic dynamic dependencies in a build system | Shake. |
| Value-equality-based early cutoff | Salsa. |
| Agent tracing / tool-call observability | LangSmith, Langfuse, AgentOps, Phoenix, OTel GenAI. |
| Data / pipeline lineage | OpenLineage (custom facets), Marquez, PROV, Pachyderm, DVC. |
| Freshness SLAs and observation-driven auto-materialize on a data-asset DAG | Dagster `FreshnessPolicy` + `AutoMaterializePolicy`. |
| LLM output staleness detection | FreshLLMs, temporal RAG. |
| Semantic caching of LLM calls | GPTCache, LangCache. |
| Agent session replay / time-travel | LangGraph. |
| Durable execution / event-replay orchestration | Temporal, Restate. |
| Signed machine-checkable predicates on derived artifacts | in-toto, SLSA, Sigstore/Rekor. |
| Portable third-party-checkable build manifests | Reproducible Builds `.buildinfo`. |
| Provenance graphs as a concept | W3C PROV. |
| "Make for X" as a framing | Dagster and predecessors. |
| A closed reason-code vocabulary on a machine-checkable status assertion, with a non-normative free-text sidecar | OpenVEX `justification` + `impact_statement`; RFC 5280 `CRLReason`. **This is ADR 0006's shape.** |
| Provenance that declares its own blind spots so a verifier can refuse to over-trust it | SLSA v0.2 `metadata.completeness`; EA-Graph anchor-completeness. **This is the coverage-deficit rule's genus.** |
| Strong-vs-weak validator trust distinction, or "heuristic freshness" for HTTP resources | RFC 9110 §8.8.1, RFC 9111 §4.2.2. The `https` probe implements their taxonomy; it did not invent it. |
| Append-only event log whose derived state is a disposable projection rebuilt by deterministic replay | Event sourcing / CQRS, Datomic, Kafka log compaction. |
| Reverse-lineage blast-radius / downstream impact analysis | DataHub Impact Analysis, dbt `state:modified+`, Marquez. |
| Typed evidence or confidence lattices over agent-produced artifacts, or a never-promote rule for LLM-generated evidence | EA-Graph (`UNKNOWN < PARTIAL < PROVEN`, "LLM quarantine"). |
| Staleness of a value read earlier in an agent run as a named failure mode | GRADE; "From Agent Traces to Trust". |

---

## 4. Adjacent Territory We Must Not Wander Into

Each of the following would silently narrow FreshDAG into a worse copy of
an existing product. If our roadmap starts sliding into one of them,
somebody has drifted; reread this section.

- **Generic distributed tracing / observability of agents.** LangSmith,
  Langfuse, and OTel GenAI own this. FreshDAG consumes traces; it is not
  another trace store.
- **Static analysis of agent code or prompts.** Different problem.
  Interesting; not ours.
- **Visualizing execution graphs.** Every tracing tool has a DAG view.
  Pretty pictures are a surface, not the wedge.
- **Prompt / model versioning for reproducibility.** Braintrust,
  LangSmith. A dependency we consume; not our contribution.
- **Answer-level factual freshness via re-retrieval (temporal RAG).**
  Adjacent; categorically different. FreshDAG operates on artifacts, not
  individual answer factuality.
- **Workflow-authoring frameworks.** If our pitch becomes "author your
  agent with FreshDAG," we have become a worse LangGraph.
- **Cost / latency observability.** Helicone, AgentOps. Not us.
- **Data pipeline orchestration with LLM operators.** If we're "Dagster
  where the operators call Claude," we are a plugin.

FreshDAG is **injectable into existing agent workflows**, not a new
orchestration DSL. This is architectural invariant #16.

---

## 5. Known Gaps in This Review

The research memos that produced this document were generated without live
web access. Before FreshDAG makes a public novelty claim, the following
must be verified against current sources:

1. ~~**Named systems we could not verify:** AgentTrails, AgentFlow
   Agent-Dependency-Graph, SkillDepAnalyzer, TVCACHE, GroundedCache.~~
   **Resolved 2026-08-15 (Wave 2 novelty review).** Four of the five are
   confirmed real and are now rows in §1. None matches our exact framing;
   the wedge did not collapse on these. **GroundedCache** is still
   unlocated — keep the adversarial default for it alone.
2. **Dagster 2025 cycle for LLM asset sensors and MCP-driven upstream
   sources.** Cited from memory; confirm before any Dagster comparison
   is made in public.
3. **OpenLineage current version** and its facet extension API.
4. **2025-2026 arXiv on "agent provenance" and "incremental agent
   computation".** New preprints likely land monthly.
5. **Anthropic Claude Code changelog** for any indication of a
   freshness / staleness feature in flight. This is the highest
   platform-owner risk — and one the novelty adversary explicitly
   escalated from a novelty risk to an existential product risk (see
   `docs/BUILD_PLAN.md §7` for how to track it).
6. **Recent Cognition / Factory / Poolside / Adept** product surfaces.
   Long-lived agent sessions are the most likely to ship a competing
   freshness story.
7. ~~**OPEN ESCALATION TO `architect` — §2's supporting argument is
   falsified as written.**~~ **RESOLVED 2026-08-16 by `architect`. §2
   rewritten.**

   The escalation was upheld. EA-Graph does encode a machine-checked
   never-promote rule over an agent-artifact graph, so §2's supporting
   sentence was rhetorically dead and has been deleted rather than
   reworded. Independently re-verified against arXiv:2608.04278: the
   abstract states the system "keeps evidence strength separate from
   freshness" and that a claim "becomes unprovable rather than guessed"
   when replacement content is unavailable, over sub-path-granular,
   alias-resolved, content-anchored local repository artifacts. It does
   not mention probing external mutable state.

   **Where the architect went further than the reviewer.** The reviewer
   proposed shifting §2's load onto "trust-class typing bound to
   cross-session external-mutation probing, in a portable artifact."
   That replacement load-bearer is also individually occupied, in two
   places the reviewer did not press:

   - **Cross-session observation of external mutable state** is
     Dagster's `@observable_source_asset`, shipped, and Iceberg/Nessie's
     version tokens.
   - **The portable third-party-checkable artifact** is in-toto/SLSA and
     Reproducible Builds `.buildinfo`.
   - And **trust-class typing itself** is not merely "narrowed" by
     EA-Graph; the four classes are RFC 9110/9111's strong-validator /
     weak-validator / heuristic-freshness / no-validator taxonomy with
     content addressing added. The word "heuristic" is theirs.

   So no single conjunct survives, and any §2 that rests on one is a
   fresh hostage. The rewritten §2 therefore rests on the **conjunction
   over an undeclared dependency set** and states explicitly that every
   component is prior art. It also downgrades the register of the claim
   from research novelty to engineering assembly, and names three
   falsification conditions.

   Consequences accepted with this resolution:

   - §2 no longer supports any marketing sentence of the form "we
     invented X." `.claude/rules/novelty.md` already requires
     `novelty-reviewer` on README changes; that review should now check
     the *register* of the claim, not only its content.
   - "Elevate trust-class-typed certificates" is retired as a
     positioning instruction. What is elevated is the certificate over
     an undeclared dependency set.
   - The wedge's defensibility is now explicitly execution-based, which
     makes `docs/EVALUATION.md` — not this document — the place the
     claim is won or lost. Wave 3 is scoped accordingly
     (`docs/BUILD_PLAN.md §6`).

Owner: `novelty-reviewer` agent (see `.claude/agents/novelty-reviewer.md`).

---

## 6. How to Use This Document

- Every ADR that expands FreshDAG's public surface must cite the relevant
  row(s) here and state why the expansion does not become the adjacent
  system.
- The `novelty-reviewer` agent is empowered to reject work on novelty
  grounds; disagreements escalate to the architect, not silently resolved
  in-line.
- If a new collision is discovered, update this document in the same PR
  that acknowledges it. Do not carry an untracked collision.
