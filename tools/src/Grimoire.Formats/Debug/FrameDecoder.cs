using System;
using System.Buffers.Binary;

namespace Grimoire.Formats.Debug;

/// <summary>
/// Splits a byte stream into frames (contract §13 "Framing"), checking each length field before it
/// allocates the frame.
/// </summary>
/// <remarks>
/// A length field below the header size or above the maximum is unrecoverable: the stream is out of
/// sync and the connection must be closed. A non-zero <c>flags</c> field consumes its frame and then
/// throws, so the stream stays in sync. An incomplete frame is not an error; <see cref="TryReadFrame"/>
/// returns <c>false</c> until more bytes arrive.
/// </remarks>
public sealed class FrameDecoder
{
    private byte[] _buffer = new byte[256];
    private int _start;
    private int _end;

    /// <summary>
    /// Creates a decoder that accepts frames up to <paramref name="maxFrameLength"/>.
    /// </summary>
    /// <param name="maxFrameLength">Largest accepted length field; at most <see cref="DebugProtocol.MaxFrameLength"/>.</param>
    public FrameDecoder(uint maxFrameLength = DebugProtocol.MaxFrameLength)
    {
        if (maxFrameLength < DebugProtocol.HeaderLength || maxFrameLength > DebugProtocol.MaxFrameLength)
        {
            throw new ArgumentOutOfRangeException(nameof(maxFrameLength));
        }

        MaxFrameLength = maxFrameLength;
    }

    /// <summary>
    /// Largest accepted length field.
    /// </summary>
    public uint MaxFrameLength { get; }

    /// <summary>
    /// Bytes pushed but not yet returned as frames.
    /// </summary>
    public int BufferedBytes => _end - _start;

    /// <summary>
    /// Appends received bytes.
    /// </summary>
    /// <param name="bytes">Bytes.</param>
    public void Push(ReadOnlySpan<byte> bytes)
    {
        if (bytes.IsEmpty)
        {
            return;
        }

        if (_buffer.Length - _end < bytes.Length)
        {
            var buffered = _end - _start;
            var needed = buffered + bytes.Length;
            if (needed <= _buffer.Length)
            {
                _buffer.AsSpan(_start, buffered).CopyTo(_buffer);
            }
            else
            {
                var grown = new byte[Math.Max(needed, _buffer.Length * 2)];
                _buffer.AsSpan(_start, buffered).CopyTo(grown);
                _buffer = grown;
            }

            _start = 0;
            _end = buffered;
        }

        bytes.CopyTo(_buffer.AsSpan(_end));
        _end += bytes.Length;
    }

    /// <summary>
    /// Returns the next complete frame, if one is buffered.
    /// </summary>
    /// <param name="frame">The frame, or <c>null</c>.</param>
    /// <returns><c>true</c> if a frame was returned.</returns>
    /// <exception cref="WireFormatException">
    /// <see cref="WireErrorKind.FrameTooShort"/> or <see cref="WireErrorKind.FrameTooLarge"/> (close the
    /// connection), or <see cref="WireErrorKind.NonZeroFlags"/> (the frame was consumed).
    /// </exception>
    public bool TryReadFrame(out Frame? frame)
    {
        frame = null;
        var buffered = _buffer.AsSpan(_start, _end - _start);
        if (buffered.Length < DebugProtocol.LengthPrefixLength)
        {
            return false;
        }

        var length = BinaryPrimitives.ReadUInt32LittleEndian(buffered);
        if (length < DebugProtocol.HeaderLength)
        {
            throw WireFormatException.FrameTooShort(length);
        }

        if (length > MaxFrameLength)
        {
            throw WireFormatException.FrameTooLarge(length, MaxFrameLength);
        }

        var total = DebugProtocol.LengthPrefixLength + (int)length;
        if (buffered.Length < total)
        {
            return false;
        }

        var id = BinaryPrimitives.ReadUInt16LittleEndian(buffered.Slice(4));
        var flags = BinaryPrimitives.ReadUInt16LittleEndian(buffered.Slice(6));
        var seq = BinaryPrimitives.ReadUInt32LittleEndian(buffered.Slice(8));
        var payload = buffered.Slice(12, (int)length - DebugProtocol.HeaderLength).ToArray();
        _start += total;
        if (_start == _end)
        {
            _start = 0;
            _end = 0;
        }

        if (flags != 0)
        {
            throw WireFormatException.NonZeroFlags(flags);
        }

        frame = new Frame(id, seq, payload);
        return true;
    }
}
