using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.Json.Nodes;
using System.Threading;
using System.Threading.Tasks;
using Grimoire.Formats.Pack;
using Xunit;

namespace Grimoire.AssetCompiler.Tests;

/// <summary>
/// The WP4.1 conformance corpus through the asset compiler (Plan 0002 WP9.2): the same
/// <c>.sigil</c> files and the same committed expectations the engine's own corpus test uses, but driven
/// through <c>grimoire-ac</c> and the real <c>sigilc</c> binary.
/// </summary>
/// <remarks>
/// This is where "diagnostics travel through unchanged" (project ADR-0010 building block 4) stops being a
/// claim: every field of every expected diagnostic is compared, and the text form is compared against
/// what <c>sigilc</c> itself prints. The tests need the binary; without one they are skipped, unless
/// <c>GRIMOIRE_REQUIRE_SIGILC=1</c> — which the CI sets, so a missing binary can never make the gate pass
/// quietly.
/// </remarks>
public sealed class CorpusConformanceTests
{
    private const string RequireVariable = "GRIMOIRE_REQUIRE_SIGILC";

    private static string CorpusRoot => Path.Combine(AppContext.BaseDirectory, "fixtures", "corpus");

    private static string ReferenceRoot => Path.Combine(AppContext.BaseDirectory, "fixtures", "reference");

    private static CancellationToken Token() => TestContext.Current.CancellationToken;

    private static SigilcProcess RequireCompiler()
    {
        string? path = null;
        try
        {
            path = SigilcProcess.Locate(null);
        }
        catch (AssetCompilerException)
        {
            if (string.Equals(Environment.GetEnvironmentVariable(RequireVariable), "1", StringComparison.Ordinal))
            {
                Assert.Fail(
                    $"{RequireVariable}=1, but no sigilc binary was found: build it with "
                    + "`cargo build --release -p grimoire_sigilc` and point GRIMOIRE_SIGILC at it.");
            }

            Assert.Skip("no sigilc binary: set GRIMOIRE_SIGILC or put it on PATH (the CI always does)");
        }

        return new SigilcProcess(path!);
    }

    [Fact]
    public async Task EveryValidCorpusFileCompilesAndBecomesAPackEntryWithItsUnitId()
    {
        var sigilc = RequireCompiler();
        using var temp = new TempDirectory();
        var result = await BuildPipeline.RunAsync(
            new BuildOptions(Path.Combine(CorpusRoot, "valid"))
            {
                OutputDirectory = temp.Path,
                Behaviors = Path.Combine(CorpusRoot, "behaviors.json"),
                AllowVersionMismatch = true,
            },
            sigilc,
            Token());

        Assert.True(result.Ok, string.Join('\n', result.AllDiagnostics().Select(static d => d.RenderText())));
        Assert.Equal(5, result.Units.Count);
        Assert.NotEmpty(result.Skipped);
        var pack = PackReader.FromBytes(await File.ReadAllBytesAsync(result.PackPath!, Token()));
        Assert.Equal(result.Units.Count, pack.Entries.Count);
        foreach (var unit in result.Units)
        {
            Assert.Equal(unit.AssetId, unit.UnitId);
            var id = AssetId.FromPath(AssetPath.Create(unit.AssetPath));
            Assert.Equal(unit.AssetPath, pack.PathOf(id)!.Value);
            Assert.Equal(1u, pack.Entries.Single(entry => entry.Id == id).KindVersion);
            Assert.Empty(ExpectedDiagnostics(Path.Combine(CorpusRoot, "valid"), unit.AssetPath));
        }
    }

    [Theory]
    [InlineData("invalid", false)]
    [InlineData("schema-invalid", true)]
    public async Task EveryInvalidCorpusFileFailsWithExactlyTheExpectedDiagnostics(string directory, bool exactly)
    {
        var sigilc = RequireCompiler();
        using var temp = new TempDirectory();
        var root = Path.Combine(CorpusRoot, directory);
        var result = await BuildPipeline.RunAsync(
            new BuildOptions(root)
            {
                OutputDirectory = temp.Path,
                Behaviors = Path.Combine(CorpusRoot, "behaviors.json"),
                AllowVersionMismatch = true,
            },
            sigilc,
            Token());

        Assert.False(result.Ok);
        Assert.Null(result.PackPath);
        var compared = 0;
        foreach (var unit in result.Units)
        {
            var expected = ExpectedDiagnostics(root, unit.AssetPath, required: false);
            if (expected is null)
            {
                // Only ever imported by another fixture (e.g. the other half of an import cycle).
                continue;
            }

            compared++;

            // The `invalid/` expectations are the parser's (the engine's WP4.1 corpus test compares them
            // against `parse`), and `build` runs the schema pass on top, which may add the consequences of
            // a broken parse. So every expected diagnostic must arrive first and unchanged; for
            // `schema-invalid/`, whose expectations are the compiler's own, the list must match exactly.
            Assert.True(
                unit.Diagnostics.Count >= expected.Count,
                $"{unit.AssetPath}: {unit.Diagnostics.Count.ToString(System.Globalization.CultureInfo.InvariantCulture)} diagnostics, "
                + $"expected at least {expected.Count.ToString(System.Globalization.CultureInfo.InvariantCulture)}");
            if (exactly)
            {
                Assert.Equal(expected.Count, unit.Diagnostics.Count);
            }

            for (var index = 0; index < expected.Count; index++)
            {
                AssertSameDiagnostic(expected[index], unit.Diagnostics[index]);
            }
        }

        Assert.True(compared >= 8, $"only {compared.ToString(System.Globalization.CultureInfo.InvariantCulture)} files of `{directory}` were compared");
    }

