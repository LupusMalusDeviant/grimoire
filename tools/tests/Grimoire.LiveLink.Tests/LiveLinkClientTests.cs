using System;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Grimoire.Formats;
using Grimoire.Formats.Debug;
using Grimoire.Tools.Tests;
using Xunit;

namespace Grimoire.LiveLink.Tests;

/// <summary>
/// <see cref="LiveLinkClient"/> against a contract-§13 engine double over in-memory streams: handshake,
/// rejection, reconnect, typed messages, swaps and degrading to file work.
/// </summary>
public sealed class LiveLinkClientTests
{
    /// <summary>
    /// The test's cancellation token, additionally cancelled after 20 seconds so a hang fails the test.
    /// </summary>
    private static CancellationToken Timeout()
    {
        var source = CancellationTokenSource.CreateLinkedTokenSource(TestContext.Current.CancellationToken);
        source.CancelAfter(TimeSpan.FromSeconds(20));
        return source.Token;
    }

    private static async Task<(LiveLinkClient Client, InMemoryLinkConnector Connector, Stream Engine)> ConnectedAsync(
        FakeEngine? engine = null, int maxReadChunk = int.MaxValue)
    {
        var connector = new InMemoryLinkConnector { MaxReadChunk = maxReadChunk };
        var client = new LiveLinkClient(connector, FakeEngine.Options());
        client.Start();
        var stream = await connector.AcceptAsync(Timeout());
        Assert.NotNull(await (engine ?? new FakeEngine()).AcceptAsync(stream, Timeout()));
        await client.WaitForStateAsync(LinkState.Connected, Timeout());
        return (client, connector, stream);
    }

    [Fact]
    public async Task TheToolHelloIsByteForByteTheEnginesGoldenRequestFrame()
    {
        var expected = RustFixtures.ByteLiteral("handshake_golden.rs", "HELLO_REQUEST_FRAME_BYTES");
        var response = RustFixtures.ByteLiteral("handshake_golden.rs", "HELLO_RESPONSE_FRAME_BYTES");
        var connector = new InMemoryLinkConnector();
        await using var client = new LiveLinkClient(connector, FakeEngine.Options());
        client.Start();
        var engine = await connector.AcceptAsync(Timeout());

        Assert.Equal(expected, await FakeEngine.ReadExactlyAsync(engine, expected.Length, Timeout()));
        await engine.WriteAsync(response, Timeout());
        await client.WaitForStateAsync(LinkState.Connected, Timeout());

        Assert.Equal(PeerRole.Engine, client.EngineHello!.Role);
        Assert.Equal("0.1.1", client.EngineHello.EngineVersion);
    }

    [Theory]
    [InlineData(1)]
    [InlineData(7)]
    public async Task StatsAndLogArriveTypedInOrderEvenInSmallChunks(int chunk)
    {
        var (client, _, engine) = await ConnectedAsync(maxReadChunk: chunk);
        await using var _ = client;
        var stats = new Stats { Frame = 5, SimTick = 9, Fps = 60f };
        await FakeEngine.SendAsync(engine, Message.Of(stats), 2);
        await FakeEngine.SendAsync(engine, Message.Of(new LogMsg { Level = 3, Tick = 9, Target = "grimoire_sim", Text = "tick" }), 3);

        var first = await client.Messages.ReadAsync(Timeout());
        var second = await client.Messages.ReadAsync(Timeout());
        Assert.Equal(stats, Assert.IsType<Message<Stats>>(first).Payload);
        Assert.Equal("tick", Assert.IsType<Message<LogMsg>>(second).Payload.Text);
    }

