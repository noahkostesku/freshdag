//! Where `fs.read` fingerprints come from.
//!
//! # The problem this solves
//!
//! Claude Code's `PreToolUse` payload for `Read` carries a path and
//! nothing else — no content, no size. So the compiled `fs.read` had
//! `hash: None`, and `freshdag_store::graph` drops a read with no
//! fingerprint as [`ExclusionReason::NoFingerprint`]. Every dependency
//! the adapter produced was discarded before it reached the graph, and
//! every artifact came back `no-dependencies-observed`. The loop ran
//! and decided nothing.
//!
//! The bytes are available — they are on disk, at the path the tool is
//! about to read. Reading them is I/O, which is why this is an injected
//! capability rather than a direct call: the compile path must stay
//! filesystem-free under the conformance harness, or goldens would
//! depend on the machine that ran them and `.claude/rules/testing.md`
//! forbids exactly that.
//!
//! [`NoContent`] is the default and reads nothing, so
//! `Compiler::new` — the constructor the conformance harness uses —
//! behaves exactly as before. [`DiskContent`] is installed only by
//! `Compiler::production`.
//!
//! # Why hashing *before* the read is sound
//!
//! The fingerprint is taken at `PreToolUse`, so it describes the file
//! as it stood a moment before the tool ran. Two things can go wrong
//! and both fail safe:
//!
//! - **The tool call is denied or fails.** A dependency is recorded
//!   that was never actually read. That is over-approximation: a
//!   spurious dependency yields spurious staleness, which invariant #15
//!   explicitly prefers.
//! - **The file changes between this hook and the read.** The recorded
//!   fingerprint is of the earlier bytes, so a later check compares
//!   against those and reports drift. Again spurious staleness, never
//!   spurious freshness.
//!
//! What it must never do is block the runtime (adapter-contract
//! §Errors and Backpressure), so [`DiskContent`] refuses to hash
//! anything over its byte cap and every failure is silent absence
//! rather than an error — `hash: None` is exactly the state the adapter
//! was already in.

use std::path::Path;

use freshdag_core::ir::{Hash, HashAlgo};

/// Default ceiling on a file this adapter will hash inline, in bytes.
///
/// The hook runs inside a tool call. Hashing is fast, but reading an
/// arbitrarily large file is not, and a slow hook is a slow agent.
/// Files above the cap simply get no fingerprint.
pub const DEFAULT_MAX_HASH_BYTES: u64 = 8 * 1024 * 1024;

/// Supplies the fingerprint of a file the runtime is about to read.
pub trait ContentSource: core::fmt::Debug + Send + Sync {
    /// `(hash, size)` for `path`, or `None` when it cannot be taken —
    /// missing file, unreadable, a directory, or over the size cap.
    ///
    /// `None` must never be an error path. It means "this adapter has
    /// no fingerprint for that read", which is a coverage fact the
    /// store already knows how to interpret.
    fn fingerprint(&self, path: &Path) -> Option<(Hash, u64)>;
}

/// Reads nothing. The default, and what the conformance harness uses.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoContent;

impl ContentSource for NoContent {
    fn fingerprint(&self, _path: &Path) -> Option<(Hash, u64)> {
        None
    }
}

/// Hashes the real file, up to a byte cap.
#[derive(Debug, Clone, Copy)]
pub struct DiskContent {
    max_bytes: u64,
}

impl DiskContent {
    /// Construct with an explicit cap.
    #[must_use]
    pub fn new(max_bytes: u64) -> Self {
        Self { max_bytes }
    }
}

impl Default for DiskContent {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_HASH_BYTES)
    }
}

impl ContentSource for DiskContent {
    fn fingerprint(&self, path: &Path) -> Option<(Hash, u64)> {
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() > self.max_bytes {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        // `metadata` and `read` are two syscalls apart, so trust the
        // bytes actually hashed rather than the earlier stat.
        let size = bytes.len() as u64;
        if size > self.max_bytes {
            return None;
        }
        let digest = blake3::hash(&bytes).to_hex().to_string();
        let hash = Hash::new(HashAlgo::Blake3, digest).ok()?;
        Some((hash, size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_content_never_fingerprints() {
        assert!(NoContent.fingerprint(Path::new("/etc/hosts")).is_none());
    }

    #[test]
    fn disk_content_hashes_a_real_file() {
        let dir = std::env::temp_dir().join("freshdag-content-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("sample.txt");
        std::fs::write(&path, b"hello\n").expect("write");

        let (hash, size) = DiskContent::default()
            .fingerprint(&path)
            .expect("a readable file fingerprints");
        assert_eq!(size, 6);
        assert_eq!(
            hash.to_string(),
            format!("blake3:{}", blake3::hash(b"hello\n").to_hex())
        );
    }

    #[test]
    fn a_missing_file_is_absence_not_an_error() {
        assert!(DiskContent::default()
            .fingerprint(Path::new("/nonexistent/freshdag/never"))
            .is_none());
    }

    /// The cap exists so a hook never stalls a tool call on a huge
    /// file. Exceeding it must degrade to "no fingerprint", which is
    /// the state the adapter was already in.
    #[test]
    fn a_file_over_the_cap_is_not_hashed() {
        let dir = std::env::temp_dir().join("freshdag-content-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("big.txt");
        std::fs::write(&path, vec![b'x'; 4096]).expect("write");

        assert!(DiskContent::new(1024).fingerprint(&path).is_none());
        assert!(DiskContent::new(8192).fingerprint(&path).is_some());
    }

    #[test]
    fn a_directory_is_not_a_file() {
        let dir = std::env::temp_dir().join("freshdag-content-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert!(DiskContent::default().fingerprint(&dir).is_none());
    }
}
