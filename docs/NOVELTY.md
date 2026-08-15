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

The founding brief's thesis is a synthesis. After the novelty-adversary
review argued that "dynamic inference of the dep graph from an unmodified
agent runtime" is a rebranding of trace ingestion (LangSmith's
`Run.parent_run_id` already gives you the graph; the "inference" is a
`SELECT`), we tighten the surviving wedge:

> **Typed-trust-class validity aggregation over agent-produced
> artifacts, driven by cross-session probes of external mutable state,
> whose per-dependency bindings are derived from agent trace data.**

Why this phrasing survives where the looser one does not:

- **Trust-class-typed aggregation** (ADR 0004: `exact | versioned |
  heuristic | volatile`) is the load-bearing primitive. No trace store,
  no lineage graph, and no incremental-computation framework encodes
  the "heuristic never promotes to valid" rule as a machine-checked
  property on their manifest. This is what makes FreshDAG resistant to
  "just run LangSmith with a SQL query."
- **Cross-session external-mutation probing** is the acting-on-the-graph
  step. Tracing tools record; Dagster asset sensors act but on authored
  assets; FreshDAG acts on artifacts whose dependencies were inferred
  from trace ingestion.
- Trace-derived edges are consumed **substrate**, not contribution.
  We do not claim to have invented dependency inference from tool
  traces.

The name and framing survive with adjustments:

- Keep: "Know when agent outputs go stale."
- Downshift: "make for conclusions about a changing world" is evocation
  only, never a novelty claim. Dagster owns "make for X."
- Elevate: **trust-class-typed validity certificates** — the shareable
  primitive. Certificates are OpenLineage-facet-shaped in transport but
  their `trust_class` typing + heuristic-cap rule is what has no clean
  precedent.

The narrowing does not shrink the product; it protects it. A reviewer
who claims "you're just LangSmith + a SQL query" now has to explain
why LangSmith enforces `heuristic → likely-valid`, checks external
mutable state cross-session, and refuses to promote `Unknown` — which
it does not.

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
7. **OPEN ESCALATION TO `architect` — §2's supporting argument is
   falsified as written.** §2 justifies the wedge by asserting that "no
   trace store, no lineage graph, and no incremental-computation
   framework encodes the 'heuristic never promotes to valid' rule as a
   machine-checked property on their manifest." **EA-Graph** (§1) does
   encode exactly that rule, machine-checked, over an agent-artifact
   graph: its LLM-quarantine rule is "no model output enters at
   `PROVEN`", and its anchor-completeness check is a coverage-deficit
   analogue. EA-Graph is technically none of the three named categories,
   so the sentence is not literally false — but it is rhetorically dead,
   and a reviewer will say so in the first minute.

   Two further §1 additions narrow the same territory from the
   supply-chain side: **SLSA `metadata.completeness`** predates the
   coverage-deficit rule as a primitive, and **OpenVEX** predates ADR
   0006's closed-reason-vocabulary-plus-non-normative-detail shape.

   The one-sentence conjunction in §2 still has no match, because
   EA-Graph does not probe **external mutable state across sessions** and
   emits no **portable certificate**. The novelty-reviewer's position is
   therefore: the wedge survives, but its *load* must shift from
   "trust-class typing is unprecedented" to "trust-class typing bound to
   cross-session external-mutation probing, in a portable artifact, is
   unprecedented." Rewriting §2 is the architect's call, not the
   reviewer's.

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
