using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using Grimoire.Formats.Debug;
using Grimoire.Tools.Tests;
using Xunit;

namespace Grimoire.Formats.Tests;

/// <summary>
/// Behaviour of the generated codecs and the frame layer beyond the fixtures: limits, enums, UTF-8,
/// hostile input (contract §2 rule 9, §13).
/// </summary>
public sealed class CodecTests
{
    [Fact]
    public void EveryPayloadTypeRoundTrips()
    {
        var preview = new SigilPreview { UnitPath = "sigils/a.sigil", UnitBytes = new byte[] { 9, 9, 9 }, Ticks = 30, Target = new[] { 1.5f, -2.5f } };
        var log = new LogMsg { Level = 3, Tick = 12_345, Target = "grimoire_sim", Text = "tick advanced" };
        var ack = new SwapAck { InReplyTo = 5, Status = 0, AppliedTick = 101, ContentSwaps = 1, ContentManifest = 999, UnitHash = 0xDEAD_BEEF_0000_0001 };
        Assert.Equal(preview, SigilPreview.Decode(preview.Encode()));
        Assert.Equal(new SigilPreview { Target = null }, SigilPreview.Decode(new SigilPreview { Target = null }.Encode()));
        Assert.Equal(log, LogMsg.Decode(log.Encode()));
        Assert.Equal(ack, SwapAck.Decode(ack.Encode()));
        Assert.NotEqual(preview, SigilPreview.Decode(new SigilPreview { UnitPath = "sigils/a.sigil", UnitBytes = new byte[] { 9, 9, 9 }, Ticks = 30 }.Encode()));
    }

    [Fact]
    public void FloatsRoundTripBitExactly()
    {
        var nan = BitConverter.Int32BitsToSingle(0x7fc0_0001);
        var stats = new Stats { Alpha = nan, Fps = -0.0f };
        var decoded = Stats.Decode(stats.Encode());
        Assert.Equal(0x7fc0_0001, BitConverter.SingleToInt32Bits(decoded.Alpha));
        Assert.Equal(BitConverter.SingleToInt32Bits(-0.0f), BitConverter.SingleToInt32Bits(decoded.Fps));
        Assert.NotEqual(stats, new Stats { Alpha = nan, Fps = 0.0f });
    }

    [Fact]
    public void AnErrorCodeFromANewerProtocolDecodesAsMalformed()
    {
        var bytes = DebugFixtureTests.SampleError().Encode();
        BinaryPrimitives.WriteUInt16LittleEndian(bytes, 999);
        Assert.Equal(ErrorCode.Malformed, ErrorMsg.Decode(bytes).Code);
    }

    [Fact]
    public void AnUndefinedRoleBoolOrOptionTagIsInvalidEnum()
    {
        var hello = DebugFixtureTests.SampleHello().Encode();
        hello[2] = 7;
        var roleError = Assert.Throws<WireFormatException>(() => Hello.Decode(hello));
        Assert.Equal((WireErrorKind.InvalidEnum, "role", 7u), (roleError.Kind, roleError.Field, roleError.Value));

        var stats = DebugFixtureTests.SampleStats().Encode();
        stats[stats.Length - 8 - 4 - 10 - 4 - 1] = 2;
        Assert.Equal("estimate", Assert.Throws<WireFormatException>(() => Stats.Decode(stats)).Field);

        var preview = new SigilPreview { Target = new[] { 1f, 2f } }.Encode();
        preview[preview.Length - 9] = 2;
        Assert.Equal(WireErrorKind.InvalidEnum, Assert.Throws<WireFormatException>(() => SigilPreview.Decode(preview)).Kind);
    }

