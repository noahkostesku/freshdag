//! `file://` probe — content-hashes local files with BLAKE3.
//!
//! Trust-class semantics (per probe-contract §Trust-class Semantics):
//! - `exact` recorded → verify by re-hashing the file. If unreadable
//!   or missing, return `Unknown` (never `Drift`, never `Match`).
//! - `versioned` recorded → no version signal is available from a
//!   local file, so this probe cannot verify versioned. Returns
//!   `Unknown { retryable: false }` with a diagnostic reason.
//! - `heuristic` recorded → currently unsupported; a follow-up may
//!   add mtime/size-based checks. Returns `Unknown { retryable: false }`.
//! - `volatile` recorded → not applicable to local files. Returns
//!   `Unknown { retryable: false }`.
//!
//! **Missing / unreadable files never become a `Match`.** This is the
//! invariant #7 keystone for W5.1.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use freshdag_core::dependency::{Fingerprint, FingerprintKind, TrustClass};
use freshdag_core::ir::{Hash, HashAlgo};
use freshdag_core::probe::{Probe, ProbeResult};

/// Probe for the `file://` scheme.
///
/// The probe accepts a base directory for path resolution. Absolute
/// paths in the dependency key are used as-is; relative paths are
/// resolved against the base directory.
#[derive(Debug, Clone)]
pub struct FileProbe {
    base_dir: PathBuf,
}

impl FileProbe {
    /// Construct a probe rooted at `base_dir`. Absolute keys ignore
    /// the base; relative keys resolve against it.
    #[must_use]
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Construct a probe rooted at the current working directory.
    ///
    /// # Errors
    ///
    /// Returns `io::Error` if the current directory cannot be read.
    pub fn with_cwd() -> io::Result<Self> {
        Ok(Self {
            base_dir: std::env::current_dir()?,
        })
    }

    /// Resolve a `file://<path>` key to a local absolute `PathBuf`.
    /// If the key lacks the `file://` scheme, it is treated as a
    /// bare path.
    fn resolve(&self, key: &str) -> PathBuf {
        let stripped = key.strip_prefix("file://").unwrap_or(key);
        let p = Path::new(stripped);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.base_dir.join(p)
        }
    }

    /// Compute a BLAKE3 content hash of the file at `path`.
    ///
    /// # Errors
    ///
    /// Any I/O error (missing, permission denied, etc.) is propagated.
    pub fn hash_file(path: &Path) -> io::Result<Hash> {
        let mut file = File::open(path)?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let digest = hasher.finalize();
        // We know this passes validation because the digest is exactly
        // 32 bytes (64 hex chars, lowercase).
        Ok(Hash {
            algo: HashAlgo::Blake3,
            digest_hex: digest.to_hex().to_string(),
        })
    }
}

impl Probe for FileProbe {
    fn scheme(&self) -> &'static str {
        "file"
    }

    fn check(
        &self,
        key: &str,
        recorded_fp: &Fingerprint,
        _ttl_hint: Option<Duration>,
    ) -> ProbeResult {
        // Only `ContentHash` fingerprints are meaningful for a local
        // file. Any other kind is an unsupported combination and MUST
        // return Unknown per probe-contract §Trust-class Semantics.
        if !matches!(recorded_fp.kind, FingerprintKind::ContentHash) {
            return ProbeResult::Unknown {
                reason: format!(
                    "file:// probe cannot verify fingerprint kind {:?}",
                    recorded_fp.kind
                ),
                retryable: false,
            };
        }

        let path = self.resolve(key);

        // Missing / unreadable / permission-denied files MUST return
        // Unknown, never Drift, never Match. This is the invariant #7
        // keystone for the file probe.
        let observed_hash = match Self::hash_file(&path) {
            Ok(h) => h,
            Err(err) => {
                let retryable = matches!(
                    err.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::TimedOut
                );
                return ProbeResult::Unknown {
                    reason: format!("could not read {}: {err}", path.display()),
                    retryable,
                };
            }
        };

        // Parse the recorded content hash from its wire form (payload
        // is "algo:digest" for ContentHash).
        let recorded_hash: Hash = match recorded_fp.payload.parse() {
            Ok(h) => h,
            Err(err) => {
                return ProbeResult::Unknown {
                    reason: format!("recorded fingerprint is malformed: {err}"),
                    retryable: false,
                };
            }
        };

        let observed_fp = Fingerprint::new(FingerprintKind::ContentHash, observed_hash.to_string());

        if observed_hash == recorded_hash {
            ProbeResult::Match {
                observed_fp,
                observed_trust_class: TrustClass::Exact,
            }
        } else {
            ProbeResult::Drift {
                observed_fp,
                observed_trust_class: TrustClass::Exact,
            }
        }
    }
}
