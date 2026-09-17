using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json.Nodes;
using System.Threading.Tasks;
using Grimoire.Formats;
using Grimoire.Formats.Pack;
using Xunit;

namespace Grimoire.AssetCompiler.Tests;

/// <summary>
/// The build: orchestration, pack and manifest, and the rule that every diagnostic stops the pack
/// (Plan 0002 WP9.2, WP9.3 "every diagnostic fails the run").
/// </summary>
public sealed class BuildPipelineTests
{
    [Fact]
    public async Task AGreenBuildWritesAPackWithOneEntryPerSource()
    {
        using var temp = new TempDirectory();
        temp.File("sigil/ring.sigil", "sigil 1\n");
        temp.File("sigil/waves/fan.sigil", "sigil 1\n");
        var sigilc = new FakeSigilc();
        var result = await BuildPipeline.RunAsync(Options(temp), sigilc, TestContext.Current.CancellationToken);

        Assert.True(result.Ok);
        Assert.Empty(result.AllDiagnostics());
        Assert.Equal(new[] { "sigil/ring.sigil", "sigil/waves/fan.sigil" }, result.Units.Select(static unit => unit.AssetPath).ToArray());
        Assert.NotNull(result.PackPath);
        var pack = PackReader.FromBytes(await File.ReadAllBytesAsync(result.PackPath!, TestContext.Current.CancellationToken));
        Assert.Equal(BuildPipeline.CompilerName, pack.Compiler);
        Assert.Equal(sigilc.Version, pack.CompilerVersion);
        Assert.Equal(2, pack.Entries.Count);
        foreach (var unit in result.Units)
        {
            var id = AssetId.FromPath(AssetPath.Create(unit.AssetPath));
            Assert.Equal(unit.AssetPath, pack.PathOf(id)!.Value);
            var entry = pack.Entries.Single(e => e.Id == id);
            Assert.Equal(AssetKind.Sigil.Value, entry.Kind.Value);
            Assert.Equal(1u, entry.KindVersion);
            Assert.Equal((ulong)unit.Size!.Value, entry.Length);
        }
    }

    [Fact]
    public async Task ThePackEntryIdIsTheUnitIdSoTheEngineFindsWhatTheCompilerWrote()
    {
        using var temp = new TempDirectory();
        temp.File("ring.sigil", "sigil 1\n");
        var result = await BuildPipeline.RunAsync(Options(temp), new FakeSigilc(), TestContext.Current.CancellationToken);
        var unit = Assert.Single(result.Units);
        Assert.Equal(unit.AssetId, unit.UnitId);
        var pack = PackReader.FromBytes(await File.ReadAllBytesAsync(result.PackPath!, TestContext.Current.CancellationToken));
        Assert.Equal(unit.AssetId, Assert.Single(pack.Entries).Id.ToString());
    }

    [Fact]
    public async Task TheSameContentGivesTheSamePackBytesTwice()
    {
        using var temp = new TempDirectory();
        temp.File("a.sigil", "sigil 1\n");
        temp.File("b.sigil", "sigil 1\n");
        var first = await BuildPipeline.RunAsync(Options(temp, "packs-1"), new FakeSigilc(), TestContext.Current.CancellationToken);
        var second = await BuildPipeline.RunAsync(Options(temp, "packs-2"), new FakeSigilc(), TestContext.Current.CancellationToken);
        Assert.Equal(
            await File.ReadAllBytesAsync(first.PackPath!, TestContext.Current.CancellationToken),
            await File.ReadAllBytesAsync(second.PackPath!, TestContext.Current.CancellationToken));
        Assert.Equal(first.PackContentHash, second.PackContentHash);
        Assert.Matches("^[0-9a-f]{64}$", first.PackContentHash!);
    }

    [Fact]
    public async Task ADiagnosticStopsThePackAndTravelsThroughUnchanged()
    {
        using var temp = new TempDirectory();
        temp.File("ring.sigil", "sigil 1\n");
        temp.File("fan.sigil", "sigil 1\n");
        var sigilc = new FakeSigilc();
        var raw = JsonNode.Parse("""
            {
              "code": "SIG0008",
              "severity": "error",
              "file": "ring.sigil",
              "line": 6,
              "column": 10,
              "node_path": "emitters.burst.block.count",
              "token": ":",
              "message": "Expected `=` after the field name, found `:`.",
              "fix_hint": "Use `=` to assign the value.",
              "related": null
            }
            """)!.AsObject();
        sigilc.GiveDiagnostics($"{temp.Path.Replace('\\', '/')}/ring.sigil", Diagnostic.FromJson(raw.DeepClone()));
        var result = await BuildPipeline.RunAsync(Options(temp), sigilc, TestContext.Current.CancellationToken);

        Assert.False(result.Ok);
        Assert.Null(result.PackPath);
        Assert.Null(result.PackContentHash);
        var diagnostic = Assert.Single(result.AllDiagnostics());
        Assert.Equal(raw.ToJsonString(), diagnostic.ToJson().ToJsonString());
        var reported = result.ToJson()["units"]!.AsArray()
            .Single(unit => unit!["asset_path"]!.GetValue<string>() == "ring.sigil")!;
        Assert.Equal(raw.ToJsonString(), Assert.Single(reported["diagnostics"]!.AsArray())!.ToJsonString());
        Assert.Null(reported["unit_id"]);
    }

