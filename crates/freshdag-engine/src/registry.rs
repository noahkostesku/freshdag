//! Probe registration and arbitration.
//!
//! Implements `docs/contracts/probe-contract.md §Probe Arbitration`:
//! probes register with `(scheme, host_pattern, priority)` and the
//! highest-priority match wins. **A tie is a contract violation that
//! fails loudly** — it is never resolved by picking either probe.
//!
//! Every way selection can fail maps to
//! [`ReasonCode::NoProbeAvailable`](freshdag_core::dependency::ReasonCode::NoProbeAvailable),
//! never to `ProbeUnknown`. `ProbeUnknown` asserts that a probe ran and
//! could not decide; saying that when nothing ran is a false statement
//! to a user reading `freshdag why`.

use std::collections::BTreeMap;
use std::sync::Arc;

use freshdag_core::probe::Probe;
use thiserror::Error;

/// Stable identity of a registration, derived from the triple probes
/// arbitrate on.
///
/// Two registrations with the same triple are the tie the contract
/// forbids, so among successfully registered probes the derived
/// identity is unique. Anti-thrash state
/// ([`TrustLedger`](crate::antithrash::TrustLedger)) is keyed by
/// `(dependency_key, probe_identity)`, and this is that identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProbeIdentity(String);

impl ProbeIdentity {
    /// Derive the identity of a `(scheme, host_pattern, priority)`
    /// triple. Wire form: `scheme[@host_pattern]#priority`.
    #[must_use]
    pub fn derive(scheme: &str, host_pattern: Option<&str>, priority: u32) -> Self {
        match host_pattern {
            Some(host) => Self(format!("{scheme}@{host}#{priority}")),
            None => Self(format!("{scheme}#{priority}")),
        }
    }

    /// The identity's wire string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProbeIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One registered probe.
struct Registration {
    identity: ProbeIdentity,
    scheme: String,
    host_pattern: Option<String>,
    priority: u32,
    probe: Arc<dyn Probe>,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("identity", &self.identity)
            .field("scheme", &self.scheme)
            .field("host_pattern", &self.host_pattern)
            .field("priority", &self.priority)
            .finish_non_exhaustive()
    }
}

/// A probe selected for one dependency key.
#[derive(Clone)]
pub struct Selected {
    /// Which registration won arbitration.
    pub identity: ProbeIdentity,
    /// The probe itself.
    pub probe: Arc<dyn Probe>,
}

impl std::fmt::Debug for Selected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Selected")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// Why no probe could be selected.
///
/// Every variant surfaces as
/// [`ReasonCode::NoProbeAvailable`](freshdag_core::dependency::ReasonCode::NoProbeAvailable)
/// on the certificate; the variant only supplies the non-normative
/// `detail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoProbe {
    /// Nothing is registered for this scheme. In Wave 2 this is the
    /// common case for `attio://`, `mcp://` and `web.search`.
    UnregisteredScheme {
        /// The scheme with no registration.
        scheme: String,
    },
    /// Probes exist for the scheme but none matches the key's host.
    NoHostMatch {
        /// The scheme.
        scheme: String,
    },
    /// Two or more probes matched at the same, highest priority. The
    /// contract calls this a violation that must fail loudly.
    PriorityTie {
        /// The scheme.
        scheme: String,
        /// The contested priority.
        priority: u32,
        /// The tied registrations, in identity order.
        candidates: Vec<ProbeIdentity>,
    },
    /// The probe that previously answered for this dependency is no
    /// longer registered (probe-contract §Anti-thrash Protocol, "Probe
    /// removal"). The engine does NOT fall through to another probe for
    /// the same scheme.
    ProbeRemoved {
        /// The identity that recorded the previous observation.
        previous: ProbeIdentity,
    },
}

impl NoProbe {
    /// Deterministic, secret-free `detail` text for the certificate.
    ///
    /// Certificate-contract §The `detail` field: this string lands in
    /// the `cert_id` preimage, so it carries no timing, no addresses,
    /// and nothing that varies run to run.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::UnregisteredScheme { scheme } => {
                format!("no-probe-registered-for-scheme={scheme}")
            }
            Self::NoHostMatch { scheme } => format!("no-host-pattern-match-for-scheme={scheme}"),
            Self::PriorityTie {
                scheme,
                priority,
                candidates,
            } => {
                let names: Vec<&str> = candidates.iter().map(ProbeIdentity::as_str).collect();
                format!(
                    "arbitration-tie scheme={scheme} priority={priority} candidates={}",
                    names.join(",")
                )
            }
            Self::ProbeRemoved { previous } => format!("probe-removed={previous}"),
        }
    }
}

/// Registration was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistrationError {
    /// Another probe already holds this exact
    /// `(scheme, host_pattern, priority)` triple. Accepting it would
    /// guarantee an arbitration tie later, so it is refused here, at
    /// the earliest point a human can act on it.
    #[error("probe registration tie: `{identity}` is already registered")]
    Tie {
        /// The contested identity.
        identity: ProbeIdentity,
    },
}

