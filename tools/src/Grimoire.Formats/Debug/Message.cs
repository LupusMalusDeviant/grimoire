using System;

namespace Grimoire.Formats.Debug;

/// <summary>
/// A decoded message of the debug protocol v1 catalogue (contract §13): a frame's id plus its typed
/// payload. The payload types and ids are generated (<see cref="DebugProtocolV1"/>); this dispatch is
/// hand-written, like <c>grimoire_debug::Message</c>.
/// </summary>
/// <remarks>
/// Build one with an <c>Of</c> overload and match on <see cref="Message{TPayload}"/>, e.g.
/// <c>message is Message&lt;Stats&gt; stats</c>.
/// </remarks>
public abstract class Message : IEquatable<Message>
{
    private protected Message(ushort id)
    {
        Id = id;
    }

    /// <summary>
    /// Message id.
    /// </summary>
    public ushort Id { get; }

    /// <summary>
    /// Direction the message may travel in.
    /// </summary>
    public MessageDirection Direction
    {
        get
        {
            foreach (var entry in DebugProtocolV1.Catalogue)
            {
                if (entry.Id == Id)
                {
                    return entry.Direction;
                }
            }

            return MessageDirection.Both;
        }
    }

    /// <summary>
    /// Wraps a <c>Hello</c>.
    /// </summary>
    /// <param name="payload">Payload.</param>
    /// <returns>The message.</returns>
    public static Message<Hello> Of(Hello payload) => new(DebugProtocolV1.HelloId, payload, static p => p.Encode());

    /// <summary>
    /// Wraps an <c>Error</c>.
    /// </summary>
    /// <param name="payload">Payload.</param>
    /// <returns>The message.</returns>
    public static Message<ErrorMsg> Of(ErrorMsg payload) => new(DebugProtocolV1.ErrorId, payload, static p => p.Encode());

    /// <summary>
    /// Wraps a <c>Log</c>.
    /// </summary>
    /// <param name="payload">Payload.</param>
    /// <returns>The message.</returns>
    public static Message<LogMsg> Of(LogMsg payload) => new(DebugProtocolV1.LogId, payload, static p => p.Encode());

    /// <summary>
    /// Wraps a <c>Stats</c>.
    /// </summary>
    /// <param name="payload">Payload.</param>
    /// <returns>The message.</returns>
    public static Message<Stats> Of(Stats payload) => new(DebugProtocolV1.StatsId, payload, static p => p.Encode());

    /// <summary>
    /// Wraps a <c>SwapSigilUnit</c>.
    /// </summary>
    /// <param name="payload">Payload.</param>
    /// <returns>The message.</returns>
    public static Message<SwapSigilUnit> Of(SwapSigilUnit payload) =>
        new(DebugProtocolV1.SwapSigilUnitId, payload, static p => p.Encode());

    /// <summary>
    /// Wraps a <c>SwapAck</c>.
    /// </summary>
    /// <param name="payload">Payload.</param>
    /// <returns>The message.</returns>
    public static Message<SwapAck> Of(SwapAck payload) => new(DebugProtocolV1.SwapAckId, payload, static p => p.Encode());

    /// <summary>
    /// Wraps a <c>SigilPreview</c>.
    /// </summary>
    /// <param name="payload">Payload.</param>
    /// <returns>The message.</returns>
    public static Message<SigilPreview> Of(SigilPreview payload) =>
        new(DebugProtocolV1.SigilPreviewId, payload, static p => p.Encode());

    /// <summary>
    /// Decodes the payload of <paramref name="frame"/> by its id.
    /// </summary>
    /// <param name="frame">Frame.</param>
    /// <returns>The message.</returns>
    /// <exception cref="WireFormatException">
    /// <see cref="WireErrorKind.UnknownMessage"/> for an id outside the catalogue (also <c>0x0000</c>,
    /// reserved and application ranges), or the payload decoder's error.
    /// </exception>
    public static Message FromFrame(Frame frame)
    {
        ArgumentNullException.ThrowIfNull(frame);
        return frame.Id switch
        {
            DebugProtocolV1.HelloId => Of(Hello.Decode(frame.Payload)),
            DebugProtocolV1.ErrorId => Of(ErrorMsg.Decode(frame.Payload)),
            DebugProtocolV1.LogId => Of(LogMsg.Decode(frame.Payload)),
            DebugProtocolV1.StatsId => Of(Stats.Decode(frame.Payload)),
            DebugProtocolV1.SwapSigilUnitId => Of(SwapSigilUnit.Decode(frame.Payload)),
            DebugProtocolV1.SwapAckId => Of(SwapAck.Decode(frame.Payload)),
            DebugProtocolV1.SigilPreviewId => Of(SigilPreview.Decode(frame.Payload)),
            _ => throw WireFormatException.UnknownMessage(frame.Id),
        };
    }

    /// <summary>
    /// Encodes the payload.
    /// </summary>
    /// <returns>The payload bytes.</returns>
    /// <exception cref="WireFormatException">A length or count exceeds its maximum.</exception>
    public abstract byte[] EncodePayload();

    /// <summary>
    /// Encodes this message as a frame with sequence number <paramref name="seq"/>.
    /// </summary>
    /// <param name="seq">Sequence number.</param>
    /// <returns>The frame.</returns>
    /// <exception cref="WireFormatException">The payload does not encode or the frame is too large.</exception>
    public Frame ToFrame(uint seq)
    {
        var payload = EncodePayload();
        var length = (long)DebugProtocol.HeaderLength + payload.Length;
        if (length > DebugProtocol.MaxFrameLength)
        {
            throw WireFormatException.FrameTooLarge(length, DebugProtocol.MaxFrameLength);
        }

        return new Frame(Id, seq, payload);
    }

    /// <inheritdoc/>
    public abstract bool Equals(Message? other);

    /// <inheritdoc/>
    public override bool Equals(object? obj) => Equals(obj as Message);

    /// <inheritdoc/>
    public override int GetHashCode() => Id;
}

/// <summary>
/// A message with its typed payload.
/// </summary>
/// <typeparam name="TPayload">Generated payload type.</typeparam>
public sealed class Message<TPayload> : Message
    where TPayload : class, IEquatable<TPayload>
{
    private readonly Func<TPayload, byte[]> _encode;

    internal Message(ushort id, TPayload payload, Func<TPayload, byte[]> encode)
        : base(id)
    {
        ArgumentNullException.ThrowIfNull(payload);
        Payload = payload;
        _encode = encode;
    }

    /// <summary>
    /// The payload.
    /// </summary>
    public TPayload Payload { get; }

    /// <inheritdoc/>
    public override byte[] EncodePayload() => _encode(Payload);

    /// <inheritdoc/>
    public override bool Equals(Message? other) =>
        other is Message<TPayload> typed && typed.Id == Id && Payload.Equals(typed.Payload);

    /// <inheritdoc/>
    public override bool Equals(object? obj) => Equals(obj as Message);

    /// <inheritdoc/>
    public override int GetHashCode() => HashCode.Combine(Id, Payload);
}
