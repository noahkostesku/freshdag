//! Tests for the file probe. Fixture-backed via `tempfile`.

use std::fs;
use std::io::Write;
use std::time::Duration;

use tempfile::tempdir;

use freshdag_core::dependency::{Fingerprint, FingerprintKind};
use freshdag_core::probe::{Probe, ProbeResult};

use crate::FileProbe;

fn write_file(dir: &std::path::Path, name: &str, contents: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = fs::File::create(&path).unwrap();
    f.write_all(contents).unwrap();
    path
}

fn recorded_hash(path: &std::path::Path) -> Fingerprint {
    let h = FileProbe::hash_file(path).unwrap();
    Fingerprint::new(FingerprintKind::ContentHash, h.to_string())
}

// --------------------------------------------------------------------
// Match path — file unchanged
// --------------------------------------------------------------------

#[test]
fn file_unchanged_returns_match() {
    let dir = tempdir().unwrap();
    let path = write_file(dir.path(), "ICP.md", b"hello world\n");
    let fp = recorded_hash(&path);

    let probe = FileProbe::new(dir.path());
    let result = probe.check(&format!("file://{}", path.display()), &fp, None);
    match result {
        ProbeResult::Match {
            observed_trust_class,
            ..
        } => {
            assert_eq!(
                observed_trust_class,
                freshdag_core::dependency::TrustClass::Exact
            );
        }
        other => panic!("expected Match, got {other:?}"),
    }
}

// --------------------------------------------------------------------
// Drift path — file content changed
// --------------------------------------------------------------------

#[test]
fn file_changed_returns_drift() {
    let dir = tempdir().unwrap();
    let path = write_file(dir.path(), "ICP.md", b"hello world\n");
    let fp = recorded_hash(&path);
    // Overwrite with different bytes.
    write_file(dir.path(), "ICP.md", b"hello NEW world\n");

    let probe = FileProbe::new(dir.path());
    let result = probe.check(&format!("file://{}", path.display()), &fp, None);
    assert!(matches!(result, ProbeResult::Drift { .. }));
}

// --------------------------------------------------------------------
// Unknown paths — missing, permission, unsupported kinds
// (invariant #7 keystone for file probe)
// --------------------------------------------------------------------

#[test]
fn missing_file_returns_unknown_not_match_not_drift() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("does-not-exist.md");
    // Fabricate a recorded fingerprint (would be a valid one from a
    // prior observation).
    let fp = Fingerprint::new(
        FingerprintKind::ContentHash,
        format!("blake3:{}", "a".repeat(64)),
    );

    let probe = FileProbe::new(dir.path());
    let result = probe.check(&format!("file://{}", missing.display()), &fp, None);
    match result {
        ProbeResult::Unknown { reason, .. } => {
            assert!(
                reason.contains("could not read"),
                "reason should describe the read failure; got: {reason}"
            );
        }
        other => panic!("invariant #7: missing file MUST return Unknown, not {other:?}"),
    }
}

#[test]
fn missing_file_never_returns_match_even_when_recorded_hash_matches_empty() {
    // Adversarial: even if the recorded fingerprint happens to be the
    // BLAKE3 of empty bytes, a missing file must not silently succeed.
    // (This is exactly the "unknown looks like fresh" failure mode the
    // adversarial reviewers named.)
    let dir = tempdir().unwrap();
    let missing = dir.path().join("nowhere.md");
    // BLAKE3 of empty input.
    let empty_hash = FileProbe::hash_file(&write_file(dir.path(), "empty", b"")).unwrap();
    let fp = Fingerprint::new(FingerprintKind::ContentHash, empty_hash.to_string());

    let probe = FileProbe::new(dir.path());
    let result = probe.check(&format!("file://{}", missing.display()), &fp, None);
    assert!(
        matches!(result, ProbeResult::Unknown { .. }),
        "missing file MUST return Unknown regardless of recorded hash; got {result:?}"
    );
}

#[test]
fn versioned_fingerprint_on_local_file_returns_unknown() {
    let dir = tempdir().unwrap();
    let path = write_file(dir.path(), "x.md", b"hi");
    let fp = Fingerprint::new(FingerprintKind::Version, "42");

    let probe = FileProbe::new(dir.path());
    let result = probe.check(&format!("file://{}", path.display()), &fp, None);
    match result {
        ProbeResult::Unknown { retryable, .. } => {
            assert!(!retryable, "unsupported fingerprint kind is not retryable");
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn etag_fingerprint_on_local_file_returns_unknown() {
    let dir = tempdir().unwrap();
    let path = write_file(dir.path(), "x.md", b"hi");
    let fp = Fingerprint::new(FingerprintKind::Etag, "\"abc\"");

    let probe = FileProbe::new(dir.path());
    let result = probe.check(&format!("file://{}", path.display()), &fp, None);
    assert!(matches!(result, ProbeResult::Unknown { .. }));
}

// --------------------------------------------------------------------
// Path resolution
// --------------------------------------------------------------------

#[test]
fn relative_paths_resolve_against_base_dir() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "ICP.md", b"contents\n");
    let path = dir.path().join("ICP.md");
    let fp = recorded_hash(&path);

    let probe = FileProbe::new(dir.path());
    let result = probe.check("file://ICP.md", &fp, None);
    assert!(matches!(result, ProbeResult::Match { .. }));
}

#[test]
fn key_without_file_scheme_still_resolves() {
    let dir = tempdir().unwrap();
    write_file(dir.path(), "ICP.md", b"contents\n");
    let path = dir.path().join("ICP.md");
    let fp = recorded_hash(&path);

    let probe = FileProbe::new(dir.path());
    let result = probe.check(&path.to_string_lossy(), &fp, None);
    assert!(matches!(result, ProbeResult::Match { .. }));
}

// --------------------------------------------------------------------
// Determinism
// --------------------------------------------------------------------

#[test]
fn hash_is_deterministic_across_runs() {
    let dir = tempdir().unwrap();
    let path = write_file(dir.path(), "x.md", b"deterministic bytes\n");
    let h1 = FileProbe::hash_file(&path).unwrap();
    let h2 = FileProbe::hash_file(&path).unwrap();
    assert_eq!(h1, h2);
}

// --------------------------------------------------------------------
// Scheme / registration
// --------------------------------------------------------------------

#[test]
fn probe_declares_file_scheme() {
    let probe = FileProbe::new(std::env::temp_dir());
    assert_eq!(probe.scheme(), "file");
    assert!(probe.host_pattern().is_none());
    assert_eq!(probe.priority(), 0);
}

// --------------------------------------------------------------------
// ttl_hint is accepted but doesn't affect file semantics
// --------------------------------------------------------------------

#[test]
fn ttl_hint_is_ignored_for_files() {
    let dir = tempdir().unwrap();
    let path = write_file(dir.path(), "x.md", b"hi");
    let fp = recorded_hash(&path);

    let probe = FileProbe::new(dir.path());
    let result = probe.check(
        &format!("file://{}", path.display()),
        &fp,
        Some(Duration::from_secs(3600)),
    );
    assert!(matches!(result, ProbeResult::Match { .. }));
}
