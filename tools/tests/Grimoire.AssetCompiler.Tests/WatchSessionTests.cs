using System;
using System.Collections.Concurrent;
using System.IO;
using System.Linq;
using System.Text.Json.Nodes;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;
using Grimoire.Formats.Debug;
using Grimoire.Formats.Pack;
using Grimoire.LiveLink;
using Grimoire.Tools.Tests;
using Xunit;

namespace Grimoire.AssetCompiler.Tests;

/// <summary>
/// <c>watch --push</c> (Plan 0002 WP9.2) against the contract-§13 engine double over in-memory streams:
/// one changed file is compiled alone and swapped into the running engine, a diagnostic keeps the engine's
/// unit, and a missing link degrades to file work (PRD-0016 FR-03). No socket is opened here.
/// </summary>
public sealed class WatchSessionTests
{
    private static CancellationToken Timeout()
    {
        var source = CancellationTokenSource.CreateLinkedTokenSource(TestContext.Current.CancellationToken);
        source.CancelAfter(TimeSpan.FromSeconds(20));
        return source.Token;
    }

    private static async Task<(LiveLinkClient Client, Stream Engine)> ConnectedAsync()
    {
        var connector = new InMemoryLinkConnector();
        var client = new LiveLinkClient(connector, FakeEngine.Options());
        client.Start();
        var stream = await connector.AcceptAsync(Timeout());
        Assert.NotNull(await new FakeEngine().AcceptAsync(stream, Timeout()));
        await client.WaitForStateAsync(LinkState.Connected, Timeout());
        return (client, stream);
    }

    [Fact]
    public async Task AChangedFileIsCompiledAloneAndSwappedIntoTheEngine()
    {
        using var temp = new TempDirectory();
        var file = temp.File("sigil/ring.sigil", "sigil 1\n");
        temp.File("sigil/other.sigil", "sigil 1\n");
        var (client, engine) = await ConnectedAsync();
        await using var _ = client;
        var sigilc = new FakeSigilc();
        var output = new StringWriter();
        var session = new WatchSession(WatchOptions(temp), sigilc, client, output);

        var push = session.CompileAndPushAsync(file, Timeout());
        var request = await FakeEngine.ReadFrameAsync(engine, new FrameDecoder(), Timeout());
        var payload = Assert.IsType<Message<SwapSigilUnit>>(Message.FromFrame(request)).Payload;
        Assert.Equal("sigil/ring.sigil", payload.UnitPath);
        Assert.Equal(
            await File.ReadAllBytesAsync(Path.Combine(temp.Path, "packs", "units", "sigil", "ring.unit"), Timeout()),
            payload.UnitBytes);
        await FakeEngine.SendAsync(engine, Message.Of(new SwapAck { InReplyTo = request.Seq, Status = 0, AppliedTick = 4711, ContentManifest = 0x00ff }), 2);

        var outcome = await push;
        Assert.Equal(WatchOutcomeKind.Pushed, outcome.Kind);
        Assert.Equal(SwapStatus.Applied, outcome.Swap!.Status);
        Assert.Equal("sigil/ring.sigil", outcome.AssetPath);
        Assert.Contains("applied at tick 4711", outcome.Line, StringComparison.Ordinal);

        // Only the changed file was compiled, not the whole content root.
        Assert.Equal(new[] { $"{temp.Path.Replace('\\', '/')}/sigil/ring.sigil" }, Assert.Single(sigilc.Invocations).ToArray());
    }