/// The set of probes the engine may dispatch to.
#[derive(Debug, Default)]
pub struct ProbeRegistry {
    registrations: BTreeMap<ProbeIdentity, Registration>,
}

impl ProbeRegistry {
    /// An empty registry. Every dependency check against it yields
    /// [`NoProbe::UnregisteredScheme`] — i.e. `Unknown`, never `Valid`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a probe under its declared
    /// `(scheme, host_pattern, priority)`.
    ///
    /// # Errors
    ///
    /// [`RegistrationError::Tie`] if the triple is already registered.
    /// The registry is left unchanged: a failed registration means the
    /// scheme keeps whatever coverage it already had, and the caller
    /// gets a loud error rather than a coin flip at check time.
    pub fn register(&mut self, probe: Arc<dyn Probe>) -> Result<ProbeIdentity, RegistrationError> {
        let identity =
            ProbeIdentity::derive(probe.scheme(), probe.host_pattern(), probe.priority());
        if self.registrations.contains_key(&identity) {
            return Err(RegistrationError::Tie { identity });
        }
        let registration = Registration {
            identity: identity.clone(),
            scheme: probe.scheme().to_string(),
            host_pattern: probe.host_pattern().map(ToString::to_string),
            priority: probe.priority(),
            probe,
        };
        self.registrations.insert(identity.clone(), registration);
        Ok(identity)
    }

    /// Remove a registration, modelling probe uninstall.
    ///
    /// Returns whether anything was removed. Dependencies previously
    /// answered by this identity become [`NoProbe::ProbeRemoved`] on
    /// their next check — never a silent fall-through to a lower-trust
    /// probe for the same scheme.
    pub fn deregister(&mut self, identity: &ProbeIdentity) -> bool {
        self.registrations.remove(identity).is_some()
    }

    /// Is this identity currently registered?
    #[must_use]
    pub fn contains(&self, identity: &ProbeIdentity) -> bool {
        self.registrations.contains_key(identity)
    }

    /// Number of registered probes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.registrations.len()
    }

    /// Is the registry empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }

    /// Every registered identity, in identity order.
    pub fn identities(&self) -> impl Iterator<Item = &ProbeIdentity> {
        self.registrations.keys()
    }

    /// Select the probe that answers for `key` under `scheme`.
    ///
    /// # Errors
    ///
    /// A [`NoProbe`] describing why arbitration produced nothing. Every
    /// variant means the edge verdict is `Unknown` with
    /// `ReasonCode::NoProbeAvailable`.
    pub fn select(&self, scheme: &str, key: &str) -> Result<Selected, NoProbe> {
        let for_scheme: Vec<&Registration> = self
            .registrations
            .values()
            .filter(|r| r.scheme == scheme)
            .collect();
        if for_scheme.is_empty() {
            return Err(NoProbe::UnregisteredScheme {
                scheme: scheme.to_string(),
            });
        }

        let host = host_of(key);
        let matching: Vec<&Registration> = for_scheme
            .into_iter()
            .filter(|r| host_pattern_matches(r.host_pattern.as_deref(), host))
            .collect();
        if matching.is_empty() {
            return Err(NoProbe::NoHostMatch {
                scheme: scheme.to_string(),
            });
        }

        let top = matching
            .iter()
            .map(|r| r.priority)
            .max()
            .expect("non-empty by the check above");
        let mut winners: Vec<&Registration> =
            matching.into_iter().filter(|r| r.priority == top).collect();
        winners.sort_by(|a, b| a.identity.cmp(&b.identity));

        if winners.len() > 1 {
            return Err(NoProbe::PriorityTie {
                scheme: scheme.to_string(),
                priority: top,
                candidates: winners.iter().map(|r| r.identity.clone()).collect(),
            });
        }
        let winner = winners[0];
        Ok(Selected {
            identity: winner.identity.clone(),
            probe: Arc::clone(&winner.probe),
        })
    }
}

