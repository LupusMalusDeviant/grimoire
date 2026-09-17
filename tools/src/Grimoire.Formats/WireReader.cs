using System;
using System.Buffers.Binary;
using System.Text;

namespace Grimoire.Formats;

/// <summary>
/// Bounds-checked little-endian reader over a byte span, the runtime of the generated decoders.
/// </summary>
/// <remarks>
/// Every method checks its length against the bytes remaining, and every length or count against its
/// maximum, before it allocates anything sized by that value (contract §2 rule 9). Malformed input
/// only ever throws <see cref="WireFormatException"/>.
/// </remarks>
public ref struct WireReader
{
    private static readonly UTF8Encoding StrictUtf8 = new(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);

    private readonly ReadOnlySpan<byte> _bytes;
    private int _position;

    /// <summary>
    /// Creates a reader positioned at the first byte of <paramref name="bytes"/>.
    /// </summary>
    /// <param name="bytes">Input.</param>
    public WireReader(ReadOnlySpan<byte> bytes)
    {
        _bytes = bytes;
        _position = 0;
    }

    /// <summary>
    /// Bytes read so far.
    /// </summary>
    public readonly int Position => _position;

    /// <summary>
    /// Bytes not read yet.
    /// </summary>
    public readonly int Remaining => _bytes.Length - _position;

    /// <summary>
    /// Reads <paramref name="length"/> bytes.
    /// </summary>
    /// <param name="length">Number of bytes.</param>
    /// <returns>The bytes, a slice of the input.</returns>
    /// <exception cref="WireFormatException">Fewer bytes remain.</exception>
    public ReadOnlySpan<byte> Take(long length)
    {
        if (length < 0 || length > Remaining)
        {
            throw WireFormatException.UnexpectedEnd(_position, length, Remaining);
        }

        var slice = _bytes.Slice(_position, (int)length);
        _position += (int)length;
        return slice;
    }

    /// <summary>
    /// Reads a <c>u8</c>.
    /// </summary>
    /// <returns>The value.</returns>
    public byte U8() => Take(1)[0];

    /// <summary>
    /// Reads a little-endian <c>u16</c>.
    /// </summary>
    /// <returns>The value.</returns>
    public ushort U16() => BinaryPrimitives.ReadUInt16LittleEndian(Take(2));

    /// <summary>
    /// Reads a little-endian <c>u32</c>.
    /// </summary>
    /// <returns>The value.</returns>
    public uint U32() => BinaryPrimitives.ReadUInt32LittleEndian(Take(4));

    /// <summary>
    /// Reads a little-endian <c>u64</c>.
    /// </summary>
    /// <returns>The value.</returns>
    public ulong U64() => BinaryPrimitives.ReadUInt64LittleEndian(Take(8));

    /// <summary>
    /// Reads an <c>f32</c> bit-exactly.
    /// </summary>
    /// <returns>The value.</returns>
    public float F32() => BitConverter.Int32BitsToSingle(BinaryPrimitives.ReadInt32LittleEndian(Take(4)));

    /// <summary>
    /// Reads a <c>bool</c>: one byte, <c>0</c> or <c>1</c>.
    /// </summary>
    /// <param name="field">Field name for the error.</param>
    /// <returns>The value.</returns>
    /// <exception cref="WireFormatException">Any other byte (<see cref="WireErrorKind.InvalidEnum"/>).</exception>
    public bool Bool(string field) => U8() switch
    {
        0 => false,
        1 => true,
        var other => throw WireFormatException.InvalidEnum(field, other),
    };

    /// <summary>
    /// Reads an option presence tag: one byte, <c>0</c> absent, <c>1</c> present.
    /// </summary>
    /// <param name="field">Field name for the error.</param>
    /// <returns>Whether a value follows.</returns>
    public bool OptionTag(string field) => Bool(field);

    /// <summary>
    /// Reads a fixed-size byte array.
    /// </summary>
    /// <param name="length">Array length.</param>
    /// <returns>A copy of the bytes.</returns>
    public byte[] Bytes(int length) => Take(length).ToArray();

    /// <summary>
    /// Reads a fixed-size <c>f32</c> array.
    /// </summary>
    /// <param name="length">Array length.</param>
    /// <returns>The values.</returns>
    public float[] Floats(int length)
    {
        if ((long)length * 4 > Remaining)
        {
            throw WireFormatException.UnexpectedEnd(_position, (long)length * 4, Remaining);
        }

        var values = new float[length];
        for (var i = 0; i < length; i++)
        {
            values[i] = F32();
        }

        return values;
    }

    /// <summary>
    /// Reads a <c>Str≤max</c>: <c>u32</c> length, then that many UTF-8 bytes.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="max">Maximum length in bytes.</param>
    /// <returns>The string.</returns>
    public string StrU32(string field, uint max) => String(field, max, U32());

    /// <summary>
    /// Reads a <c>Str16≤max</c>: <c>u16</c> length, then that many UTF-8 bytes.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="max">Maximum length in bytes.</param>
    /// <returns>The string.</returns>
    public string StrU16(string field, uint max) => String(field, max, U16());

    /// <summary>
    /// Reads a <c>Bytes≤max</c>: <c>u32</c> length, then that many bytes.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="max">Maximum length.</param>
    /// <returns>A copy of the bytes.</returns>
    public byte[] BytesU32(string field, uint max)
    {
        var length = U32();
        if (length > max)
        {
            throw WireFormatException.FieldTooLong(field, length, max);
        }

        return Take(length).ToArray();
    }

    /// <summary>
    /// Reads a <c>Vec≤max</c> element count and checks it against the maximum and against the bytes
    /// remaining, at <paramref name="minElementSize"/> bytes per element, before the caller allocates.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="max">Maximum element count.</param>
    /// <param name="minElementSize">Smallest wire size of one element.</param>
    /// <returns>The count.</returns>
    public int Count(string field, uint max, uint minElementSize) =>
        CountUsing(field, max, U32(), minElementSize);

    /// <summary>
    /// Checks an element count that another field declared, like <see cref="Count"/>.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="max">Maximum element count.</param>
    /// <param name="declared">Declared count.</param>
    /// <param name="minElementSize">Smallest wire size of one element.</param>
    /// <returns>The count.</returns>
    public readonly int CountUsing(string field, uint max, ulong declared, uint minElementSize)
    {
        if (declared > max)
        {
            throw WireFormatException.FieldTooLong(field, (long)Math.Min(declared, long.MaxValue), max);
        }

        var needed = (long)declared * minElementSize;
        if (needed > Remaining)
        {
            throw WireFormatException.UnexpectedEnd(_position, needed, Remaining);
        }

        return (int)declared;
    }

    /// <summary>
    /// Checks that every byte has been read.
    /// </summary>
    /// <exception cref="WireFormatException">Bytes are left over (<see cref="WireErrorKind.TrailingBytes"/>).</exception>
    public readonly void Finish()
    {
        if (Remaining != 0)
        {
            throw WireFormatException.TrailingBytes(Remaining);
        }
    }

    private string String(string field, uint max, uint length)
    {
        if (length > max)
        {
            throw WireFormatException.FieldTooLong(field, length, max);
        }

        var bytes = Take(length);
        try
        {
            return StrictUtf8.GetString(bytes);
        }
        catch (DecoderFallbackException)
        {
            throw WireFormatException.InvalidUtf8(field);
        }
    }
}
