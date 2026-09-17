using System;

namespace Grimoire.Formats;

/// <summary>
/// Why an encoding or decoding step of an engine wire format failed (contract §2 rule 9, §13).
/// </summary>
public enum WireErrorKind
{
    /// <summary>
    /// Fewer bytes remain than a field needs.
    /// </summary>
    UnexpectedEnd,

    /// <summary>
    /// Bytes are left over after the last field of a payload.
    /// </summary>
    TrailingBytes,

    /// <summary>
    /// A string field is not valid UTF-8, or a string to encode cannot be encoded as UTF-8.
    /// </summary>
    InvalidUtf8,

    /// <summary>
    /// A length or element count exceeds the field's maximum.
    /// </summary>
    FieldTooLong,

    /// <summary>
    /// An enum, <c>bool</c> or option tag byte has no defined meaning.
    /// </summary>
    InvalidEnum,

    /// <summary>
    /// A list whose count is another field's value does not have that many elements.
    /// </summary>
    CountMismatch,

    /// <summary>
    /// A fixed-size array to encode does not have the declared length.
    /// </summary>
    ArrayLength,

    /// <summary>
    /// A frame's length field is smaller than the 8-byte frame header.
    /// </summary>
    FrameTooShort,

    /// <summary>
    /// A frame's length field exceeds the maximum frame length.
    /// </summary>
    FrameTooLarge,

    /// <summary>
    /// A frame's <c>flags</c> field is not zero.
    /// </summary>
    NonZeroFlags,

    /// <summary>
    /// A frame carries a message id outside the catalogue.
    /// </summary>
    UnknownMessage,
}

/// <summary>
/// The only exception the codecs of <c>Grimoire.Formats</c> throw for malformed input or for a value
/// that cannot be encoded. The properties that apply to <see cref="Kind"/> are set; the others are
/// <c>null</c> or zero.
/// </summary>
public sealed class WireFormatException : Exception
{
    private WireFormatException(WireErrorKind kind, string message)
        : base(message)
    {
        Kind = kind;
    }

    /// <summary>
    /// What went wrong.
    /// </summary>
    public WireErrorKind Kind { get; }

    /// <summary>
    /// Schema name of the field concerned, if any.
    /// </summary>
    public string? Field { get; private init; }

    /// <summary>
    /// Read position at which the error was found (<see cref="WireErrorKind.UnexpectedEnd"/>).
    /// </summary>
    public long Offset { get; private init; }

    /// <summary>
    /// Bytes needed (<see cref="WireErrorKind.UnexpectedEnd"/>).
    /// </summary>
    public long Needed { get; private init; }

    /// <summary>
    /// Bytes available (<see cref="WireErrorKind.UnexpectedEnd"/>) or left over
    /// (<see cref="WireErrorKind.TrailingBytes"/>).
    /// </summary>
    public long Available { get; private init; }

    /// <summary>
    /// Offending length, count or frame length.
    /// </summary>
    public long Length { get; private init; }

    /// <summary>
    /// Maximum or expected length.
    /// </summary>
    public long Max { get; private init; }

    /// <summary>
    /// Offending raw value (enum byte, flags, message id).
    /// </summary>
    public uint Value { get; private init; }

    /// <summary>
    /// Name of the field holding the declared count (<see cref="WireErrorKind.CountMismatch"/>).
    /// </summary>
    public string? CountField { get; private init; }

    /// <summary>
    /// Creates an <see cref="WireErrorKind.UnexpectedEnd"/> error.
    /// </summary>
    /// <param name="offset">Read position.</param>
    /// <param name="needed">Bytes needed.</param>
    /// <param name="available">Bytes available.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException UnexpectedEnd(long offset, long needed, long available) =>
        new(WireErrorKind.UnexpectedEnd, $"input ends early: {needed} bytes needed at offset {offset}, {available} left")
        {
            Offset = offset,
            Needed = needed,
            Available = available,
        };

    /// <summary>
    /// Creates a <see cref="WireErrorKind.TrailingBytes"/> error.
    /// </summary>
    /// <param name="extra">Bytes left over.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException TrailingBytes(long extra) =>
        new(WireErrorKind.TrailingBytes, $"{extra} trailing bytes after the payload") { Available = extra };

    /// <summary>
    /// Creates an <see cref="WireErrorKind.InvalidUtf8"/> error.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException InvalidUtf8(string field) =>
        new(WireErrorKind.InvalidUtf8, $"field `{field}` is not valid UTF-8") { Field = field };

    /// <summary>
    /// Creates a <see cref="WireErrorKind.FieldTooLong"/> error.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="length">Offending length or count.</param>
    /// <param name="max">Maximum.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException FieldTooLong(string field, long length, long max) =>
        new(WireErrorKind.FieldTooLong, $"field `{field}` has length {length}, at most {max} allowed")
        {
            Field = field,
            Length = length,
            Max = max,
        };

    /// <summary>
    /// Creates an <see cref="WireErrorKind.InvalidEnum"/> error.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="value">Offending raw value.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException InvalidEnum(string field, uint value) =>
        new(WireErrorKind.InvalidEnum, $"field `{field}` has no variant {value}") { Field = field, Value = value };

    /// <summary>
    /// Creates a <see cref="WireErrorKind.CountMismatch"/> error.
    /// </summary>
    /// <param name="field">List field name.</param>
    /// <param name="countField">Name of the field holding the count.</param>
    /// <param name="declared">Declared count.</param>
    /// <param name="actual">Elements present.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException CountMismatch(string field, string countField, long declared, long actual) =>
        new(WireErrorKind.CountMismatch, $"field `{field}` has {actual} elements but `{countField}` declares {declared}")
        {
            Field = field,
            CountField = countField,
            Max = declared,
            Length = actual,
        };

    /// <summary>
    /// Creates an <see cref="WireErrorKind.ArrayLength"/> error.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="length">Actual array length.</param>
    /// <param name="expected">Declared array length.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException ArrayLength(string field, long length, long expected) =>
        new(WireErrorKind.ArrayLength, $"field `{field}` has {length} elements, exactly {expected} required")
        {
            Field = field,
            Length = length,
            Max = expected,
        };

    /// <summary>
    /// Creates a <see cref="WireErrorKind.FrameTooShort"/> error.
    /// </summary>
    /// <param name="length">Frame length field.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException FrameTooShort(uint length) =>
        new(WireErrorKind.FrameTooShort, $"frame length {length} is shorter than the frame header") { Length = length };

    /// <summary>
    /// Creates a <see cref="WireErrorKind.FrameTooLarge"/> error.
    /// </summary>
    /// <param name="length">Frame length field.</param>
    /// <param name="max">Maximum frame length.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException FrameTooLarge(long length, long max) =>
        new(WireErrorKind.FrameTooLarge, $"frame length {length} exceeds {max}") { Length = length, Max = max };

    /// <summary>
    /// Creates a <see cref="WireErrorKind.NonZeroFlags"/> error.
    /// </summary>
    /// <param name="flags">Frame flags field.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException NonZeroFlags(ushort flags) =>
        new(WireErrorKind.NonZeroFlags, $"frame flags {flags:x4} are not zero") { Value = flags };

    /// <summary>
    /// Creates an <see cref="WireErrorKind.UnknownMessage"/> error.
    /// </summary>
    /// <param name="id">Message id.</param>
    /// <returns>The exception.</returns>
    public static WireFormatException UnknownMessage(ushort id) =>
        new(WireErrorKind.UnknownMessage, $"message id 0x{id:x4} is not in the catalogue") { Value = id };
}