/// The host portion of a dependency key, if it has one.
///
/// `file:///abs/path` yields `None` (the authority is empty);
/// `https://acme.com/pricing` yields `Some("acme.com")`.
fn host_of(key: &str) -> Option<&str> {
    let (_, rest) = key.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// Does a registration's host pattern admit this host?
///
/// `None` means "any host, including keys with no host". `Some("*")`
/// means any host but requires the key to have one. `Some("*.suffix")`
/// matches proper subdomains of `suffix`, not `suffix` itself.
fn host_pattern_matches(pattern: Option<&str>, host: Option<&str>) -> bool {
    match (pattern, host) {
        (None, _) | (Some("*"), Some(_)) => true,
        (Some(_), None) => false,
        (Some(pat), Some(host)) => match pat.strip_prefix("*.") {
            Some(suffix) => host.len() > suffix.len() + 1 && host.ends_with(&format!(".{suffix}")),
            None => pat == host,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use freshdag_core::dependency::Fingerprint;
    use freshdag_core::probe::ProbeResult;

    use super::*;

    #[derive(Debug)]
    struct Stub {
        scheme: &'static str,
        host: Option<&'static str>,
        priority: u32,
    }

    impl Probe for Stub {
        fn scheme(&self) -> &'static str {
            self.scheme
        }
        fn host_pattern(&self) -> Option<&'static str> {
            self.host
        }
        fn priority(&self) -> u32 {
            self.priority
        }
        fn check(&self, _: &str, _: &Fingerprint, _: Option<Duration>) -> ProbeResult {
            ProbeResult::Unknown {
                reason: "stub".into(),
                retryable: false,
            }
        }
    }

    fn stub(scheme: &'static str, host: Option<&'static str>, priority: u32) -> Arc<dyn Probe> {
        Arc::new(Stub {
            scheme,
            host,
            priority,
        })
    }

    #[test]
    fn empty_registry_yields_unregistered_scheme() {
        let registry = ProbeRegistry::new();
        assert_eq!(
            registry
                .select("attio", "attio://company/acme")
                .expect_err("no probe"),
            NoProbe::UnregisteredScheme {
                scheme: "attio".into()
            }
        );
    }

    #[test]
    fn highest_priority_wins() {
        let mut registry = ProbeRegistry::new();
        registry.register(stub("https", None, 0)).expect("register");
        registry
            .register(stub("https", None, 10))
            .expect("register");
        let selected = registry
            .select("https", "https://acme.com/x")
            .expect("select");
        assert_eq!(selected.identity.as_str(), "https#10");
    }

    #[test]
    fn identical_triple_is_refused_at_registration() {
        let mut registry = ProbeRegistry::new();
        registry.register(stub("file", None, 0)).expect("register");
        let err = registry.register(stub("file", None, 0)).expect_err("tie");
        assert_eq!(
            err,
            RegistrationError::Tie {
                identity: ProbeIdentity::derive("file", None, 0)
            }
        );
        assert_eq!(registry.len(), 1, "a refused registration changes nothing");
    }

    #[test]
    fn equal_priority_across_host_patterns_is_a_loud_tie() {
        // Neither register() call can catch this: the triples differ.
        // The tie is real only for keys both patterns match.
        let mut registry = ProbeRegistry::new();
        registry
            .register(stub("https", Some("*"), 5))
            .expect("register");
        registry.register(stub("https", None, 5)).expect("register");
        let err = registry
            .select("https", "https://api.github.com/x")
            .expect_err("tie");
        match err {
            NoProbe::PriorityTie {
                priority,
                candidates,
                ..
            } => {
                assert_eq!(priority, 5);
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected a tie, got {other:?}"),
        }
    }

    #[test]
    fn tie_detail_is_deterministic() {
        let err = NoProbe::PriorityTie {
            scheme: "https".into(),
            priority: 5,
            candidates: vec![
                ProbeIdentity::derive("https", None, 5),
                ProbeIdentity::derive("https", Some("*"), 5),
            ],
        };
        assert_eq!(err.detail(), err.detail());
        assert_eq!(
            err.detail(),
            "arbitration-tie scheme=https priority=5 candidates=https#5,https@*#5"
        );
    }

    #[test]
    fn host_patterns_scope_selection() {
        let mut registry = ProbeRegistry::new();
        registry
            .register(stub("https", Some("*.github.com"), 10))
            .expect("register");
        registry.register(stub("https", None, 1)).expect("register");

        assert_eq!(
            registry
                .select("https", "https://api.github.com/x")
                .expect("select")
                .identity
                .as_str(),
            "https@*.github.com#10"
        );
        // `*.github.com` does not match the apex domain.
        assert_eq!(
            registry
                .select("https", "https://github.com/x")
                .expect("select")
                .identity
                .as_str(),
            "https#1"
        );
    }

    #[test]
    fn no_host_match_is_distinct_from_unregistered_scheme() {
        let mut registry = ProbeRegistry::new();
        registry
            .register(stub("https", Some("*.github.com"), 10))
            .expect("register");
        assert_eq!(
            registry
                .select("https", "https://acme.com/x")
                .expect_err("no host match"),
            NoProbe::NoHostMatch {
                scheme: "https".into()
            }
        );
    }

    #[test]
    fn deregistration_removes_the_probe() {
        let mut registry = ProbeRegistry::new();
        let id = registry.register(stub("file", None, 0)).expect("register");
        assert!(registry.contains(&id));
        assert!(registry.deregister(&id));
        assert!(!registry.contains(&id));
        assert!(registry.select("file", "file:///x").is_err());
    }

    #[test]
    fn host_of_handles_authority_less_keys() {
        assert_eq!(host_of("file:///repo/notes.md"), None);
        assert_eq!(host_of("https://acme.com/pricing"), Some("acme.com"));
        assert_eq!(host_of("attio://company/acme"), Some("company"));
        assert_eq!(host_of("web.search(\"x\")"), None);
    }
}
