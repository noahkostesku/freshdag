//! Deterministic fixtures for the engine's unit tests.
//!
//! Everything here is synthetic and hermetic: no filesystem, no
//! network, no wall clock. A test that needed any of those would be
//! non-reproducible, which `.claude/rules/testing.md` forbids.

use std::fmt::Write as _;
use std::sync::Arc;

use freshdag_core::artifact::ArtifactId;
use freshdag_core::computation::ComputationId;
use freshdag_core::dependency::TrustClass;
use freshdag_core::ir::{CoverageManifest, EventKind, EventKindPattern, IrEvent, ProducerRole};
use freshdag_store::CoverageRegistry;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{CheckOutcome, Engine, FrozenClock, ProbeRegistry};

pub const ADAPTER: &str = "freshdag-adapter-claude";
pub const OBSERVER: &str = "freshdag-observer-fsatrace";
pub const VERSION: &str = "0.1.0";
pub const UNREGISTERED: &str = "some-unregistered-tool";

const DEFAULT_ADAPTER_EMITS: &[&str] = &[
    "session.*",
    "computation.*",
    "tool.*",
    "fs.read",
    "fs.write",
    "artifact.produced",
    "diagnostic",
];
const OBSERVER_EMITS: &[&str] = &["fs.*", "proc.*", "net.*", "diagnostic"];
const PROBE_EMITS: &[&str] = &["probe.checked", "diagnostic"];