    [Fact]
    public async Task ADiagnosticKeepsTheUnitTheEngineHasAndPrintsTheTextForm()
    {
        using var temp = new TempDirectory();
        var file = temp.File("ring.sigil", "sigil 1\n");
        var (client, engine) = await ConnectedAsync();
        await using var _ = client;
        var sigilc = new FakeSigilc();
        var diagnostic = Diagnostic.FromJson(JsonNode.Parse("""
            {
              "code": "SIG0006",
              "severity": "error",
              "file": "ring.sigil",
              "line": 3,
              "column": 11,
              "node_path": "emitters.burst.delay",
              "token": "5s",
              "message": "Unknown unit `s` on `5s`.",
              "fix_hint": "Use one of the known units.",
              "related": null
            }
            """));
        sigilc.GiveDiagnostics($"{temp.Path.Replace('\\', '/')}/ring.sigil", diagnostic);
        var output = new StringWriter();
        var session = new WatchSession(WatchOptions(temp), sigilc, client, output);

        var outcome = await session.CompileAndPushAsync(file, Timeout());
        Assert.Equal(WatchOutcomeKind.Diagnostics, outcome.Kind);
        Assert.Null(outcome.Swap);
        Assert.Contains("ring.sigil:3:11: error[SIG0006]: Unknown unit `s` on `5s`.", output.ToString(), StringComparison.Ordinal);
        Assert.Contains("nothing pushed", outcome.Line, StringComparison.Ordinal);

        // Nothing reached the engine: no frame is waiting.
        var read = engine.ReadAsync(new byte[1], Timeout()).AsTask();
        Assert.False(read.IsCompleted);
    }

    [Fact]
    public async Task WithoutALinkTheCompiledUnitOnDiskStaysTheSourceOfTruth()
    {
        using var temp = new TempDirectory();
        var file = temp.File("ring.sigil", "sigil 1\n");
        var connector = new InMemoryLinkConnector { Refuse = true };
        await using var client = new LiveLinkClient(connector, FakeEngine.Options());
        client.Start();
        var output = new StringWriter();
        var session = new WatchSession(WatchOptions(temp), new FakeSigilc(), client, output);

        var outcome = await session.CompileAndPushAsync(file, Timeout());
        Assert.Equal(WatchOutcomeKind.Pushed, outcome.Kind);
        Assert.Equal(SwapStatus.NotConnected, outcome.Swap!.Status);
        Assert.Contains("no link, the unit on disk stays the source of truth", outcome.Line, StringComparison.Ordinal);
        Assert.True(File.Exists(Path.Combine(temp.Path, "packs", "units", "ring.unit")));
    }

    [Fact]
    public async Task ARejectedSwapIsReportedWithTheEnginesReason()
    {
        using var temp = new TempDirectory();
        var file = temp.File("ring.sigil", "sigil 1\n");
        var (client, engine) = await ConnectedAsync();
        await using var _ = client;
        var session = new WatchSession(WatchOptions(temp), new FakeSigilc(), client, new StringWriter());

        var push = session.CompileAndPushAsync(file, Timeout());
        var request = await FakeEngine.ReadFrameAsync(engine, new FrameDecoder(), Timeout());
        await FakeEngine.SendAsync(engine, Message.Of(new SwapAck { InReplyTo = request.Seq, Status = 1, Reason = "unit id mismatch" }), 2);
        var outcome = await push;
        Assert.Equal(SwapStatus.Rejected, outcome.Swap!.Status);
        Assert.Contains("rejected by the engine: unit id mismatch", outcome.Line, StringComparison.Ordinal);
    }

    [Fact]
    public async Task AFileThatIsGoneOrOutsideTheRootOrHasNoAssetPathPushesNothing()
    {
        using var temp = new TempDirectory();
        var outside = Path.Combine(Path.GetTempPath(), "grimoire-ac-outside.sigil");
        await File.WriteAllTextAsync(outside, "sigil 1\n", Timeout());
        try
        {
            var upper = temp.File("Ring.sigil", "sigil 1\n");
            var sigilc = new FakeSigilc();
            var session = new WatchSession(WatchOptions(temp), sigilc, null, new StringWriter());

            var gone = await session.CompileAndPushAsync(Path.Combine(temp.Path, "gone.sigil"), Timeout());
            Assert.Equal(WatchOutcomeKind.Skipped, gone.Kind);
            Assert.Contains("gone", gone.Line, StringComparison.Ordinal);

            var elsewhere = await session.CompileAndPushAsync(outside, Timeout());
            Assert.Equal(WatchOutcomeKind.Skipped, elsewhere.Kind);
            Assert.Contains("outside the content root", elsewhere.Line, StringComparison.Ordinal);

            var invalid = await session.CompileAndPushAsync(upper, Timeout());
            Assert.Equal(WatchOutcomeKind.Skipped, invalid.Kind);
            Assert.Contains("no asset path", invalid.Line, StringComparison.Ordinal);
            Assert.Empty(sigilc.Invocations);
        }
        finally
        {
            File.Delete(outside);
        }
    }

