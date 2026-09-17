using System;

namespace Grimoire.Formats.Pack;

/// <summary>
/// Why a pack could not be read or written; mirrors <c>grimoire_assets::PackError</c> plus the two read
/// errors of <c>AssetError</c> (contract §12).
/// </summary>
public enum PackErrorKind
{
    /// <summary>
    /// The input ends inside a structure.
    /// </summary>
    UnexpectedEnd,

    /// <summary>
    /// The file does not start with <c>GRIMPACK</c>.
    /// </summary>
    BadMagic,

    /// <summary>
    /// The format version is not 1.
    /// </summary>
    UnsupportedVersion,

    /// <summary>
    /// The header length is not 64.
    /// </summary>
    HeaderLength,

    /// <summary>
    /// The declared file length differs from the input length.
    /// </summary>
    FileLength,

    /// <summary>
    /// A flags or reserved field is not zero.
    /// </summary>
    NonZeroReserved,

    /// <summary>
    /// More than 65,536 entries.
    /// </summary>
    TooManyEntries,

    /// <summary>
    /// A table, payload or the manifest lies outside its allowed range.
    /// </summary>
    OutOfBounds,

    /// <summary>
    /// A payload offset is not a multiple of 16.
    /// </summary>
    Misaligned,

    /// <summary>
    /// A payload starts before the previous one ends.
    /// </summary>
    Overlap,

    /// <summary>
    /// Entry ids are not strictly ascending.
    /// </summary>
    UnsortedIds,

    /// <summary>
    /// Kind 0 or 6 to 0x7FFF.
    /// </summary>
    InvalidKind,

    /// <summary>
    /// A kind reserved for later engine versions (2 to 5).
    /// </summary>
    ReservedKind,

    /// <summary>
    /// An entry is longer than 256 MiB.
    /// </summary>
    EntryTooLarge,

    /// <summary>
    /// The manifest does not decode or is invalid.
    /// </summary>
    Manifest,

    /// <summary>
    /// The manifest's entry count or a path does not match the table of contents.
    /// </summary>
    ManifestMismatch,

    /// <summary>
    /// No entry has the requested id.
    /// </summary>
    NotFound,

    /// <summary>
    /// An entry's bytes do not match its SHA-256.
    /// </summary>
    HashMismatch,
}

/// <summary>
/// The only exception <see cref="PackReader"/> and <see cref="PackWriter"/> throw for malformed or
/// invalid packs.
/// </summary>
public sealed class PackFormatException : Exception
{
    /// <summary>
    /// Creates the exception.
    /// </summary>
    /// <param name="kind">What went wrong.</param>
    /// <param name="message">Detail.</param>
    /// <param name="index">Entry index, if the error concerns one.</param>
    public PackFormatException(PackErrorKind kind, string message, long? index = null)
        : base(message)
    {
        Kind = kind;
        Index = index;
    }

    /// <summary>
    /// What went wrong.
    /// </summary>
    public PackErrorKind Kind { get; }

    /// <summary>
    /// Entry index the error concerns, if any.
    /// </summary>
    public long? Index { get; }
}
