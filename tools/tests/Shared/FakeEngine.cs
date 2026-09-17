using System;
using System.IO;
using System.Security.Cryptography;
using System.Threading;
using System.Threading.Tasks;
using Grimoire.Formats;
using Grimoire.Formats.Debug;
using Grimoire.LiveLink;

namespace Grimoire.Tools.Tests;

/// <summary>
/// The engine side of one debug-link connection, following contract §13 "Handshake" steps 1 to 8, for
/// driving <see cref="LiveLinkClient"/> in tests. Frames after the handshake are read and sent by the test.
/// </summary>
internal sealed class FakeEngine
{
    public static readonly byte[] Token = Filled(32, 0x11);

    public string EngineVersion { get; init; } = "0.1.1";

    public string BuildHash { get; init; } = "unknown";

    public byte[] ExpectedToken { get; init; } = Token;

    public static ToolIdentity Identity(ushort statsIntervalFrames = 30, string buildHash = "unknown") =>
        new(Token, engineVersion: "0.1.1", buildHash: buildHash, statsIntervalFrames: statsIntervalFrames);

    public static LiveLinkOptions Options(ToolIdentity? identity = null) => new(identity ?? Identity())
    {
        InitialReconnectDelay = TimeSpan.FromMilliseconds(10),
        MaxReconnectDelay = TimeSpan.FromMilliseconds(40),
        HandshakeTimeout = TimeSpan.FromSeconds(5),
    };

    public static byte[] Filled(int length, byte value)
    {
        var bytes = new byte[length];
        Array.Fill(bytes, value);
        return bytes;
    }

    /// <summary>
    /// Runs the engine's handshake on <paramref name="stream"/>: answers with <c>Hello</c> and returns the
    /// tool's <c>Hello</c>, or answers with <c>Error</c>, closes the stream and returns <c>null</c>.
    /// </summary>
    public async Task<Hello?> AcceptAsync(Stream stream, CancellationToken cancellationToken)
    {
        Frame first;
        try
        {
            first = await ReadFrameAsync(stream, new FrameDecoder(DebugProtocol.MaxHelloFrameLength), cancellationToken);
        }
        catch (WireFormatException)
        {
            await RejectAsync(stream, ErrorCode.TooLarge, "first frame too large");
            return null;
        }

        if (first.Id != DebugProtocolV1.HelloId)
        {
            await RejectAsync(stream, ErrorCode.HandshakeRequired, "handshake required");
            return null;
        }

        if (first.PeekHelloVersion() is not { } version)
        {
            await RejectAsync(stream, ErrorCode.Malformed, "malformed hello");
            return null;
        }

        if (version != DebugProtocol.ProtocolVersion)
        {
            await RejectAsync(stream, ErrorCode.VersionMismatch, "protocol version mismatch");
            return null;
        }

        Hello hello;
        try
        {
            hello = Hello.Decode(first.Payload);
        }
        catch (WireFormatException)
        {
            await RejectAsync(stream, ErrorCode.Malformed, "malformed hello");
            return null;
        }

        if (hello.Role != PeerRole.Tool)
        {
            await RejectAsync(stream, ErrorCode.Malformed, "role is not tool");
            return null;
        }

        if (!CryptographicOperations.FixedTimeEquals(hello.Token, ExpectedToken))
        {
            await RejectAsync(stream, ErrorCode.Unauthorized, "unauthorized");
            return null;
        }

        var bothKnown = hello.BuildHash != EngineBuild.UnknownBuildHash && BuildHash != EngineBuild.UnknownBuildHash;
        if (!string.Equals(hello.EngineVersion, EngineVersion, StringComparison.Ordinal) || (bothKnown && hello.BuildHash != BuildHash))
        {
            await RejectAsync(stream, ErrorCode.VersionMismatch, $"engine {EngineVersion}, tool {hello.EngineVersion}");
            return null;
        }

        await SendAsync(stream, Message.Of(new Hello
        {
            ProtocolVersion = DebugProtocol.ProtocolVersion,
            Role = PeerRole.Engine,
            EngineVersion = EngineVersion,
            BuildHash = BuildHash,
            Token = new byte[32],
            StatsIntervalFrames = 0,
        }), 1);
        return hello;
    }

    public static async Task SendAsync(Stream stream, Message message, uint seq)
    {
        await stream.WriteAsync(message.ToFrame(seq).Encode());
        await stream.FlushAsync();
    }

    public static async Task<Frame> ReadFrameAsync(Stream stream, FrameDecoder decoder, CancellationToken cancellationToken)
    {
        var buffer = new byte[4096];
        while (true)
        {
            if (decoder.TryReadFrame(out var frame))
            {
                return frame!;
            }

            var read = await stream.ReadAsync(buffer, cancellationToken);
            if (read == 0)
            {
                throw new EndOfStreamException("the tool closed the connection");
            }

            decoder.Push(buffer.AsSpan(0, read));
        }
    }

    public static async Task<byte[]> ReadExactlyAsync(Stream stream, int length, CancellationToken cancellationToken)
    {
        var bytes = new byte[length];
        await stream.ReadExactlyAsync(bytes, cancellationToken);
        return bytes;
    }

    private static async Task RejectAsync(Stream stream, ErrorCode code, string message)
    {
        try
        {
            await SendAsync(stream, Message.Of(new ErrorMsg { Code = code, InReplyTo = 1, Message = message }), 1);
        }
        catch (IOException)
        {
        }

        await stream.DisposeAsync();
    }
}
