//! Every JSON example in `docs/contracts/` is checked against the type
//! it illustrates.
//!
//! # Why this exists
//!
//! A contract example is the thing producer authors copy. When it drifts
//! from the type it claims to show, it does not fail loudly — it teaches
//! the wrong shape, and it gets cited as if it were fact. Both have
//! happened here, twice, and neither was caught by a test:
//!
//! - ADR 0011 read `observer-contract.md`'s example manifest as a
//!   description of `crates/freshdag-observer`, concluded "the fsatrace
//!   observer's two notes are `over-approximates`", and built its
//!   central argument on it. The shipped observer declares the opposite.
//!   The Amendment withdrew the conclusion and closed the ambiguity in
//!   Ruling 5.
//! - `execution-ir.md`'s event sketch named producers
//!   (`"adapter-claude"`, `"observer-fsatrace"`, `"probe-http"`) that no
//!   in-tree producer emits. `producer` is matched by exact string, so a
//!   reader copying it would have emitted events that fail attribution.
//! - `adapter-contract.md`'s example carried a `partial` entry for
//!   `net.fetch` while omitting that kind from `emits`, which declares
//!   nothing — `covers()` reads `emits` alone.
//!
//! Each was fixed by hand, one instance at a time. This closes the
//! class: an example that stops matching its type now fails the build.
//!
//! # What it cannot check
//!
//! That an example is *honest* — that it does not overclaim about a
//! shipped producer. Nothing mechanical catches that; Ruling 5's banners
//! and the `-example` naming convention are the defence, and they are
//! social. This test checks only that an example is *well-formed
//! against the type it illustrates*.
//!
//! # Registration
//!
//! Every fenced `json` code block in `docs/contracts/` must appear in
//! [`EXPECTATIONS`], or [`every_contract_example_is_registered`] fails.
//! That is deliberate: the failure mode this test exists to prevent is
//! an unchecked example, so a new one cannot be added silently.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use freshdag_core::ir::CoverageManifest;

/// How a given example block must hold up.
#[derive(Debug, Clone, Copy)]
enum Check {
    /// The block is complete: it must deserialize into the named type,
    /// values and all.
    Manifest,
    /// The block is a shape sketch with deliberately elided values
    /// (`"blake3:..."`), so it cannot be deserialized. Its top-level
    /// keys must still be keys a real certificate has — which catches a
    /// renamed or removed field, the drift that actually bites.
    CertificateKeys,
}

/// `(contract file, 0-based index of the json block, check)`.
///
/// Hand-enumerated on purpose, like `ALL_REASON_CODES`. A block with no
/// entry is a test failure, not a silent pass.
const EXPECTATIONS: &[(&str, usize, Check)] = &[
    ("adapter-contract.md", 0, Check::Manifest),
    ("observer-contract.md", 0, Check::Manifest),
    ("certificate-contract.md", 0, Check::CertificateKeys),
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn contracts_dir() -> PathBuf {
    repo_root().join("docs").join("contracts")
}

/// Every fenced `json` code block in `path`, in document order, with the
/// common leading indentation removed.
fn json_blocks(path: &Path) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut out = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        match &mut current {
            None => {
                if trimmed == "```json" {
                    current = Some(Vec::new());
                }
            }
            Some(body) => {
                if trimmed == "```" {
                    out.push(dedent(&body.join("\n")));
                    current = None;
                } else {
                    body.push(line);
                }
            }
        }
    }
    assert!(
        current.is_none(),
        "{}: an unterminated ```json fence",
        path.display()
    );
    out
}

/// Strip the smallest common leading-space prefix, so a block indented
/// inside a list item still parses.
fn dedent(block: &str) -> String {
    let indent = block
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    block
        .lines()
        .map(|l| if l.len() >= indent { &l[indent..] } else { l })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Top-level keys of every certificate in the conformance fixture set.
///
/// Ground truth rather than a hand-written list: those files are
/// deserialized as `Certificate` by
/// `tests/certificate_conformance.rs`, so anything they contain is a
/// field the type really has.
fn known_certificate_keys() -> BTreeSet<String> {
    let root = repo_root().join("fixtures").join("certificate-conformance");
    let mut keys = BTreeSet::new();
    let mut seen_any = false;
    for kind in ["positive", "illegal"] {
        let dir = root.join(kind);
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
            let cert = entry.expect("dir entry").path().join("certificate.json");
            if !cert.is_file() {
                continue;
            }
            let raw = fs::read_to_string(&cert).expect("read certificate");
            let value: serde_json::Value = serde_json::from_str(&raw).expect("fixture parses");
            if let Some(obj) = value.as_object() {
                seen_any = true;
                keys.extend(obj.keys().cloned());
            }
        }
    }
    assert!(seen_any, "no conformance certificates found to learn from");
    keys
}

