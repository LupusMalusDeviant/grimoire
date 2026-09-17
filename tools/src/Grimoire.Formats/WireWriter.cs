using System;
using System.Buffers;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.Text;

namespace Grimoire.Formats;

/// <summary>
/// Little-endian writer, the runtime of the generated encoders.
/// </summary>
/// <remarks>
/// Every length and count is checked against its maximum before anything of the field is written. A
/// failed encode may leave a partial field in the writer; <c>Encode()</c> of a generated type discards
/// the writer in that case.
/// </remarks>
public sealed class WireWriter
{
    private static readonly UTF8Encoding StrictUtf8 = new(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);

    private readonly ArrayBufferWriter<byte> _buffer = new();

    /// <summary>
    /// Number of bytes written.
    /// </summary>
    public int Length => _buffer.WrittenCount;

    /// <summary>
    /// The bytes written so far.
    /// </summary>
    public ReadOnlySpan<byte> Written => _buffer.WrittenSpan;

    /// <summary>
    /// Copies the bytes written so far into a new array.
    /// </summary>
    /// <returns>The bytes.</returns>
    public byte[] ToArray() => _buffer.WrittenSpan.ToArray();

    /// <summary>
    /// Writes raw bytes without a length prefix.
    /// </summary>
    /// <param name="bytes">Bytes.</param>
    public void Raw(ReadOnlySpan<byte> bytes) => _buffer.Write(bytes);

    /// <summary>
    /// Writes a <c>u8</c>.
    /// </summary>
    /// <param name="value">Value.</param>
    public void U8(byte value)
    {
        _buffer.GetSpan(1)[0] = value;
        _buffer.Advance(1);
    }

    /// <summary>
    /// Writes a little-endian <c>u16</c>.
    /// </summary>
    /// <param name="value">Value.</param>
    public void U16(ushort value)
    {
        BinaryPrimitives.WriteUInt16LittleEndian(_buffer.GetSpan(2), value);
        _buffer.Advance(2);
    }

    /// <summary>
    /// Writes a little-endian <c>u32</c>.
    /// </summary>
    /// <param name="value">Value.</param>
    public void U32(uint value)
    {
        BinaryPrimitives.WriteUInt32LittleEndian(_buffer.GetSpan(4), value);
        _buffer.Advance(4);
    }

    /// <summary>
    /// Writes a little-endian <c>u64</c>.
    /// </summary>
    /// <param name="value">Value.</param>
    public void U64(ulong value)
    {
        BinaryPrimitives.WriteUInt64LittleEndian(_buffer.GetSpan(8), value);
        _buffer.Advance(8);
    }

    /// <summary>
    /// Writes an <c>f32</c> bit-exactly.
    /// </summary>
    /// <param name="value">Value.</param>
    public void F32(float value) => U32(unchecked((uint)BitConverter.SingleToInt32Bits(value)));

    /// <summary>
    /// Writes a <c>bool</c> as one byte.
    /// </summary>
    /// <param name="value">Value.</param>
    public void Bool(bool value) => U8(value ? (byte)1 : (byte)0);

    /// <summary>
    /// Writes an option presence tag.
    /// </summary>
    /// <param name="present">Whether a value follows.</param>
    public void OptionTag(bool present) => Bool(present);

    /// <summary>
    /// Writes a fixed-size byte array without a length prefix.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="length">Declared length.</param>
    /// <param name="value">Array.</param>
    public void Bytes(string field, int length, byte[] value)
    {
        ArgumentNullException.ThrowIfNull(value);
        if (value.Length != length)
        {
            throw WireFormatException.ArrayLength(field, value.Length, length);
        }

        Raw(value);
    }

    /// <summary>
    /// Writes a fixed-size <c>f32</c> array without a length prefix.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="length">Declared length.</param>
    /// <param name="value">Array.</param>
    public void Floats(string field, int length, float[] value)
    {
        ArgumentNullException.ThrowIfNull(value);
        if (value.Length != length)
        {
            throw WireFormatException.ArrayLength(field, value.Length, length);
        }

        foreach (var item in value)
        {
            F32(item);
        }
    }

    /// <summary>
    /// Writes a <c>Str≤max</c>: <c>u32</c> length, then the UTF-8 bytes.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="max">Maximum length in bytes.</param>
    /// <param name="value">String.</param>
    public void StrU32(string field, uint max, string value)
    {
        var bytes = Utf8(field, max, value);
        U32((uint)bytes.Length);
        Raw(bytes);
    }

    /// <summary>
    /// Writes a <c>Str16≤max</c>: <c>u16</c> length, then the UTF-8 bytes.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="max">Maximum length in bytes, at most 65535.</param>
    /// <param name="value">String.</param>
    public void StrU16(string field, uint max, string value)
    {
        var bytes = Utf8(field, Math.Min(max, ushort.MaxValue), value);
        U16((ushort)bytes.Length);
        Raw(bytes);
    }

    /// <summary>
    /// Writes a <c>Bytes≤max</c>: <c>u32</c> length, then the bytes.
    /// </summary>
    /// <param name="field">Field name.</param>
    /// <param name="max">Maximum length.</param>
    /// <param name="value">Bytes.</param>
    public void BytesU32(string field, uint max, byte[] value)
    {
        ArgumentNullException.ThrowIfNull(value);
        if ((uint)value.Length > max)
        {
            throw WireFormatException.FieldTooLong(field, value.Length, max);
        }

        U32((uint)value.Length);
        Raw(value);
    }

    /// <summary>
    /// Writes a <c>Vec≤max</c> element count; the caller writes the elements.
    /// </summary>
    /// <typeparam name="T">Element type.</typeparam>
    /// <param name="field">Field name.</param>
    /// <param name="max">Maximum element count.</param>
    /// <param name="items">List.</param>
    public void Count<T>(string field, uint max, List<T> items)
    {
        ArgumentNullException.ThrowIfNull(items);
        if ((uint)items.Count > max)
        {
            throw WireFormatException.FieldTooLong(field, items.Count, max);
        }

        U32((uint)items.Count);
    }

    /// <summary>
    /// Checks a list whose count is another field's value: the count must match and not exceed the
    /// maximum. Writes nothing; the caller writes the elements.
    /// </summary>
    /// <typeparam name="T">Element type.</typeparam>
    /// <param name="field">Field name.</param>
    /// <param name="countField">Name of the count field.</param>
    /// <param name="max">Maximum element count.</param>
    /// <param name="declared">Value of the count field.</param>
    /// <param name="items">List.</param>
    public void CountUsing<T>(string field, string countField, uint max, ulong declared, List<T> items)
    {
        ArgumentNullException.ThrowIfNull(items);
        if (declared != (ulong)items.Count)
        {
            throw WireFormatException.CountMismatch(field, countField, (long)Math.Min(declared, long.MaxValue), items.Count);
        }

        if ((uint)items.Count > max)
        {
            throw WireFormatException.FieldTooLong(field, items.Count, max);
        }
    }

    private static byte[] Utf8(string field, uint max, string value)
    {
        ArgumentNullException.ThrowIfNull(value);
        byte[] bytes;
        try
        {
            bytes = StrictUtf8.GetBytes(value);
        }
        catch (EncoderFallbackException)
        {
            throw WireFormatException.InvalidUtf8(field);
        }

        if ((uint)bytes.Length > max)
        {
            throw WireFormatException.FieldTooLong(field, bytes.Length, max);
        }

        return bytes;
    }
}