    [Fact]
    public async Task AnEmptyContentRootIsAcZeroZeroZeroSixUnlessItIsAllowed()
    {
        using var temp = new TempDirectory();
        var refused = await BuildPipeline.RunAsync(Options(temp), new FakeSigilc(), TestContext.Current.CancellationToken);
        Assert.False(refused.Ok);
        Assert.Equal(AcCode.NoSources, Assert.Single(refused.Diagnostics).Code);
        Assert.Null(refused.PackPath);

        var allowed = await BuildPipeline.RunAsync(
            new BuildOptions(temp.Path) { OutputDirectory = Path.Combine(temp.Path, "packs"), AllowEmpty = true },
            new FakeSigilc(),
            TestContext.Current.CancellationToken);
        Assert.True(allowed.Ok);
        var pack = PackReader.FromBytes(await File.ReadAllBytesAsync(allowed.PackPath!, TestContext.Current.CancellationToken));
        Assert.Empty(pack.Entries);
    }

    [Fact]
    public async Task ACompilerOfAnotherVersionIsAcZeroZeroZeroFiveUnlessItIsAllowed()
    {
        using var temp = new TempDirectory();
        temp.File("a.sigil", "sigil 1\n");
        var sigilc = new FakeSigilc { Version = "0.0.1-other" };
        var refused = await BuildPipeline.RunAsync(Options(temp), sigilc, TestContext.Current.CancellationToken);
        Assert.False(refused.Ok);
        var diagnostic = Assert.Single(refused.Diagnostics);
        Assert.Equal(AcCode.CompilerVersionMismatch, diagnostic.Code);
        Assert.Contains(EngineBuild.EngineVersion, diagnostic.Message, StringComparison.Ordinal);

        var allowed = await BuildPipeline.RunAsync(
            new BuildOptions(temp.Path) { OutputDirectory = Path.Combine(temp.Path, "packs"), AllowVersionMismatch = true },
            sigilc,
            TestContext.Current.CancellationToken);
        Assert.True(allowed.Ok);
        var pack = PackReader.FromBytes(await File.ReadAllBytesAsync(allowed.PackPath!, TestContext.Current.CancellationToken));
        Assert.Equal("0.0.1-other", pack.CompilerVersion);
    }

    [Fact]
    public async Task AUnitWhoseIdIsNotItsAssetPathIsAcZeroZeroZeroThree()
    {
        using var temp = new TempDirectory();
        temp.File("a.sigil", "sigil 1\n");
        var sigilc = new FakeSigilc();
        sigilc.GiveUnitId($"{temp.Path.Replace('\\', '/')}/a.sigil", 0x1234_5678_9abc_def0);
        var result = await BuildPipeline.RunAsync(Options(temp), sigilc, TestContext.Current.CancellationToken);
        Assert.False(result.Ok);
        Assert.Null(result.PackPath);
        var diagnostic = Assert.Single(Assert.Single(result.Units).Diagnostics);
        Assert.Equal(AcCode.UnitUnreadable, diagnostic.Code);
        Assert.Contains("not the asset id", diagnostic.Message, StringComparison.Ordinal);
    }

    [Fact]
    public async Task AMissingUnitFileIsAcZeroZeroZeroThreeInsteadOfAPackWithAHole()
    {
        using var temp = new TempDirectory();
        temp.File("a.sigil", "sigil 1\n");
        var result = await BuildPipeline.RunAsync(Options(temp), new FakeSigilc { WriteNoUnits = true }, TestContext.Current.CancellationToken);
        Assert.False(result.Ok);
        Assert.Equal(AcCode.UnitUnreadable, Assert.Single(Assert.Single(result.Units).Diagnostics).Code);
    }

