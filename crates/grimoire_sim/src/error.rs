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
}