    [Fact]
    public async Task UnknownAndUndecodableFramesAreSkippedAndTheStreamStaysInSync()
    {
        var (client, _, engine) = await ConnectedAsync();
        await using var _ = client;
        await engine.WriteAsync(new Frame(0x8000, 2, new byte[] { 1, 2, 3 }).Encode(), Timeout());
        await engine.WriteAsync(new Frame(DebugProtocolV1.StatsId, 3, new byte[] { 1 }).Encode(), Timeout());
        await FakeEngine.SendAsync(engine, Message.Of(new Stats { Frame = 77 }), 4);

        var message = await client.Messages.ReadAsync(Timeout());
        Assert.Equal(77UL, Assert.IsType<Message<Stats>>(message).Payload.Frame);
        Assert.Equal(LinkState.Connected, client.State);
    }

    [Theory]
    [InlineData(ErrorCode.VersionMismatch)]
    [InlineData(ErrorCode.Unauthorized)]
    public async Task AVersionOrTokenRejectionStopsReconnecting(ErrorCode code)
    {
        var engine = code == ErrorCode.VersionMismatch
            ? new FakeEngine { EngineVersion = "9.9.9" }
            : new FakeEngine { ExpectedToken = FakeEngine.Filled(32, 0x22) };
        var connector = new InMemoryLinkConnector();
        await using var client = new LiveLinkClient(connector, FakeEngine.Options());
        client.Start();
        Assert.Null(await engine.AcceptAsync(await connector.AcceptAsync(Timeout()), Timeout()));
        await client.WaitForStateAsync(LinkState.Rejected, Timeout());

        Assert.Equal(code, client.Rejection!.Code);
        await Task.Delay(150, Timeout());
        Assert.Equal(1, connector.ConnectionAttempts);
        Assert.Equal(LinkState.Rejected, client.State);
        Assert.Equal(new SwapResult(SwapStatus.NotConnected), await client.SwapSigilUnitAsync("sigils/a.sigil", new byte[] { 1 }, TimeSpan.FromSeconds(1), Timeout()));
    }

    [Fact]
    public async Task DifferentKnownBuildHashesAreRejectedButUnknownIsAccepted()
    {
        var connector = new InMemoryLinkConnector();
        var identity = FakeEngine.Identity(buildHash: new string('a', 40));
        await using var client = new LiveLinkClient(connector, FakeEngine.Options(identity));
        client.Start();
        var strict = new FakeEngine { BuildHash = new string('b', 40) };
        Assert.Null(await strict.AcceptAsync(await connector.AcceptAsync(Timeout()), Timeout()));
        await client.WaitForStateAsync(LinkState.Rejected, Timeout());

        client.Start();
        var lenient = new FakeEngine { BuildHash = EngineBuild.UnknownBuildHash };
        Assert.NotNull(await lenient.AcceptAsync(await connector.AcceptAsync(Timeout()), Timeout()));
        await client.WaitForStateAsync(LinkState.Connected, Timeout());
    }

    [Fact]
    public async Task StartingAgainAfterARejectionAttemptsANewConnection()
    {
        var connector = new InMemoryLinkConnector();
        await using var client = new LiveLinkClient(connector, FakeEngine.Options());
        client.Start();
        Assert.Null(await new FakeEngine { EngineVersion = "9.9.9" }.AcceptAsync(await connector.AcceptAsync(Timeout()), Timeout()));
        await client.WaitForStateAsync(LinkState.Rejected, Timeout());

        client.Start();
        Assert.NotNull(await new FakeEngine().AcceptAsync(await connector.AcceptAsync(Timeout()), Timeout()));
        await client.WaitForStateAsync(LinkState.Connected, Timeout());
        Assert.Null(client.Rejection);
        Assert.Equal(2, connector.ConnectionAttempts);
    }

    [Fact]
    public async Task ABusyEngineIsRetriedUntilItAccepts()
    {
        var connector = new InMemoryLinkConnector();
        await using var client = new LiveLinkClient(connector, FakeEngine.Options());
        client.Start();

        var busy = await connector.AcceptAsync(Timeout());
        await FakeEngine.ReadFrameAsync(busy, new FrameDecoder(), Timeout());
        await FakeEngine.SendAsync(busy, Message.Of(new ErrorMsg { Code = ErrorCode.Busy, InReplyTo = 1, Message = "busy" }), 1);
        await busy.DisposeAsync();

        Assert.NotNull(await new FakeEngine().AcceptAsync(await connector.AcceptAsync(Timeout()), Timeout()));
        await client.WaitForStateAsync(LinkState.Connected, Timeout());
        Assert.Equal(2, connector.ConnectionAttempts);
    }