#[test]
fn every_contract_example_matches_the_type_it_illustrates() {
    let known = known_certificate_keys();

    for (file, index, check) in EXPECTATIONS {
        let path = contracts_dir().join(file);
        let blocks = json_blocks(&path);
        let block = blocks.get(*index).unwrap_or_else(|| {
            panic!(
                "{file}: no ```json block at index {index} (found {})",
                blocks.len()
            )
        });

        match check {
            Check::Manifest => {
                let manifest: CoverageManifest = serde_json::from_str(block).unwrap_or_else(|e| {
                    panic!(
                        "{file}[{index}] no longer deserializes as a CoverageManifest: {e}\n\
                             An example that does not parse is teaching a shape the code \
                             will reject.\n--- block ---\n{block}"
                    )
                });

                // A `partial` entry for a kind the producer does not
                // emit declares nothing: `covers()` reads `emits`
                // alone. The adapter contract shipped exactly this.
                for pattern in manifest.partial.keys() {
                    let covered = manifest.emits.iter().any(|e| {
                        let e = e.as_str();
                        e == pattern.as_str()
                            || e.strip_suffix('*').is_some_and(|p| pattern.starts_with(p))
                            || pattern.strip_suffix('*').is_some_and(|p| e.starts_with(p))
                    });
                    assert!(
                        covered,
                        "{file}[{index}]: `partial` annotates `{pattern}`, which no entry \
                         in `emits` covers. That declaration is inert — the example is \
                         teaching a manifest whose admission does nothing."
                    );
                }
            }
            Check::CertificateKeys => {
                // Deliberately elided values (`"blake3:..."`) mean this
                // block cannot deserialize. Its KEYS still must be real.
                let keys: BTreeSet<String> = block
                    .lines()
                    .filter_map(|l| {
                        let l = l.trim_start();
                        // Top-level keys only: exactly two spaces of
                        // indent inside the object.
                        l.strip_prefix('"')
                            .and_then(|r| r.split_once('"'))
                            .map(|(k, _)| k.to_string())
                    })
                    .collect();
                let top: BTreeSet<String> = keys.intersection(&known).cloned().collect();
                assert!(
                    !top.is_empty(),
                    "{file}[{index}]: not one key matched a real certificate field; \
                     the example has drifted wholesale"
                );
                for required in ["cert_id", "schema", "artifact", "produced_by", "status"] {
                    assert!(
                        keys.contains(required),
                        "{file}[{index}]: the certificate example no longer shows \
                         `{required}`, which every real certificate carries"
                    );
                }
            }
        }
    }
}

/// No contract example may go unchecked.
///
/// The defect this whole file exists to prevent is an example nobody
/// verifies, so adding one without registering it fails here rather
/// than passing quietly.
#[test]
fn every_contract_example_is_registered() {
    let dir = contracts_dir();
    let mut unregistered = Vec::new();
    for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        for index in 0..json_blocks(&path).len() {
            if !EXPECTATIONS
                .iter()
                .any(|(f, i, _)| *f == name && *i == index)
            {
                unregistered.push(format!("{name}[{index}]"));
            }
        }
    }
    assert!(
        unregistered.is_empty(),
        "unregistered contract example(s): {unregistered:?}\n\
         Add each to EXPECTATIONS in this file. An example nobody checks \
         is the failure mode this test exists to prevent — it drifts, and \
         then it gets cited as fact (ADR 0011, Amendment, Ruling 5)."
    );
}

/// Guard the harness itself: a registration naming a file that does not
/// exist would make the check above vacuous.
#[test]
fn every_registration_names_a_real_contract() {
    for (file, _, _) in EXPECTATIONS {
        let path = contracts_dir().join(file);
        assert!(
            path.is_file(),
            "EXPECTATIONS names {file}, which does not exist at {}",
            path.display()
        );
    }
}
