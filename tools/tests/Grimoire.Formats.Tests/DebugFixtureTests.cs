using System;
using System.Collections.Generic;
using Grimoire.Formats.Debug;
using Grimoire.Tools.Tests;
using Xunit;

namespace Grimoire.Formats.Tests;

/// <summary>
/// The debug protocol v1 against the engine's golden fixtures: the payload files of
/// <c>grimoire_debug/tests/fixtures/debug_v1/</c> and the full frames in
/// <c>grimoire_debug/tests/handshake_golden.rs</c>, all hand-derived from contract §13.
/// </summary>
public sealed class DebugFixtureTests
{
    private const string HandshakeSource = "handshake_golden.rs";

    internal static Hello SampleHello() => new()
    {
        ProtocolVersion = 1,
        Role = PeerRole.Tool,
        EngineVersion = "0.1.1",
        BuildHash = "deadbeefdeadbeefdeadbeefdeadbeef",
        Token = Filled(32, 0xAA),
        StatsIntervalFrames = 30,
    };

    internal static ErrorMsg SampleError() => new()
    {
        Code = ErrorCode.VersionMismatch,
        InReplyTo = 7,
        Message = "protocol version mismatch",
    };

    internal static Stats SampleStats() => new()
    {
        Frame = 100,
        SimTick = 100,
        TicksThisFrame = 1,
        Alpha = 0.5f,
        FrameTimeNs = 16_000_000,
        Fps = 60.0f,
        DroppedTimeNs = 0,
        ContentSwaps = 0,
        ContentManifest = 0,
        Scopes = new List<StatsScope>
        {
            new() { Scope = 4, Name = "physics", TotalNs = 123_456, Calls = 10, BudgetNs = 200_000, Estimate = false },
        },
        Counters = new List<StatsCounter> { new() { Name = "draw_calls", Value = 42 } },
    };

    internal static SwapSigilUnit SampleSwap() => new()
    {
        UnitPath = "sigils/basic_bolt.sigil",
        UnitBytes = new byte[] { 1, 2, 3, 4 },
    };

    internal static byte[] Filled(int length, byte value)
    {
        var bytes = new byte[length];
        Array.Fill(bytes, value);
        return bytes;
    }

    public static TheoryData<string> PayloadFixtures => new() { "hello", "error", "stats", "swap_sigil_unit" };

    [Fact]
    public void HelloPayloadFixtureDecodesAndReencodes()
    {
        var bytes = RustFixtures.Bytes("debug_v1", "hello.bin");
        Assert.Equal(SampleHello(), Hello.Decode(bytes));
        Assert.Equal(bytes, SampleHello().Encode());
    }

    [Fact]
    public void ErrorPayloadFixtureDecodesAndReencodes()
    {
        var bytes = RustFixtures.Bytes("debug_v1", "error.bin");
        Assert.Equal(SampleError(), ErrorMsg.Decode(bytes));
        Assert.Equal(bytes, SampleError().Encode());
    }

    [Fact]
    public void StatsPayloadFixtureDecodesAndReencodes()
    {
        var bytes = RustFixtures.Bytes("debug_v1", "stats.bin");
        Assert.Equal(SampleStats(), Stats.Decode(bytes));
        Assert.Equal(bytes, SampleStats().Encode());
    }

    [Fact]
    public void SwapSigilUnitPayloadFixtureDecodesAndReencodes()
    {
        var bytes = RustFixtures.Bytes("debug_v1", "swap_sigil_unit.bin");
        Assert.Equal(SampleSwap(), SwapSigilUnit.Decode(bytes));
        Assert.Equal(bytes, SampleSwap().Encode());
    }

    [Theory]
    [MemberData(nameof(PayloadFixtures))]
    public void EveryTruncationAndATrailingByteOfAPayloadFixtureFail(string name)
    {
        var bytes = RustFixtures.Bytes("debug_v1", name + ".bin");
        Func<byte[], object> decode = name switch
        {
            "hello" => b => Hello.Decode(b),
            "error" => b => ErrorMsg.Decode(b),
            "stats" => b => Stats.Decode(b),
            _ => b => SwapSigilUnit.Decode(b),
        };
        for (var length = 0; length < bytes.Length; length++)
        {
            var error = Assert.Throws<WireFormatException>(() => decode(bytes.AsSpan(0, length).ToArray()));
            Assert.Equal(WireErrorKind.UnexpectedEnd, error.Kind);
        }

        var longer = new byte[bytes.Length + 1];
        bytes.CopyTo(longer, 0);
        Assert.Equal(WireErrorKind.TrailingBytes, Assert.Throws<WireFormatException>(() => decode(longer)).Kind);
    }

