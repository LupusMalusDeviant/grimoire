//! Identifiers and small value types shared by the pack format and [`crate::AssetSource`]
//! (contract §12).

use std::fmt;

use grimoire_core::{StableHash, StableHasher};
use sha2::{Digest, Sha256 as Sha256Hasher};

use crate::MAX_PATH_LEN;
use crate::error::AssetError;

/// Domain separator fed into [`AssetId::from_path`] before the path itself, so an asset id never
/// collides with a stable hash computed for an unrelated purpose (contract §12).
const ASSET_ID_DOMAIN: &str = "grimoire.asset-id.v1";

/// Stable 64-bit identifier of one asset, derived from its [`AssetPath`].
///
/// Two equal [`AssetPath`]s always yield the same id, and the id is stable across platforms,
/// Rust releases and file systems: it depends only on the path's bytes (contract §12).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AssetId(pub u64);

impl AssetId {
    /// Derives the id of `path`.
    ///
    /// Algorithm (frozen by a golden test, contract §12): a fresh `StableHasher` (algorithm
    /// version 1) fed with the domain separator `"grimoire.asset-id.v1"` and then the path
    /// string, finished with [`StableHasher::finish`].
    #[must_use]
    pub fn from_path(path: &AssetPath) -> AssetId {
        let mut hasher = StableHasher::new();
        hasher.write_str(ASSET_ID_DOMAIN);
        hasher.write_str(path.as_str());
        AssetId(hasher.finish())
    }
}

impl fmt::Display for AssetId {
    /// Sixteen lowercase hex digits without a prefix, matching the engine's JSON `u64` convention
    /// (contract §2 rule 11).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl StableHash for AssetId {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.0);
    }
}

/// Validated, normalised relative path of one asset inside a pack or [`crate::MemorySource`].
///
/// Only ASCII `[a-z0-9_.-]` and `/` as a path separator are allowed, so the id derived from a
/// path never depends on file system case sensitivity, Unicode normalisation, or the platform's
/// path separator (contract §12). Construct with [`AssetPath::new`]; there is no way to obtain an
/// invalid instance.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AssetPath(String);

impl AssetPath {
    /// Validates and wraps `path`.
    ///
    /// # Errors
    /// Returns [`AssetError::InvalidPath`] if `path` is empty, longer than [`MAX_PATH_LEN`]
    /// bytes, starts or ends with `/`, contains an empty segment or a `.`/`..` segment, or
    /// contains a byte outside ASCII `[a-z0-9_.-]` and `/` (this includes backslashes, uppercase
    /// letters and any non-ASCII byte, per contract §12).
    pub fn new(path: &str) -> Result<AssetPath, AssetError> {
        let invalid = |reason: &'static str| AssetError::InvalidPath {
            path: path.to_owned(),
            reason,
        };
        if path.is_empty() {
            return Err(invalid("path is empty"));
        }
        if path.len() > MAX_PATH_LEN {
            return Err(invalid("path exceeds MAX_PATH_LEN bytes"));
        }
        if path.starts_with('/') || path.ends_with('/') {
            return Err(invalid("path has a leading or trailing '/'"));
        }
        for segment in path.split('/') {
            if segment.is_empty() {
                return Err(invalid("path has an empty segment"));
            }
            if segment == "." || segment == ".." {
                return Err(invalid("path has a '.' or '..' segment"));
            }
        }
        let is_allowed_byte = |byte: u8| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'.' | b'-' | b'/')
        };
        if !path.bytes().all(is_allowed_byte) {
            return Err(invalid(
                "path contains a byte outside ASCII [a-z0-9_.-] and '/'",
            ));
        }
        Ok(AssetPath(path.to_owned()))
    }

    /// Returns the validated path string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Kind tag of an asset's payload, interpreted by the consumer that decodes it.
///
/// `0` is invalid, `1` ([`AssetKind::SIGIL`]) is the only kind a v1 pack reader accepts among the
/// engine-reserved range, `2..=5` are reserved for future engine kinds a v1 reader rejects, and
/// `0x8000..=0xFFFF` are passed through opaquely for application-defined content (contract §12).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AssetKind(pub u16);

