//! Error type of `grimoire_sim`.

/// Errors of the simulation crate, currently all about decoding an [`crate::InputLog`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SimError {
    /// The input ended inside the fixed-size header.
    #[error("replay data ends early: {needed} bytes needed at offset {offset}, {available} left")]
    UnexpectedEnd {
        /// Byte offset of the field that could not be read.
        offset: usize,
        /// Size of that field in bytes.
        needed: usize,
        /// Bytes left from `offset` to the end of the input.
        available: usize,
    },
    /// The input does not start with `b"GRIMREPL"`.
    #[error("replay data does not start with the magic bytes GRIMREPL")]
    BadMagic,
    /// The format version is not supported by this build.
    #[error("unsupported replay format version {0}")]
    UnsupportedVersion(u32),
    /// The header declares a tick rate of 0 Hz.
    #[error("replay tick rate must be at least 1 Hz")]
    InvalidTickRate,
    /// The payload size does not equal the declared frame count times the frame size.
    #[error("replay declares {frames} frames but {remaining} payload bytes follow")]
    FrameDataLength {
        /// Frame count declared in the header.
        frames: u64,
        /// Bytes actually following the header.
        remaining: usize,
    },
    /// A version-2 header did not consume exactly its declared `header_len` bytes.
    #[error("replay header declares {declared} bytes but {consumed} were read")]
    HeaderLength {
        /// `header_len` declared in the input.
        declared: u32,
        /// Bytes actually consumed while reading the header (or available, if `declared`
        /// already exceeded the remaining input).
        consumed: usize,
    },
    /// A length-prefixed text field exceeded its maximum length.
    #[error("field `{field}` is {len} bytes long, at most {max} allowed")]
    FieldTooLong {
        /// Name of the offending field.
        field: &'static str,
        /// Length actually declared.
        len: usize,
        /// Maximum allowed length.
        max: usize,
    },
    /// A repeated group (swaps, metadata entries) declared more entries than allowed.
    #[error("field `{field}` declares {count} entries, at most {max} allowed")]
    TooManyEntries {
        /// Name of the offending field.
        field: &'static str,
        /// Entry count actually declared.
        count: u64,
        /// Maximum allowed entry count.
        max: usize,
    },
    /// A text field was not valid UTF-8 or used a character outside its allowed charset.
    #[error("field `{field}` is not valid text")]
    InvalidText {
        /// Name of the offending field.
        field: &'static str,
    },
    /// A hex-encoded field did not decode (wrong length or a non-hex-digit byte).
    #[error("field `{field}` is not valid hex")]
    InvalidHex {
        /// Name of the offending field.
        field: &'static str,
    },
    /// Metadata keys were not strictly ascending in byte order.
    #[error("metadata key at index {index} is not strictly greater than the previous one")]
    MetadataKeyOrder {
        /// Index of the offending entry.
        index: usize,
    },
    /// Swap ticks were not strictly ascending.
    #[error(
        "swap record at index {index} does not have a strictly greater tick than the previous one"
    )]
    SwapOrder {
        /// Index of the offending entry.
        index: usize,
    },
    /// A swap record's tick lies beyond the end of the log.
    #[error("swap at tick {tick} lies beyond the {frames} recorded frames")]
    SwapOutOfRange {
        /// Tick declared by the swap record.
        tick: u64,
        /// Number of frames in the log.
        frames: u64,
    },
}