    [Fact]
    public async Task TheApplicationBlockIsCarriedIntoTheManifest()
    {
        using var temp = new TempDirectory();
        temp.File("a.sigil", "sigil 1\n");
        var application = temp.File("application.bin", "fnp 0.1.0");
        var result = await BuildPipeline.RunAsync(
            new BuildOptions(temp.Path) { OutputDirectory = Path.Combine(temp.Path, "packs"), ApplicationFile = application },
            new FakeSigilc(),
            TestContext.Current.CancellationToken);
        var pack = PackReader.FromBytes(await File.ReadAllBytesAsync(result.PackPath!, TestContext.Current.CancellationToken));
        Assert.Equal("fnp 0.1.0", System.Text.Encoding.UTF8.GetString(pack.Application.Span));
    }

    [Fact]
    public async Task AnApplicationBlockAboveSixtyFourKibibytesStopsTheRun()
    {
        using var temp = new TempDirectory();
        temp.File("a.sigil", "sigil 1\n");
        var application = temp.File("application.bin", new string('x', BuildPipeline.MaxApplicationBytes + 1));
        var error = await Assert.ThrowsAsync<AssetCompilerException>(() => BuildPipeline.RunAsync(
            new BuildOptions(temp.Path) { OutputDirectory = Path.Combine(temp.Path, "packs"), ApplicationFile = application },
            new FakeSigilc(),
            TestContext.Current.CancellationToken));
        Assert.Contains("64 KiB", error.Message, StringComparison.Ordinal);
    }

    [Fact]
    public async Task TheReportDocumentHasAFixedShape()
    {
        using var temp = new TempDirectory();
        temp.File("ring.sigil", "sigil 1\n");
        temp.File("notes.md", "not a source\n");
        var result = await BuildPipeline.RunAsync(Options(temp), new FakeSigilc(), TestContext.Current.CancellationToken);
        var document = result.ToJson();
        Assert.Equal(
            new[]
            {
                "schema", "schema_version", "command", "ok", "content_root", "compiler", "compiler_version",
                "pack", "pack_bytes", "pack_content_hash", "units", "diagnostics", "skipped",
            },
            document.Select(static pair => pair.Key).ToArray());
        Assert.Equal("grimoire.ac.build", document["schema"]!.GetValue<string>());
        Assert.Equal(1, document["schema_version"]!.GetValue<int>());
        Assert.True(document["ok"]!.GetValue<bool>());
        var unit = Assert.Single(document["units"]!.AsArray())!.AsObject();
        Assert.Equal(
            new[] { "source", "asset_path", "asset_id", "unit_id", "content_hash", "size", "kind_version", "diagnostics" },
            unit.Select(static pair => pair.Key).ToArray());
        Assert.Matches("^[0-9a-f]{16}$", unit["asset_id"]!.GetValue<string>());
        var skipped = Assert.Single(document["skipped"]!.AsArray())!.AsObject();
        Assert.EndsWith("notes.md", skipped["path"]!.GetValue<string>(), StringComparison.Ordinal);
        Assert.False(document.ToJsonString().Contains("timings", StringComparison.Ordinal));
    }

    [Fact]
    public async Task LongFileListsAreCompiledInBatchesOfWholeFiles()
    {
        using var temp = new TempDirectory();
        var expected = new List<string>();
        for (var index = 0; index < SigilcProcess.MaxFilesPerInvocation + 5; index++)
        {
            temp.File($"p{index.ToString("D4", System.Globalization.CultureInfo.InvariantCulture)}.sigil", "sigil 1\n");
            expected.Add($"p{index.ToString("D4", System.Globalization.CultureInfo.InvariantCulture)}.sigil");
        }

        var batches = SigilcProcess.Batches(expected).ToList();
        Assert.Equal(2, batches.Count);
        Assert.Equal(SigilcProcess.MaxFilesPerInvocation, batches[0].Count);
        Assert.Equal(5, batches[1].Count);
        Assert.Equal(expected, batches.SelectMany(static batch => batch).ToList());

        var sigilc = new FakeSigilc();
        var result = await BuildPipeline.RunAsync(Options(temp), sigilc, TestContext.Current.CancellationToken);
        Assert.True(result.Ok);
        Assert.Equal(expected.Count, result.Units.Count);
        Assert.Single(sigilc.Invocations);
    }

    [Fact]
    public async Task TheSourcesAreHandedToTheCompilerWithTheRootTheUserWrote()
    {
        using var temp = new TempDirectory();
        temp.File("sigil/a.sigil", "sigil 1\n");
        var sigilc = new FakeSigilc();
        await BuildPipeline.RunAsync(Options(temp), sigilc, TestContext.Current.CancellationToken);
        var files = Assert.Single(sigilc.Invocations);
        Assert.Equal($"{temp.Path.Replace('\\', '/')}/sigil/a.sigil", Assert.Single(files));
    }

    private static BuildOptions Options(TempDirectory temp, string output = "packs") =>
        new(temp.Path) { OutputDirectory = Path.Combine(temp.Path, output) };
}