impl AssetKind {
    /// A `SigilUnit` (§11.1); the only non-application kind a v1 pack accepts.
    pub const SIGIL: Self = Self(1);
    /// Reserved for a future mesh kind; a v1 pack reader rejects it (`ReservedKind`).
    pub const MESH: Self = Self(2);
    /// Reserved for a future material kind; a v1 pack reader rejects it (`ReservedKind`).
    pub const MATERIAL: Self = Self(3);
    /// Reserved for a future audio kind; a v1 pack reader rejects it (`ReservedKind`).
    pub const AUDIO: Self = Self(4);
    /// Reserved for a future template kind; a v1 pack reader rejects it (`ReservedKind`).
    pub const TEMPLATE: Self = Self(5);
}

/// A 32-byte SHA-256 digest of one asset's payload bytes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256(pub [u8; 32]);

impl fmt::Debug for Sha256 {
    /// Lower-case hex, e.g. `Sha256(e3b0c4...)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sha256(")?;
        write_hex(f, &self.0)?;
        f.write_str(")")
    }
}

/// SHA-256 over the entries of an [`crate::AssetSource`] (not over path or manifest bytes); see
/// [`crate::AssetSource::content_hash`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(pub [u8; 32]);

impl fmt::Debug for ContentHash {
    /// Lower-case hex, e.g. `ContentHash(e3b0c4...)`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ContentHash(")?;
        write_hex(f, &self.0)?;
        f.write_str(")")
    }
}

fn write_hex(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for byte in bytes {
        write!(f, "{byte:02x}")?;
    }
    Ok(())
}

/// One entry of an [`crate::AssetSource`]'s directory: identity, kind and content digest, but not
/// where or how the bytes are stored.
///
/// `#[non_exhaustive]` and constructed through [`AssetEntry::new`] (contract §2 rule 13), because
/// `AssetSource` implementations outside this crate must be able to build their own entries.
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AssetEntry {
    /// Identity of the asset, matching [`AssetId::from_path`] of its source path.
    pub id: AssetId,
    /// Kind tag interpreted by the consumer that decodes the payload.
    pub kind: AssetKind,
    /// Format version of the payload, meaningful only to the kind's own decoder.
    pub kind_version: u32,
    /// Payload length in bytes.
    pub len: u64,
    /// SHA-256 digest of the payload bytes.
    pub sha256: Sha256,
}

impl AssetEntry {
    /// Builds an entry. For [`crate::AssetSource`] implementations outside this crate (§2 rule
    /// 13, §12).
    #[must_use]
    pub fn new(id: AssetId, kind: AssetKind, kind_version: u32, len: u64, sha256: Sha256) -> Self {
        Self {
            id,
            kind,
            kind_version,
            len,
            sha256,
        }
    }
}

/// Computes the SHA-256 digest of `bytes`.
pub(crate) fn sha256_of(bytes: &[u8]) -> Sha256 {
    let mut hasher = Sha256Hasher::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    // `Sha256Hasher::finalize` always returns exactly 32 bytes for the SHA-256 algorithm.
    out.copy_from_slice(&digest);
    Sha256(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_sixteen_lowercase_hex_digits() {
        assert_eq!(AssetId(0).to_string(), "0000000000000000");
        assert_eq!(AssetId(0xdead_beef).to_string(), "00000000deadbeef");
        assert_eq!(AssetId(u64::MAX).to_string(), "ffffffffffffffff");
    }

    #[test]
    fn valid_paths_are_accepted() {
        for path in ["a", "a/b", "a.b-c_d/e.f", "0/1/2"] {
            assert!(AssetPath::new(path).is_ok(), "{path} should be valid");
        }
    }

    #[test]
    fn from_path_is_deterministic_and_domain_separated() {
        let path = AssetPath::new("a/b.c").unwrap();
        assert_eq!(AssetId::from_path(&path), AssetId::from_path(&path));
        let other = AssetPath::new("a/b.d").unwrap();
        assert_ne!(AssetId::from_path(&path), AssetId::from_path(&other));
    }

    #[test]
    fn sha256_debug_is_lowercase_hex() {
        let mut bytes = [0u8; 32];
        bytes[0] = 0xab;
        bytes[31] = 0xcd;
        let hash = Sha256(bytes);
        let expected = format!("Sha256(ab{}cd)", "00".repeat(30));
        assert_eq!(format!("{hash:?}"), expected);
    }
}