    [Fact]
    public async Task TheTextFormIsWhatSigilcItselfPrints()
    {
        var sigilc = RequireCompiler();
        using var temp = new TempDirectory();
        var root = Path.Combine(CorpusRoot, "invalid");
        var files = Directory.GetFiles(root, "*.sigil").OrderBy(static file => file, StringComparer.Ordinal).ToList();
        Assert.NotEmpty(files);
        foreach (var file in files)
        {
            var compiled = await sigilc.BuildAsync(root, temp.Path, null, new[] { file }, Token());
            var ours = string.Join('\n', Assert.Single(compiled.Units).Diagnostics.Select(static d => d.RenderText()));
            var (exitCode, stdout, _) = await sigilc.InvokeAsync(new List<string> { "check", "--root", root, "--", file }, Token());
            Assert.Equal(1, exitCode);
            Assert.Equal(stdout.Replace("\r\n", "\n", StringComparison.Ordinal).TrimEnd('\n'), ours);
        }
    }

    [Fact]
    public async Task TheReferencePatternsGiveTheSamePackBytesOnEveryRun()
    {
        var sigilc = RequireCompiler();
        using var temp = new TempDirectory();
        async Task<BuildResult> BuildAsync(string output) => await BuildPipeline.RunAsync(
            new BuildOptions(ReferenceRoot)
            {
                OutputDirectory = Path.Combine(temp.Path, output),
                Behaviors = Path.Combine(ReferenceRoot, "behaviors.json"),
                AllowVersionMismatch = true,
            },
            sigilc,
            Token());

        var first = await BuildAsync("first");
        Assert.True(first.Ok, string.Join('\n', first.AllDiagnostics().Select(static d => d.RenderText())));
        Assert.Equal(12, first.Units.Count);
        var second = await BuildAsync("second");
        Assert.Equal(
            await File.ReadAllBytesAsync(first.PackPath!, Token()),
            await File.ReadAllBytesAsync(second.PackPath!, Token()));
        Assert.Equal(first.PackContentHash, second.PackContentHash);
    }

    [Fact]
    public async Task ACompilerOfAnotherVersionIsReportedAgainstTheEngineVersionOfThisBuild()
    {
        var sigilc = RequireCompiler();
        using var temp = new TempDirectory();
        var result = await BuildPipeline.RunAsync(
            new BuildOptions(Path.Combine(CorpusRoot, "valid"))
            {
                OutputDirectory = temp.Path,
                Behaviors = Path.Combine(CorpusRoot, "behaviors.json"),
                EngineVersion = "0.0.0-not-this-engine",
            },
            sigilc,
            Token());
        Assert.False(result.Ok);
        Assert.Equal(AcCode.CompilerVersionMismatch, Assert.Single(result.Diagnostics).Code);
        Assert.Equal(sigilc.Version, result.CompilerVersion);
    }

    private static void AssertSameDiagnostic(JsonObject expected, Diagnostic actual)
    {
        var json = actual.ToJson().AsObject();
        foreach (var key in new[] { "code", "severity", "line", "column", "node_path", "token", "message", "fix_hint" })
        {
            Assert.Equal(expected[key]?.ToJsonString(), json[key]?.ToJsonString());
        }

        Assert.Equal(expected["related"]?.ToJsonString(), json["related"]?.ToJsonString());

        // The corpus names files relative to the corpus root, the asset compiler as they stood on the
        // command line; the file itself must be the same one.
        Assert.EndsWith(
            Path.GetFileName(expected["file"]!.GetValue<string>()),
            json["file"]!.GetValue<string>(),
            StringComparison.Ordinal);
    }

    private static List<JsonObject> ExpectedDiagnostics(string root, string assetPath) =>
        ExpectedDiagnostics(root, assetPath, required: true)!;

    private static List<JsonObject>? ExpectedDiagnostics(string root, string assetPath, bool required)
    {
        var sidecar = Path.Combine(root, Path.ChangeExtension(assetPath, ".expected.json"));
        if (!File.Exists(sidecar))
        {
            return required ? throw new InvalidOperationException($"no expectation beside {assetPath}") : null;
        }

        var document = JsonNode.Parse(File.ReadAllText(sidecar))!.AsObject();
        Assert.Equal(1, document["schema_version"]!.GetValue<int>());
        return document["diagnostics"]!.AsArray().Select(static node => node!.AsObject()).ToList();
    }
}
