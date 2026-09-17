using System;
using System.Net;
using System.Net.Sockets;
using System.Threading;
using System.Threading.Tasks;
using Grimoire.Formats.Debug;
using Xunit;

namespace Grimoire.LiveLink.Tests;

/// <summary>
/// <see cref="TcpLinkConnector"/> over real loopback sockets. Like the engine's socket tests (contract §13
/// "Socket-Tests"), these run only with <c>GRIMOIRE_SOCKET_TESTS=1</c>, which only CI sets; everywhere else
/// they are skipped before anything binds a port.
/// </summary>
public sealed class TcpLinkTests
{
    private const string SkipReason = "real sockets only with GRIMOIRE_SOCKET_TESTS=1 (contract §13), which only CI sets";

    private static CancellationToken Timeout()
    {
        var source = CancellationTokenSource.CreateLinkedTokenSource(TestContext.Current.CancellationToken);
        source.CancelAfter(TimeSpan.FromSeconds(20));
        return source.Token;
    }

    [Fact]
    public async Task HandshakeMessagesSwapAndReconnectOverLoopbackSockets()
    {
        Assert.SkipUnless(DebugProtocol.SocketTestsEnabled(), SkipReason);
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        try
        {
            var port = ((IPEndPoint)listener.LocalEndpoint).Port;
            await using var client = new LiveLinkClient(new TcpLinkConnector(port), FakeEngine.Options());
            client.Start();

            Task disconnected;
            using (var first = await listener.AcceptTcpClientAsync(Timeout()))
            {
                var stream = first.GetStream();
                Assert.NotNull(await new FakeEngine().AcceptAsync(stream, Timeout()));
                await client.WaitForStateAsync(LinkState.Connected, Timeout());

                await FakeEngine.SendAsync(stream, Message.Of(new Stats { Frame = 3 }), 2);
                Assert.Equal(3UL, Assert.IsType<Message<Stats>>(await client.Messages.ReadAsync(Timeout())).Payload.Frame);

                var swap = client.SwapSigilUnitAsync("sigils/basic_bolt.sigil", new byte[] { 1, 2, 3 }, TimeSpan.FromSeconds(10), Timeout());
                var request = await FakeEngine.ReadFrameAsync(stream, new FrameDecoder(), Timeout());
                await FakeEngine.SendAsync(stream, Message.Of(new SwapAck { InReplyTo = request.Seq, Status = 0, AppliedTick = 7 }), 3);
                Assert.Equal(SwapStatus.Applied, (await swap).Status);

                disconnected = client.WaitForStateAsync(LinkState.Disconnected, Timeout());
            }

            await disconnected;
            using var second = await listener.AcceptTcpClientAsync(Timeout());
            Assert.NotNull(await new FakeEngine().AcceptAsync(second.GetStream(), Timeout()));
            await client.WaitForStateAsync(LinkState.Connected, Timeout());
        }
        finally
        {
            listener.Stop();
        }
    }

    [Fact]
    public async Task AnEngineThatNeverAnswersTheHandshakeTimesOutAndIsRetried()
    {
        Assert.SkipUnless(DebugProtocol.SocketTestsEnabled(), SkipReason);
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        try
        {
            var port = ((IPEndPoint)listener.LocalEndpoint).Port;
            var options = new LiveLinkOptions(FakeEngine.Identity())
            {
                HandshakeTimeout = TimeSpan.FromMilliseconds(200),
                InitialReconnectDelay = TimeSpan.FromMilliseconds(10),
                MaxReconnectDelay = TimeSpan.FromMilliseconds(20),
            };
            await using var client = new LiveLinkClient(new TcpLinkConnector(port), options);
            var timedOut = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
            client.StateChanged += (_, args) =>
            {
                if (args.State == LinkState.Disconnected && args.Reason is { } reason && reason.Contains("in time", StringComparison.Ordinal))
                {
                    timedOut.TrySetResult();
                }
            };
            client.Start();

            using var silent = await listener.AcceptTcpClientAsync(Timeout());
            await timedOut.Task.WaitAsync(Timeout());
            using var answering = await listener.AcceptTcpClientAsync(Timeout());
            Assert.NotNull(await new FakeEngine().AcceptAsync(answering.GetStream(), Timeout()));
            await client.WaitForStateAsync(LinkState.Connected, Timeout());
        }
        finally
        {
            listener.Stop();
        }
    }
}