/// A deterministic 64-char lowercase hex string derived from a label.
///
/// Not a real BLAKE3 digest — the tests only need well-formed,
/// distinguishable, reproducible fingerprints, and depending on a hash
/// crate for that would be gratuitous.
pub fn blake3_of(label: &str) -> String {
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    let mut out = String::with_capacity(128);
    for chunk in 0..8u64 {
        for b in label.as_bytes() {
            state ^= u64::from(*b);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
        state = state.wrapping_add(chunk).wrapping_mul(31);
        write!(out, "{state:016x}").expect("writing to a String cannot fail");
    }
    out.truncate(64);
    out
}

pub fn manifest(producer: &str, role: ProducerRole, emits: &[&str]) -> CoverageManifest {
    CoverageManifest {
        producer: producer.to_string(),
        version: VERSION.to_string(),
        role,
        platforms: Vec::new(),
        emits: emits.iter().map(|s| EventKindPattern::new(*s)).collect(),
        partial: std::collections::BTreeMap::new(),
        capabilities: std::collections::BTreeMap::new(),
        known_limitations: Vec::new(),
    }
}

/// A synthetic computation: lifecycle events, whatever body events the
/// test asked for, one output file, one artifact.
pub struct Fixture {
    computation: String,
    body: Vec<(String, EventKind, serde_json::Value)>,
    adapter_emits: Vec<String>,
    observer: bool,
    unregistered: bool,
    probe_producers: Vec<String>,
    artifact: ArtifactId,
    extra: Vec<IrEvent>,
}

impl Fixture {
    pub fn new(name: &str) -> Self {
        Self {
            computation: format!("comp:{}", blake3_of(name)),
            body: Vec::new(),
            adapter_emits: DEFAULT_ADAPTER_EMITS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            observer: false,
            unregistered: false,
            probe_producers: Vec::new(),
            artifact: ArtifactId(format!("blake3:{}", blake3_of(&format!("{name}/artifact")))),
            extra: Vec::new(),
        }
    }

    /// Append already-built events to the log verbatim.
    ///
    /// For replaying the engine's own emissions: two fixtures built from
    /// the same name share a computation id and an artifact id, so the
    /// events one engine emitted are addressed to the computation the
    /// next one evaluates.
    pub fn with_extra_events(mut self, events: impl IntoIterator<Item = IrEvent>) -> Self {
        self.extra.extend(events);
        self
    }

    /// Replace the adapter's declared `emits` list.
    pub fn with_adapter_emits(mut self, emits: &[&str]) -> Self {
        self.adapter_emits = emits.iter().map(|s| (*s).to_string()).collect();
        self
    }

    /// An `fs.read` with a content hash, which the store turns into an
    /// `exact` `file://` dependency.
    pub fn with_file_edge(mut self, path: &str, fingerprint: &str) -> Self {
        self.body.push((
            ADAPTER.to_string(),
            EventKind::FsRead,
            serde_json::json!({ "path": path, "size": 128, "hash": fingerprint }),
        ));
        self
    }

    /// An `fs.read` the producer could not fingerprint — what
    /// `DiskContent` emits for a file over its byte cap, unreadable, or
    /// deleted between stat and read. The store keeps it as an
    /// `ExcludedEdge` with `NoFingerprint`, never as a dependency.
    pub fn with_unfingerprinted_read(mut self, path: &str) -> Self {
        self.body.push((
            ADAPTER.to_string(),
            EventKind::FsRead,
            serde_json::json!({ "path": path, "size": 0, "size_observed": false }),
        ));
        self
    }

    /// A `probe.checked`, which is how external dependencies enter the
    /// graph (the store deliberately derives no edge from a tool call).
    pub fn with_probe_edge(
        mut self,
        scheme: &str,
        key: &str,
        fingerprint: &str,
        trust_class: TrustClass,
        ttl_seconds: Option<u64>,
    ) -> Self {
        let producer = format!("freshdag-probe-{scheme}");
        let mut payload = serde_json::json!({
            "scheme": scheme,
            "key": key,
            "observed_fingerprint": fingerprint,
            "trust_class": match trust_class {
                TrustClass::Exact => "exact",
                TrustClass::Versioned => "versioned",
                TrustClass::Heuristic => "heuristic",
                TrustClass::Volatile => "volatile",
            },
            "result": "match",
        });
        if let Some(ttl) = ttl_seconds {
            payload["ttl_seconds"] = ttl.into();
        }
        self.body
            .push((producer.clone(), EventKind::ProbeChecked, payload));
        self.probe_producers.push(producer);
        self
    }

    /// A `bash` tool invocation, which creates an observation obligation
    /// only an observer can discharge.
    pub fn with_bash_invocation(mut self) -> Self {
        self.body.push((
            ADAPTER.to_string(),
            EventKind::ToolInvoked,
            serde_json::json!({
                "tool_name": "bash",
                "tool_kind": "bash",
                "tool_input": { "command": "python script.py" },
                "cwd": "/repo"
            }),
        ));
        self
    }

    /// A `net.fetch`, an effect kind the default adapter manifest does
    /// not declare.
    pub fn with_net_fetch(mut self) -> Self {
        self.body.push((
            ADAPTER.to_string(),
            EventKind::NetFetch,
            serde_json::json!({ "url": "https://acme.com/x", "method": "GET", "status": 200 }),
        ));
        self
    }

    /// An observer contributing to the computation, with `fs.*` coverage.
    pub fn with_observer(mut self) -> Self {
        self.observer = true;
        self.body.push((
            OBSERVER.to_string(),
            EventKind::FsStat,
            serde_json::json!({ "path": "/repo/config.json", "existed": true }),
        ));
        self
    }

    /// A producer that emits without registering a coverage manifest, so
    /// its silences cannot be interpreted.
    pub fn with_unregistered_producer(mut self) -> Self {
        self.unregistered = true;
        self.body.push((
            UNREGISTERED.to_string(),
            EventKind::Diagnostic,
            serde_json::json!({ "message": "something happened" }),
        ));
        self
    }

    fn base_ts() -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000)
    }

    fn events(&self) -> Vec<IrEvent> {
        let base = Self::base_ts();
        let mut events = Vec::new();
        let mut push =
            |producer: &str, offset: i64, kind: EventKind, payload: serde_json::Value| {
                let index = u128::try_from(events.len()).expect("small");
                events.push(IrEvent {
                    event_id: Uuid::from_u128(0x1_0000_0000_0000_0000_0000 + index),
                    producer: producer.to_string(),
                    producer_version: VERSION.to_string(),
                    session_id: "sess".to_string(),
                    computation_id: Some(self.computation.clone()),
                    parent_id: None,
                    causal_inputs: None,
                    ts: base + time::Duration::seconds(offset),
                    kind,
                    payload,
                });
            };

        push(
            ADAPTER,
            0,
            EventKind::ComputationStarted,
            serde_json::json!({
                "recipe_id": "test-recipe",
                "recipe_hash": format!("blake3:{}", blake3_of("recipe")),
            }),
        );
        for (i, (producer, kind, payload)) in self.body.iter().enumerate() {
            push(
                producer,
                i64::try_from(i).expect("small") + 1,
                *kind,
                payload.clone(),
            );
        }
        let offset = i64::try_from(self.body.len()).expect("small") + 2;
        push(
            ADAPTER,
            offset,
            EventKind::FsWrite,
            serde_json::json!({
                "path": "/repo/out.md",
                "size": 64,
                "mode": "truncate",
                "hash": self.artifact.0,
            }),
        );
        push(
            ADAPTER,
            offset + 1,
            EventKind::ArtifactProduced,
            serde_json::json!({
                "artifact_id": self.artifact.0,
                "path": "/repo/out.md",
                "content_hash": self.artifact.0,
                "kind": "text/markdown",
                "size": 64,
                "produced_by": self.computation,
                "comparator": "exact",
            }),
        );
        push(
            ADAPTER,
            offset + 2,
            EventKind::ComputationEnded,
            serde_json::json!({ "status": "ok" }),
        );
        events.extend(self.extra.iter().cloned());
        events
    }

    fn coverage(&self) -> CoverageRegistry {
        let mut registry = CoverageRegistry::in_memory();
        let emits: Vec<&str> = self.adapter_emits.iter().map(String::as_str).collect();
        registry
            .register_at(
                manifest(ADAPTER, ProducerRole::Adapter, &emits),
                Self::base_ts(),
            )
            .expect("register adapter");
        if self.observer {
            registry
                .register_at(
                    manifest(OBSERVER, ProducerRole::Observer, OBSERVER_EMITS),
                    Self::base_ts(),
                )
                .expect("register observer");
        }
        for producer in &self.probe_producers {
            registry
                .register_at(
                    manifest(producer, ProducerRole::Probe, PROBE_EMITS),
                    Self::base_ts(),
                )
                .expect("register probe producer");
        }
        assert!(
            self.unregistered || !registry.is_empty(),
            "a fixture always registers at least the adapter"
        );
        registry
    }

    /// Build the engine, with the clock frozen shortly after the
    /// computation ended.
    pub fn engine(self, probes: ProbeRegistry) -> TestEngine {
        self.engine_with(probes, |b| b)
    }

    /// As [`Fixture::engine`], letting the test adjust the builder — for
    /// the tests that need a non-default `max_volatile_ttl`.
    pub fn engine_with(
        self,
        probes: ProbeRegistry,
        tune: impl FnOnce(crate::EngineBuilder) -> crate::EngineBuilder,
    ) -> TestEngine {
        let clock = Arc::new(FrozenClock::new(
            Self::base_ts() + time::Duration::seconds(300),
        ));
        let engine = tune(
            Engine::builder()
                .events(self.events())
                .coverage(self.coverage())
                .probes(probes)
                .clock(clock.clone()),
        )
        .build();
        TestEngine {
            engine,
            clock,
            artifact: self.artifact,
            computation: ComputationId(self.computation),
        }
    }
}

pub struct TestEngine {
    pub engine: Engine,
    pub clock: Arc<FrozenClock>,
    pub artifact: ArtifactId,
    #[allow(dead_code)]
    pub computation: ComputationId,
}

impl TestEngine {
    /// Check, asserting the engine did not refuse to emit.
    pub fn check_ok(&self) -> CheckOutcome {
        match self.engine.check(&self.artifact) {
            Ok(outcome) => outcome,
            Err(e) => panic!("engine refused to emit a certificate: {e}"),
        }
    }
}
