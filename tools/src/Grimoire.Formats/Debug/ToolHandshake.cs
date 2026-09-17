using System;

namespace Grimoire.Formats.Debug;

/// <summary>
/// What a tool presents when it connects to the engine (contract §13 "Handshake").
/// </summary>
public sealed class ToolIdentity
{
    /// <summary>
    /// Creates an identity.
    /// </summary>
    /// <param name="token">The 32-byte debug-link token.</param>
    /// <param name="engineVersion">Engine version the tool targets; defaults to <see cref="EngineBuild.EngineVersion"/>.</param>
    /// <param name="buildHash">This tool build's hash; defaults to <see cref="EngineBuild.BuildHash"/>.</param>
    /// <param name="statsIntervalFrames"><c>Stats</c> cadence in frames; <c>0</c> requests none.</param>
    public ToolIdentity(byte[] token, string? engineVersion = null, string? buildHash = null, ushort statsIntervalFrames = 0)
    {
        ArgumentNullException.ThrowIfNull(token);
        if (token.Length != 32)
        {
            throw new ArgumentException("the debug-link token has 32 bytes", nameof(token));
        }

        Token = (byte[])token.Clone();
        EngineVersion = engineVersion ?? EngineBuild.EngineVersion;
        BuildHash = buildHash ?? EngineBuild.BuildHash;
        StatsIntervalFrames = statsIntervalFrames;
    }

    /// <summary>
    /// The debug-link token.
    /// </summary>
    public byte[] Token { get; }

    /// <summary>
    /// Engine version the tool targets; the engine rejects any other.
    /// </summary>
    public string EngineVersion { get; }

    /// <summary>
    /// This tool build's hash, or <c>unknown</c>.
    /// </summary>
    public string BuildHash { get; }

    /// <summary>
    /// <c>Stats</c> cadence in frames; <c>0</c> requests none.
    /// </summary>
    public ushort StatsIntervalFrames { get; }
}

/// <summary>
/// How the engine answered a tool's <c>Hello</c>.
/// </summary>
public enum HandshakeOutcome
{
    /// <summary>
    /// The engine answered with its own <c>Hello</c>; the connection is open.
    /// </summary>
    Accepted,

    /// <summary>
    /// The engine answered with an <c>Error</c> and closes the connection.
    /// </summary>
    Rejected,

    /// <summary>
    /// The reply was neither a valid engine <c>Hello</c> nor a valid <c>Error</c>.
    /// </summary>
    Malformed,
}

/// <summary>
/// The engine's reply to a tool's <c>Hello</c>, interpreted.
/// </summary>
/// <param name="Outcome">Accepted, rejected or malformed.</param>
/// <param name="EngineHello">The engine's <c>Hello</c> when accepted.</param>
/// <param name="Error">The engine's <c>Error</c> when rejected.</param>
public sealed record HandshakeReply(HandshakeOutcome Outcome, Hello? EngineHello, ErrorMsg? Error);

/// <summary>
/// The tool side of the debug-link handshake (contract §13 "Handshake"): the tool sends its
/// <c>Hello</c> as its first frame (<c>seq = 1</c>) and reads exactly one reply.
/// </summary>
public static class ToolHandshake
{
    /// <summary>
    /// Sequence number of the tool's <c>Hello</c>.
    /// </summary>
    public const uint HelloSeq = 1;

    /// <summary>
    /// The tool's first frame.
    /// </summary>
    /// <param name="identity">Tool identity.</param>
    /// <returns>The <c>Hello</c> frame.</returns>
    /// <exception cref="WireFormatException">A field of the identity exceeds its maximum.</exception>
    public static Frame HelloFrame(ToolIdentity identity)
    {
        ArgumentNullException.ThrowIfNull(identity);
        var hello = new Hello
        {
            ProtocolVersion = DebugProtocol.ProtocolVersion,
            Role = PeerRole.Tool,
            EngineVersion = identity.EngineVersion,
            BuildHash = identity.BuildHash,
            Token = (byte[])identity.Token.Clone(),
            StatsIntervalFrames = identity.StatsIntervalFrames,
        };
        return Message.Of(hello).ToFrame(HelloSeq);
    }

    /// <summary>
    /// Interprets the engine's first frame. Never throws for malformed input.
    /// </summary>
    /// <param name="reply">The first frame the engine sent.</param>
    /// <returns>The interpreted reply.</returns>
    public static HandshakeReply Interpret(Frame reply)
    {
        ArgumentNullException.ThrowIfNull(reply);
        try
        {
            switch (reply.Id)
            {
                case DebugProtocolV1.HelloId:
                    if (reply.PeekHelloVersion() != DebugProtocol.ProtocolVersion)
                    {
                        return new HandshakeReply(HandshakeOutcome.Malformed, null, null);
                    }

                    var hello = Hello.Decode(reply.Payload);
                    return hello.Role == PeerRole.Engine
                        ? new HandshakeReply(HandshakeOutcome.Accepted, hello, null)
                        : new HandshakeReply(HandshakeOutcome.Malformed, null, null);
                case DebugProtocolV1.ErrorId:
                    return new HandshakeReply(HandshakeOutcome.Rejected, null, ErrorMsg.Decode(reply.Payload));
                default:
                    return new HandshakeReply(HandshakeOutcome.Malformed, null, null);
            }
        }
        catch (WireFormatException)
        {
            return new HandshakeReply(HandshakeOutcome.Malformed, null, null);
        }
    }
}