    [Fact]
    public async Task NoEngineMeansRetriesWithBackoffAndFileWorkUntilOneAppears()
    {
        var connector = new InMemoryLinkConnector { Refuse = true };
        await using var client = new LiveLinkClient(connector, FakeEngine.Options());
        client.Start();
        while (connector.ConnectionAttempts < 3)
        {
            await Task.Delay(10, Timeout());
        }

        Assert.NotEqual(LinkState.Connected, client.State);
        Assert.Equal(SwapStatus.NotConnected, (await client.SwapSigilUnitAsync("sigils/a.sigil", new byte[] { 1 }, TimeSpan.FromSeconds(1), Timeout())).Status);
        Assert.False((await client.SendAsync(Message.Of(new SigilPreview()), Timeout())).Sent);

        connector.Refuse = false;
        Assert.NotNull(await new FakeEngine().AcceptAsync(await connector.AcceptAsync(Timeout()), Timeout()));
        await client.WaitForStateAsync(LinkState.Connected, Timeout());
    }

    [Fact]
    public async Task ADropMidFrameDisconnectsWithoutThrowingAndReconnects()
    {
        var (client, connector, engine) = await ConnectedAsync();
        await using var _ = client;
        var disconnected = client.WaitForStateAsync(LinkState.Disconnected, Timeout());
        var frame = Message.Of(new Stats { Frame = 1 }).ToFrame(2).Encode();
        await engine.WriteAsync(frame.AsMemory(0, frame.Length / 2), Timeout());
        await engine.DisposeAsync();
        await disconnected;

        Assert.NotNull(await new FakeEngine().AcceptAsync(await connector.AcceptAsync(Timeout()), Timeout()));
        await client.WaitForStateAsync(LinkState.Connected, Timeout());
        Assert.Equal(2, connector.ConnectionAttempts);
    }

    [Fact]
    public async Task AnInvalidFrameLengthClosesTheLinkAndReconnects()
    {
        var (client, connector, engine) = await ConnectedAsync();
        await using var _ = client;
        var disconnected = client.WaitForStateAsync(LinkState.Disconnected, Timeout());
        await engine.WriteAsync(BitConverter.GetBytes(DebugProtocol.MaxFrameLength + 1), Timeout());
        await disconnected;

        Assert.NotNull(await new FakeEngine().AcceptAsync(await connector.AcceptAsync(Timeout()), Timeout()));
        await client.WaitForStateAsync(LinkState.Connected, Timeout());
    }

    [Theory]
    [InlineData((byte)0, SwapStatus.Applied)]
    [InlineData((byte)1, SwapStatus.Rejected)]
    [InlineData((byte)2, SwapStatus.Superseded)]
    public async Task ASwapWaitsForTheMatchingAcknowledgement(byte status, SwapStatus expected)
    {
        var (client, _, engine) = await ConnectedAsync();
        await using var _ = client;
        var swap = client.SwapSigilUnitAsync("sigils/basic_bolt.sigil", new byte[] { 1, 2, 3, 4 }, TimeSpan.FromSeconds(10), Timeout());

        var request = await FakeEngine.ReadFrameAsync(engine, new FrameDecoder(), Timeout());
        var payload = Assert.IsType<Message<SwapSigilUnit>>(Message.FromFrame(request)).Payload;
        Assert.Equal("sigils/basic_bolt.sigil", payload.UnitPath);
        Assert.Equal((uint)2, request.Seq);
        await FakeEngine.SendAsync(engine, Message.Of(new SwapAck { InReplyTo = request.Seq + 100, Status = 0 }), 2);
        var ack = new SwapAck { InReplyTo = request.Seq, Status = status, AppliedTick = 42, Reason = status == 1 ? "unit id mismatch" : string.Empty };
        await FakeEngine.SendAsync(engine, Message.Of(ack), 3);

        var result = await swap;
        Assert.Equal(expected, result.Status);
        Assert.Equal(ack, result.Ack);
    }

