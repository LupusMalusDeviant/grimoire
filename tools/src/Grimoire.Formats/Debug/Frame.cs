using System;
using System.Buffers.Binary;

namespace Grimoire.Formats.Debug;

/// <summary>
/// One debug-protocol frame (contract §13 "Framing"): message id, sequence number and payload.
/// </summary>
public sealed class Frame : IEquatable<Frame>
{
    /// <summary>
    /// Creates a frame.
    /// </summary>
    /// <param name="id">Message id.</param>
    /// <param name="seq">Sender sequence number; <c>0</c> expects no reply.</param>
    /// <param name="payload">Payload bytes.</param>
    public Frame(ushort id, uint seq, byte[] payload)
    {
        ArgumentNullException.ThrowIfNull(payload);
        Id = id;
        Seq = seq;
        Payload = payload;
    }

    /// <summary>
    /// Message id.
    /// </summary>
    public ushort Id { get; }

    /// <summary>
    /// Sender sequence number.
    /// </summary>
    public uint Seq { get; }

    /// <summary>
    /// Payload bytes.
    /// </summary>
    public byte[] Payload { get; }

    /// <summary>
    /// Encodes this frame: <c>len: u32</c>, <c>id: u16</c>, <c>flags: u16 = 0</c>, <c>seq: u32</c>, payload.
    /// </summary>
    /// <returns>The frame bytes.</returns>
    /// <exception cref="WireFormatException">The frame would exceed <see cref="DebugProtocol.MaxFrameLength"/>.</exception>
    public byte[] Encode()
    {
        var length = (long)DebugProtocol.HeaderLength + Payload.Length;
        if (length > DebugProtocol.MaxFrameLength)
        {
            throw WireFormatException.FrameTooLarge(length, DebugProtocol.MaxFrameLength);
        }

        var bytes = new byte[DebugProtocol.LengthPrefixLength + length];
        var span = bytes.AsSpan();
        BinaryPrimitives.WriteUInt32LittleEndian(span, (uint)length);
        BinaryPrimitives.WriteUInt16LittleEndian(span.Slice(4), Id);
        BinaryPrimitives.WriteUInt16LittleEndian(span.Slice(6), 0);
        BinaryPrimitives.WriteUInt32LittleEndian(span.Slice(8), Seq);
        Payload.CopyTo(span.Slice(12));
        return bytes;
    }

    /// <summary>
    /// The frozen first field of a <c>Hello</c> payload: <c>Some</c> only for id <c>0x0001</c> with at least
    /// two payload bytes (contract §13 <c>peek_hello_version</c>).
    /// </summary>
    /// <returns>The protocol version, or <c>null</c>.</returns>
    public ushort? PeekHelloVersion() =>
        Id == DebugProtocolV1.HelloId && Payload.Length >= 2
            ? BinaryPrimitives.ReadUInt16LittleEndian(Payload)
            : null;

    /// <inheritdoc/>
    public bool Equals(Frame? other) =>
        other is not null && Id == other.Id && Seq == other.Seq && WireEquality.Bytes(Payload, other.Payload);

    /// <inheritdoc/>
    public override bool Equals(object? obj) => Equals(obj as Frame);

    /// <inheritdoc/>
    public override int GetHashCode() => HashCode.Combine(Id, Seq, Payload.Length);
}