    [Fact]
    public void LengthsAndCountsAreCheckedBeforeAllocating()
    {
        // `scopes` declares u32::MAX elements: rejected against its maximum of 64.
        var stats = new Stats().Encode();
        BinaryPrimitives.WriteUInt32LittleEndian(stats.AsSpan(stats.Length - 8), uint.MaxValue);
        var tooMany = Assert.Throws<WireFormatException>(() => Stats.Decode(stats));
        Assert.Equal((WireErrorKind.FieldTooLong, "scopes", 64L), (tooMany.Kind, tooMany.Field, tooMany.Max));

        // 64 declared elements of at least 27 bytes each (u16 + empty str + u64 + u32 + u64 + bool) with
        // only 4 bytes left: rejected before the list is allocated.
        BinaryPrimitives.WriteUInt32LittleEndian(stats.AsSpan(stats.Length - 8), 64);
        var early = Assert.Throws<WireFormatException>(() => Stats.Decode(stats));
        Assert.Equal((WireErrorKind.UnexpectedEnd, 64L * 27), (early.Kind, early.Needed));

        // A unit of MaxUnitBytes + 1 bytes declared by its length prefix.
        var swap = DebugFixtureTests.SampleSwap().Encode();
        BinaryPrimitives.WriteUInt32LittleEndian(swap.AsSpan(4 + 23), DebugProtocol.MaxUnitBytes + 1);
        Assert.Equal(WireErrorKind.FieldTooLong, Assert.Throws<WireFormatException>(() => SwapSigilUnit.Decode(swap)).Kind);
    }

    [Fact]
    public void StringsAreStrictUtf8BothWays()
    {
        var log = new LogMsg { Target = "t", Text = "ab" }.Encode();
        log[log.Length - 1] = 0xFF;
        var invalid = Assert.Throws<WireFormatException>(() => LogMsg.Decode(log));
        Assert.Equal((WireErrorKind.InvalidUtf8, "text"), (invalid.Kind, invalid.Field));

        var surrogate = new LogMsg { Text = "\ud800" };
        Assert.Equal(WireErrorKind.InvalidUtf8, Assert.Throws<WireFormatException>(() => surrogate.Encode()).Kind);

        var tooLong = new LogMsg { Target = new string('x', 129) };
        var error = Assert.Throws<WireFormatException>(() => tooLong.Encode());
        Assert.Equal((WireErrorKind.FieldTooLong, "target", 129L, 128L), (error.Kind, error.Field, error.Length, error.Max));

        var multiByte = new LogMsg { Text = "grüße" };
        Assert.Equal(multiByte, LogMsg.Decode(multiByte.Encode()));
    }

    [Fact]
    public void FixedArraysAndCountedListsMustHaveTheirDeclaredLength()
    {
        var hello = new Hello { Token = new byte[31] };
        var error = Assert.Throws<WireFormatException>(() => hello.Encode());
        Assert.Equal((WireErrorKind.ArrayLength, "token"), (error.Kind, error.Field));

        var stats = new Stats();
        for (var i = 0; i < 65; i++)
        {
            stats.Counters.Add(new StatsCounter { Name = "c" });
        }

        Assert.Equal(WireErrorKind.FieldTooLong, Assert.Throws<WireFormatException>(() => stats.Encode()).Kind);
    }

    [Fact]
    public void TheFrameDecoderReassemblesSplitAndConcatenatedFrames()
    {
        var first = Message.Of(DebugFixtureTests.SampleStats()).ToFrame(3).Encode();
        var second = Message.Of(DebugFixtureTests.SampleError()).ToFrame(4).Encode();
        var stream = new byte[first.Length + second.Length];
        first.CopyTo(stream, 0);
        second.CopyTo(stream, first.Length);

        var decoder = new FrameDecoder();
        var frames = new List<Frame>();
        foreach (var b in stream)
        {
            decoder.Push(new[] { b });
            while (decoder.TryReadFrame(out var frame))
            {
                frames.Add(frame!);
            }
        }

        Assert.Equal(new uint[] { 3, 4 }, frames.ConvertAll(f => f.Seq));
        Assert.Equal(Message.Of(DebugFixtureTests.SampleStats()), Message.FromFrame(frames[0]));
        Assert.Equal(0, decoder.BufferedBytes);
    }

