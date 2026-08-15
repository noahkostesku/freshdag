//! Trust-class-tagged content hashes.
//!
//! Serialized wire form is `"<algo>:<hex-digest>"` (e.g. `"blake3:abc..."`).
//! Absence is represented as `Option<Hash>` = `None`, never as a `Hash`
//! with a null algo or empty digest — invariant #7 rules out "unknown as
//! valid," and that begins here.

use std::str::FromStr;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// A supported content-hash algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HashAlgo {
    /// BLAKE3, the default for FreshDAG artifacts.
    Blake3,
    /// SHA-256, provided for interoperability with existing content-addressed stores.
    Sha256,
}

impl HashAlgo {
    /// Wire-format prefix (before the `:`).
    #[must_use]
    pub const fn as_wire_str(self) -> &'static str {
        match self {
            Self::Blake3 => "blake3",
            Self::Sha256 => "sha256",
        }
    }

    /// Expected hex-digest length in characters (algo output-length * 2).
    #[must_use]
    pub const fn hex_len(self) -> usize {
        match self {
            Self::Blake3 | Self::Sha256 => 64,
        }
    }
}

impl core::fmt::Display for HashAlgo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_wire_str())
    }
}

impl FromStr for HashAlgo {
    type Err = HashParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "blake3" => Ok(Self::Blake3),
            "sha256" => Ok(Self::Sha256),
            other => Err(HashParseError::UnknownAlgo(other.to_string())),
        }
    }
}

/// A content hash of some observed dependency or artifact.
///
/// The `digest_hex` is lowercase hex of the algorithm's raw output. The
/// invariant `digest_hex.len() == algo.hex_len()` is enforced on
/// construction and on deserialization.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash {
    /// Which algorithm produced the digest.
    pub algo: HashAlgo,
    /// Lowercase hex of the raw digest bytes.
    pub digest_hex: String,
}

impl Hash {
    /// Construct a hash, validating the digest is lowercase hex of the
    /// expected length for the algorithm.
    ///
    /// # Errors
    ///
    /// Returns `HashParseError::BadDigest` if `digest_hex` is not lowercase
    /// hex or is not the expected length.
    pub fn new(algo: HashAlgo, digest_hex: impl Into<String>) -> Result<Self, HashParseError> {
        let digest_hex = digest_hex.into();
        Self::validate_digest(algo, &digest_hex)?;
        Ok(Self { algo, digest_hex })
    }

    fn validate_digest(algo: HashAlgo, digest_hex: &str) -> Result<(), HashParseError> {
        if digest_hex.len() != algo.hex_len() {
            return Err(HashParseError::BadDigest(format!(
                "expected {} hex chars for {}, got {}",
                algo.hex_len(),
                algo,
                digest_hex.len()
            )));
        }
        if !digest_hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(HashParseError::BadDigest(
                "digest must be lowercase hex".to_string(),
            ));
        }
        Ok(())
    }
}

impl core::fmt::Display for Hash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}:{}", self.algo, self.digest_hex)
    }
}

impl FromStr for Hash {
    type Err = HashParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (algo, digest) = s
            .split_once(':')
            .ok_or_else(|| HashParseError::MalformedTag(s.to_string()))?;
        let algo: HashAlgo = algo.parse()?;
        Self::new(algo, digest)
    }
}

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

/// Errors from parsing a `Hash` from its wire form.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum HashParseError {
    /// Wire form was missing the `algo:` prefix.
    #[error("hash string missing `algo:` prefix: {0}")]
    MalformedTag(String),
    /// Algorithm identifier was not recognized.
    #[error("unknown hash algorithm: {0}")]
    UnknownAlgo(String),
    /// Digest failed validation (length or character class).
    #[error("bad digest: {0}")]
    BadDigest(String),
}