    [Fact]
    public async Task ASwapAnsweredWithAnErrorReportsIt()
    {
        var (client, _, engine) = await ConnectedAsync();
        await using var _ = client;
        var swap = client.SwapSigilUnitAsync("sigils/a.sigil", new byte[] { 1 }, TimeSpan.FromSeconds(10), Timeout());
        var request = await FakeEngine.ReadFrameAsync(engine, new FrameDecoder(), Timeout());
        await FakeEngine.SendAsync(engine, Message.Of(new ErrorMsg { Code = ErrorCode.Malformed, InReplyTo = request.Seq, Message = "bad unit" }), 2);

        var result = await swap;
        Assert.Equal(SwapStatus.EngineError, result.Status);
        Assert.Equal("bad unit", result.Error!.Message);
    }

    [Fact]
    public async Task ASwapWithoutAnswerTimesOutAndOneWhoseLinkDropsIsLost()
    {
        var (client, _, engine) = await ConnectedAsync();
        await using var _ = client;
        Assert.Equal(SwapStatus.TimedOut, (await client.SwapSigilUnitAsync("sigils/a.sigil", new byte[] { 1 }, TimeSpan.FromMilliseconds(50), Timeout())).Status);

        var lost = client.SwapSigilUnitAsync("sigils/a.sigil", new byte[] { 1 }, TimeSpan.FromSeconds(10), Timeout());
        var decoder = new FrameDecoder();
        Assert.Equal((uint)2, (await FakeEngine.ReadFrameAsync(engine, decoder, Timeout())).Seq);
        Assert.Equal((uint)3, (await FakeEngine.ReadFrameAsync(engine, decoder, Timeout())).Seq);
        await engine.DisposeAsync();
        Assert.Equal(SwapStatus.ConnectionLost, (await lost).Status);
    }

    [Fact]
    public async Task StoppingEndsTheLinkAndTheClientStaysStopped()
    {
        var (client, _, engine) = await ConnectedAsync();
        await client.StopAsync();
        Assert.Equal(LinkState.Stopped, client.State);
        Assert.Throws<ObjectDisposedException>(() => client.Start());
        Assert.Equal(0, await engine.ReadAsync(new byte[16], Timeout()));
        Assert.False(await client.Messages.WaitToReadAsync(Timeout()));
        await client.DisposeAsync();
    }

    [Theory]
    [InlineData("127.0.0.1:47474", true, 47474)]
    [InlineData("127.0.0.1:1", true, 1)]
    [InlineData("127.0.0.1:0", false, 0)]
    [InlineData("127.0.0.1:65536", false, 0)]
    [InlineData("127.0.0.1:", false, 0)]
    [InlineData("127.0.0.1:+80", false, 0)]
    [InlineData("0.0.0.0:47474", false, 0)]
    [InlineData("127.0.0.2:47474", false, 0)]
    [InlineData("localhost:47474", false, 0)]
    [InlineData("[::1]:47474", false, 0)]
    public void OnlyTheIpv4LoopbackAddressIsAccepted(string text, bool valid, int port)
    {
        Assert.Equal(valid, TcpLinkConnector.TryParseAddress(text, out var parsed, out var error));
        Assert.Equal(port, parsed);
        Assert.Equal(valid, error is null);
    }

    [Fact]
    public void TokensAreExactly64HexDigits()
    {
        Assert.Equal(FakeEngine.Filled(32, 0xAB), LiveLinkOptions.ParseToken(string.Concat(Enumerable.Repeat("aB", 32))));
        Assert.Throws<FormatException>(() => LiveLinkOptions.ParseToken(new string('a', 63)));
        Assert.Throws<FormatException>(() => LiveLinkOptions.ParseToken(new string('g', 64)));
    }
}
