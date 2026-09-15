//! Error types for asset sources and the pack v1 format (contract §12).

use std::io;

use crate::ids::{AssetId, AssetKind};

/// Error returned by [`crate::AssetSource`], [`crate::AssetStore`] and [`crate::AssetPath::new`].
///
/// `#[non_exhaustive]`: new variants are additive (contract §2 rule 13).
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AssetError {
    /// No entry with this id exists in the source.
    #[error("asset {0} not found")]
    NotFound(AssetId),
    /// `AssetPath::new` rejected a path.
    #[error("invalid asset path {path:?}: {reason}")]
    InvalidPath {
        /// The rejected input, verbatim.
        path: String,
        /// Human-readable reason, stable enough for logs but not meant to be matched on.
        reason: &'static str,
    },
    /// [`crate::AssetStore::load`] was called with an `expected` kind that does not match the
    /// entry's actual kind.
    #[error("kind mismatch for {id}: expected {expected:?}, found {found:?}")]
    KindMismatch {
        /// The requested asset.
        id: AssetId,
        /// The kind the caller expected.
        expected: AssetKind,
        /// The kind actually recorded for `id`.
        found: AssetKind,
    },
    /// The payload's SHA-256 digest does not match its directory entry.
    #[error("hash mismatch for asset {0}")]
    HashMismatch(AssetId),
    /// The caller-provided decode function failed.
    #[error("failed to decode asset {id}: {message}")]
    Decode {
        /// The asset whose payload failed to decode.
        id: AssetId,
        /// The decode error, rendered with `Display`.
        message: String,
    },
    /// An I/O error occurred while reading a pack file.
    #[error("I/O error reading {path}: {kind}")]
    Io {
        /// The path that was being read.
        path: String,
        /// The underlying [`io::ErrorKind`].
        kind: io::ErrorKind,
    },
    /// A pack file is larger than the caller's declared limit and was never fully loaded.
    #[error("{path} is larger than the {max}-byte limit")]
    TooLarge {
        /// The path that was rejected.
        path: String,
        /// The limit that was exceeded.
        max: u64,
    },
    /// A pack file failed to parse; see [`PackError`].
    #[error(transparent)]
    Pack(#[from] PackError),
}

/// Error returned while decoding a pack v1 byte stream (contract §12).
///
/// Every variant reports a structural problem in foreign bytes; a malformed pack always yields
/// one of these instead of panicking (contract §2 rule 9). `#[non_exhaustive]`: new variants are
/// additive (contract §2 rule 13).
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PackError {
    /// Fewer bytes remain than a field or block needs.
    #[error(
        "unexpected end of pack at offset {offset}: needed {needed} bytes, {available} available"
    )]
    UnexpectedEnd {
        /// Absolute byte offset the read started at.
        offset: u64,
        /// Bytes the read needed.
        needed: u64,
        /// Bytes actually remaining at `offset`.
        available: u64,
    },
    /// The header does not start with [`crate::PACK_MAGIC`].
    #[error("bad pack magic")]
    BadMagic,
    /// The header's format version is not one this reader understands.
    #[error("unsupported pack format version {0}")]
    UnsupportedVersion(u32),
    /// The header's declared header length is not 64.
    #[error("unexpected header length {0} (expected 64)")]
    HeaderLength(u32),
    /// The header's declared file length does not match the actual byte count.
    #[error("declared file length {declared} does not match actual length {actual}")]
    FileLength {
        /// Length the header declares.
        declared: u64,
        /// Length of the byte slice actually given to `from_bytes`.
        actual: u64,
    },
    /// A field required to be zero in pack v1 is not.
    #[error("non-zero reserved field at offset {offset}")]
    NonZeroReserved {
        /// Byte offset of the offending field.
        offset: u64,
    },
    /// The header's entry count exceeds [`crate::MAX_ENTRIES`].
    #[error("entry count {0} exceeds MAX_ENTRIES")]
    TooManyEntries(u32),
    /// A declared offset and length describe a region outside the file (or, for the manifest,
    /// outside the space left for it).
    #[error("{what} at offset {offset} (length {len}) is out of bounds")]
    OutOfBounds {
        /// What the offending region was (e.g. `"toc"`, `"payload"`, `"manifest"`).
        what: &'static str,
        /// Declared start offset.
        offset: u64,
        /// Declared length.
        len: u64,
    },
    /// A TOC entry's payload offset is not a multiple of [`crate::PACK_ALIGN`].
    #[error("TOC entry {index} has a misaligned payload offset")]
    Misaligned {
        /// Index of the offending TOC entry.
        index: u32,
    },
    /// A TOC entry's payload region overlaps the previous entry (or the TOC itself).
    #[error("TOC entry {index} overlaps the previous entry")]
    Overlap {
        /// Index of the offending TOC entry.
        index: u32,
    },
    /// TOC entries are not in strictly ascending [`AssetId`] order.
    #[error("TOC entry {index} is not in strictly ascending id order")]
    UnsortedIds {
        /// Index of the offending TOC entry.
        index: u32,
    },
    /// A TOC entry's kind is `0`, or in `6..=0x7FFF`.
    #[error("TOC entry {index} has invalid kind {kind}")]
    InvalidKind {
        /// Index of the offending TOC entry.
        index: u32,
        /// The raw, invalid kind value.
        kind: u16,
    },
    /// A TOC entry's kind is in the engine-reserved range `2..=5`.
    #[error("TOC entry {index} has reserved kind {kind}")]
    ReservedKind {
        /// Index of the offending TOC entry.
        index: u32,
        /// The raw, reserved kind value.
        kind: u16,
    },
    /// A TOC entry's declared length exceeds [`crate::MAX_ENTRY_LEN`].
    #[error("TOC entry {index} has length {len}, exceeding MAX_ENTRY_LEN")]
    EntryTooLarge {
        /// Index of the offending TOC entry.
        index: u32,
        /// The declared, too-large length.
        len: u64,
    },
    /// The manifest is structurally invalid in a way not covered by a more specific variant
    /// (bad `manifest_version`, an oversized or non-UTF-8 string, an oversized application
    /// block, or a `PackWriter` write-time validation failure such as a duplicate asset id).
    #[error("invalid pack manifest: {0}")]
    Manifest(String),
    /// A manifest entry count or path does not match the TOC entry at the same index.
    #[error("manifest entry {index} does not match its TOC entry")]
    ManifestMismatch {
        /// Index of the offending entry (or, for a whole-manifest entry-count mismatch, the
        /// entry count the manifest declared).
        index: u32,
    },
}