    [Fact]
    public void InvalidFrameLengthsFailFromTheLengthFieldAlone()
    {
        var shortFrame = new FrameDecoder();
        shortFrame.Push(new byte[] { 7, 0, 0, 0 });
        Assert.Equal(WireErrorKind.FrameTooShort, Assert.Throws<WireFormatException>(() => shortFrame.TryReadFrame(out _)).Kind);

        var largeFrame = new FrameDecoder();
        largeFrame.Push(BitConverter.GetBytes(DebugProtocol.MaxFrameLength + 1));
        Assert.Equal(WireErrorKind.FrameTooLarge, Assert.Throws<WireFormatException>(() => largeFrame.TryReadFrame(out _)).Kind);

        var helloLimit = new FrameDecoder(DebugProtocol.MaxHelloFrameLength);
        helloLimit.Push(BitConverter.GetBytes(DebugProtocol.MaxHelloFrameLength + 1));
        Assert.Equal(WireErrorKind.FrameTooLarge, Assert.Throws<WireFormatException>(() => helloLimit.TryReadFrame(out _)).Kind);
    }

    [Fact]
    public void NonZeroFlagsConsumeTheFrameAndKeepTheStreamInSync()
    {
        var bad = Message.Of(DebugFixtureTests.SampleError()).ToFrame(1).Encode();
        bad[6] = 1;
        var good = Message.Of(DebugFixtureTests.SampleError()).ToFrame(2).Encode();
        var decoder = new FrameDecoder();
        decoder.Push(bad);
        decoder.Push(good);
        Assert.Equal(WireErrorKind.NonZeroFlags, Assert.Throws<WireFormatException>(() => decoder.TryReadFrame(out _)).Kind);
        Assert.True(decoder.TryReadFrame(out var frame));
        Assert.Equal((uint)2, frame!.Seq);
    }

    [Theory]
    [InlineData((ushort)0x0000)]
    [InlineData((ushort)0x0004)]
    [InlineData((ushort)0x0100)]
    [InlineData((ushort)0x8000)]
    public void IdsOutsideTheCatalogueAreUnknownMessages(ushort id)
    {
        var error = Assert.Throws<WireFormatException>(() => Message.FromFrame(new Frame(id, 1, Array.Empty<byte>())));
        Assert.Equal((WireErrorKind.UnknownMessage, (uint)id), (error.Kind, error.Value));
    }

    [Fact]
    public void ArbitraryBytesNeverThrowAnythingButWireFormatException()
    {
        var random = new Random(0x5EED);
        var decoders = new Func<byte[], object>[]
        {
            b => Hello.Decode(b), b => ErrorMsg.Decode(b), b => LogMsg.Decode(b), b => Stats.Decode(b),
            b => SwapSigilUnit.Decode(b), b => SwapAck.Decode(b), b => SigilPreview.Decode(b),
        };
        for (var iteration = 0; iteration < 3000; iteration++)
        {
            var bytes = new byte[random.Next(0, 160)];
            random.NextBytes(bytes);
            foreach (var decode in decoders)
            {
                AssertOnlyWireErrors(() => decode(bytes));
            }

            AssertOnlyWireErrors(() =>
            {
                var decoder = new FrameDecoder();
                decoder.Push(bytes);
                while (decoder.TryReadFrame(out var frame))
                {
                    Message.FromFrame(frame!);
                }
            });
        }
    }

    [Fact]
    public void SingleByteMutationsOfTheFixturesNeverThrowAnythingButWireFormatException()
    {
        var random = new Random(7);
        var fixtures = new (byte[] Bytes, Func<byte[], object> Decode)[]
        {
            (RustFixtures.Bytes("debug_v1", "hello.bin"), b => Hello.Decode(b)),
            (RustFixtures.Bytes("debug_v1", "error.bin"), b => ErrorMsg.Decode(b)),
            (RustFixtures.Bytes("debug_v1", "stats.bin"), b => Stats.Decode(b)),
            (RustFixtures.Bytes("debug_v1", "swap_sigil_unit.bin"), b => SwapSigilUnit.Decode(b)),
        };
        foreach (var (bytes, decode) in fixtures)
        {
            for (var position = 0; position < bytes.Length; position++)
            {
                var mutated = (byte[])bytes.Clone();
                mutated[position] = (byte)random.Next(256);
                AssertOnlyWireErrors(() => decode(mutated));
            }
        }
    }

    private static void AssertOnlyWireErrors(Action action)
    {
        try
        {
            action();
        }
        catch (WireFormatException)
        {
        }
    }

    private static void AssertOnlyWireErrors(Func<object> action) => AssertOnlyWireErrors(() => { action(); });
}