    [Fact]
    public async Task OneSaveThatArrivesAsSeveralEventsIsCompiledOnce()
    {
        using var temp = new TempDirectory();
        var file = temp.File("ring.sigil", "sigil 1\n");
        var sigilc = new FakeSigilc();
        var outcomes = new ConcurrentQueue<WatchOutcome>();
        var options = new WatchOptions(temp.Path)
        {
            OutputDirectory = Path.Combine(temp.Path, "packs"),
            Debounce = TimeSpan.FromMilliseconds(60),
        };
        var session = new WatchSession(options, sigilc, null, new StringWriter(), outcomes.Enqueue);
        var changes = Channel.CreateUnbounded<string>();
        var loop = session.RunAsync(changes.Reader, Timeout());

        changes.Writer.TryWrite(file);
        changes.Writer.TryWrite(file);
        changes.Writer.TryWrite(file);
        while (outcomes.Count == 0)
        {
            await Task.Delay(10, Timeout());
        }

        changes.Writer.Complete();
        await loop;
        Assert.Single(outcomes);
        Assert.Single(sigilc.Invocations);
    }

    [Fact]
    public async Task TheLoopStopsWhenItIsCancelledAndPrintsEveryOutcome()
    {
        using var temp = new TempDirectory();
        var first = temp.File("a.sigil", "sigil 1\n");
        var second = temp.File("b.sigil", "sigil 1\n");
        var sigilc = new FakeSigilc();
        var output = new StringWriter();
        var session = new WatchSession(WatchOptions(temp), sigilc, null, output);
        var changes = Channel.CreateUnbounded<string>();
        using var cancellation = CancellationTokenSource.CreateLinkedTokenSource(Timeout());
        var loop = session.RunAsync(changes.Reader, cancellation.Token);

        changes.Writer.TryWrite(first);
        changes.Writer.TryWrite(second);
        while (sigilc.Invocations.Count < 2)
        {
            await Task.Delay(10, Timeout());
        }

        await cancellation.CancelAsync();
        await loop;
        var lines = output.ToString().Split('\n', StringSplitOptions.RemoveEmptyEntries);
        Assert.Equal(2, lines.Length);
        Assert.Contains("a.sigil", lines[0], StringComparison.Ordinal);
        Assert.Contains("b.sigil", lines[1], StringComparison.Ordinal);
    }

    [Fact]
    public async Task ASwappedUnitIsTheSameByteSequenceThePackWouldCarry()
    {
        using var temp = new TempDirectory();
        var file = temp.File("ring.sigil", "sigil 1\n");
        var (client, engine) = await ConnectedAsync();
        await using var _ = client;
        var session = new WatchSession(WatchOptions(temp), new FakeSigilc(), client, new StringWriter());

        var push = session.CompileAndPushAsync(file, Timeout());
        var request = await FakeEngine.ReadFrameAsync(engine, new FrameDecoder(), Timeout());
        var payload = Assert.IsType<Message<SwapSigilUnit>>(Message.FromFrame(request)).Payload;
        await FakeEngine.SendAsync(engine, Message.Of(new SwapAck { InReplyTo = request.Seq, Status = 0 }), 2);
        await push;

        var build = await BuildPipeline.RunAsync(
            new BuildOptions(temp.Path) { OutputDirectory = Path.Combine(temp.Path, "packs-full") },
            new FakeSigilc(),
            Timeout());
        var pack = PackReader.FromBytes(await File.ReadAllBytesAsync(build.PackPath!, Timeout()));
        var entry = Assert.Single(pack.Entries);
        Assert.Equal(payload.UnitBytes, pack.Read(entry.Id).ToArray());
        Assert.Equal(AssetId.FromPath(AssetPath.Create("ring.sigil")), entry.Id);
    }

    private static WatchOptions WatchOptions(TempDirectory temp) =>
        new(temp.Path)
        {
            OutputDirectory = Path.Combine(temp.Path, "packs"),
            Debounce = TimeSpan.Zero,
            SwapTimeout = TimeSpan.FromSeconds(10),
        };
}