    [Fact]
    public void HelloRequestFrameIsWhatTheToolHandshakeSends()
    {
        var bytes = RustFixtures.ByteLiteral(HandshakeSource, "HELLO_REQUEST_FRAME_BYTES");
        var identity = new ToolIdentity(Filled(32, 0x11), engineVersion: "0.1.1", buildHash: "unknown", statsIntervalFrames: 30);
        Assert.Equal(bytes, ToolHandshake.HelloFrame(identity).Encode());

        var frame = DecodeOneFrame(bytes);
        var hello = Assert.IsType<Message<Hello>>(Message.FromFrame(frame));
        Assert.Equal(PeerRole.Tool, hello.Payload.Role);
        Assert.Equal(Filled(32, 0x11), hello.Payload.Token);
        Assert.Equal((ushort)30, hello.Payload.StatsIntervalFrames);
        Assert.Equal((uint)1, frame.Seq);
    }

    [Fact]
    public void HelloResponseFrameIsAnAcceptedHandshake()
    {
        var bytes = RustFixtures.ByteLiteral(HandshakeSource, "HELLO_RESPONSE_FRAME_BYTES");
        var expected = new Hello
        {
            ProtocolVersion = 1,
            Role = PeerRole.Engine,
            EngineVersion = "0.1.1",
            BuildHash = "unknown",
            Token = new byte[32],
            StatsIntervalFrames = 0,
        };
        var frame = DecodeOneFrame(bytes);
        var reply = ToolHandshake.Interpret(frame);
        Assert.Equal(HandshakeOutcome.Accepted, reply.Outcome);
        Assert.Equal(expected, reply.EngineHello);
        Assert.Equal(bytes, Message.Of(expected).ToFrame(1).Encode());
    }

    [Fact]
    public void ErrorFrameIsARejectedHandshake()
    {
        var bytes = RustFixtures.ByteLiteral(HandshakeSource, "ERROR_FRAME_BYTES");
        var frame = DecodeOneFrame(bytes);
        Assert.Equal((uint)5, frame.Seq);
        var reply = ToolHandshake.Interpret(frame);
        Assert.Equal(HandshakeOutcome.Rejected, reply.Outcome);
        Assert.Equal(SampleError(), reply.Error);
        Assert.Equal(bytes, Message.Of(SampleError()).ToFrame(5).Encode());
    }

    [Fact]
    public void SwapAckFrameDecodesAndReencodes()
    {
        var bytes = RustFixtures.ByteLiteral(HandshakeSource, "SWAP_ACK_FRAME_BYTES");
        var expected = new SwapAck
        {
            InReplyTo = 42,
            Status = 1,
            AppliedTick = 0,
            ContentSwaps = 3,
            ContentManifest = 0xAABB_CCDD,
            UnitHash = 0x1122_3344_5566_7788,
            Reason = "unit id mismatch",
        };
        var frame = DecodeOneFrame(bytes);
        Assert.Equal(Message.Of(expected), Message.FromFrame(frame));
        Assert.Equal(MessageDirection.EngineToTool, Message.FromFrame(frame).Direction);
        Assert.Equal(bytes, Message.Of(expected).ToFrame(9).Encode());
    }

    [Fact]
    public void TheCatalogueMatchesTheContractTable()
    {
        var expected = new (ushort Id, string Name, MessageDirection Direction, Type Payload)[]
        {
            (0x0001, "Hello", MessageDirection.Both, typeof(Hello)),
            (0x0002, "Error", MessageDirection.Both, typeof(ErrorMsg)),
            (0x0003, "Log", MessageDirection.EngineToTool, typeof(LogMsg)),
            (0x0010, "Stats", MessageDirection.EngineToTool, typeof(Stats)),
            (0x0020, "SwapSigilUnit", MessageDirection.ToolToEngine, typeof(SwapSigilUnit)),
            (0x0021, "SwapAck", MessageDirection.EngineToTool, typeof(SwapAck)),
            (0x0022, "SigilPreview", MessageDirection.ToolToEngine, typeof(SigilPreview)),
        };
        Assert.Equal(expected.Length, DebugProtocolV1.Catalogue.Count);
        for (var i = 0; i < expected.Length; i++)
        {
            var entry = DebugProtocolV1.Catalogue[i];
            Assert.Equal(expected[i], (entry.Id, entry.Name, entry.Direction, entry.PayloadType));
        }

        Assert.Equal(1u, DebugProtocolV1.FormatVersion);
    }

    private static Frame DecodeOneFrame(byte[] bytes)
    {
        var decoder = new FrameDecoder();
        decoder.Push(bytes);
        Assert.True(decoder.TryReadFrame(out var frame));
        Assert.False(decoder.TryReadFrame(out _));
        Assert.Equal(0, decoder.BufferedBytes);
        return frame!;
    }
}
