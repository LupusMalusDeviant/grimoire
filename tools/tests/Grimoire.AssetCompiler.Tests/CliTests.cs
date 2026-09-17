using System;
using System.IO;
using System.Threading.Tasks;
using Grimoire.Formats;
using Xunit;

namespace Grimoire.AssetCompiler.Tests;

/// <summary>
/// The command line: the option grammar and the three exit codes of <c>sigilc</c> (sigil.md §13.1), so
/// the game's CI and an agent drive both tools the same way. Nothing here starts a compiler or opens a
/// socket; every case is decided before that.
/// </summary>
public sealed class CliTests
{
    [Fact]
    public async Task HelpAndVersionSucceedAndNameTheEngineBuild()
    {
        var (code, stdout, _) = await RunAsync("--help");
        Assert.Equal(Cli.ExitSuccess, code);
        Assert.Contains("grimoire-ac build <content-dir>", stdout, StringComparison.Ordinal);

        var (versionCode, version, _) = await RunAsync("--version");
        Assert.Equal(Cli.ExitSuccess, versionCode);
        Assert.Equal($"grimoire-ac {EngineBuild.EngineVersion} (engine build {EngineBuild.BuildHash})", version.Trim());
    }

    [Fact]
    public async Task NoArgumentsPrintsTheUsageAndFails()
    {
        var (code, stdout, stderr) = await RunAsync();
        Assert.Equal(Cli.ExitUsage, code);
        Assert.Empty(stdout);
        Assert.Contains("grimoire-ac build <content-dir>", stderr, StringComparison.Ordinal);
    }

    [Theory]
    [InlineData(new[] { "compile", "content" }, "unknown command")]
    [InlineData(new[] { "build" }, "needs <content-dir>")]
    [InlineData(new[] { "build", "a", "b" }, "takes one <content-dir>")]
    [InlineData(new[] { "build", "content", "--nonsense" }, "unknown option `--nonsense`")]
    [InlineData(new[] { "build", "content", "--out", "a", "--out", "b" }, "given more than once")]
    [InlineData(new[] { "build", "content", "--out" }, "needs a value")]
    [InlineData(new[] { "build", "content", "--json=yes" }, "takes no value")]
    [InlineData(new[] { "build", "content", "--address", "127.0.0.1:7878" }, "does not belong to this command")]
    [InlineData(new[] { "watch", "content" }, "needs `--push`")]
    [InlineData(new[] { "watch", "--push", "content", "--debounce", "-5" }, "--debounce")]
    [InlineData(new[] { "watch", "--push", "content", "--address", "localhost:7878" }, "loopback")]
    [InlineData(new[] { "watch", "--push", "content", "--address", "127.0.0.1:7878", "--token", "abc" }, "--token")]
    [InlineData(new[] { "push", "a.sigil", "--pack", "x" }, "does not belong to this command")]
    public async Task EveryUsageErrorEndsWithExitCodeTwoAndSaysWhat(string[] arguments, string expected)
    {
        var (code, stdout, stderr) = await RunAsync(arguments);
        Assert.Equal(Cli.ExitUsage, code);
        Assert.Empty(stdout);
        Assert.Contains(expected, stderr, StringComparison.Ordinal);
    }

    [Fact]
    public async Task AMissingCompilerBinaryIsSaidPlainlyAndIsNoCrash()
    {
        using var temp = new TempDirectory();
        temp.File("a.sigil", "sigil 1\n");
        var (code, _, stderr) = await RunAsync("build", temp.Path, "--sigilc", Path.Combine(temp.Path, "no-sigilc"));
        Assert.Equal(Cli.ExitUsage, code);
        Assert.Contains("no sigilc binary at", stderr, StringComparison.Ordinal);
    }

    [Fact]
    public async Task AMissingContentRootIsSaidBeforeAnythingIsCompiledOrConnected()
    {
        using var temp = new TempDirectory();
        var (code, _, stderr) = await RunAsync("watch", "--push", Path.Combine(temp.Path, "gone"), "--token", new string('a', 64));
        Assert.Equal(Cli.ExitUsage, code);
        Assert.Contains("does not exist", stderr, StringComparison.Ordinal);
    }

    [Fact]
    public async Task WithoutATokenNothingIsConnectedAndTheReasonNamesTheVariable()
    {
        Assert.SkipWhen(
            Environment.GetEnvironmentVariable("GRIMOIRE_DEBUG_TOKEN") is { Length: > 0 },
            "a debug-link token is set in this environment, so the tool would connect");
        using var temp = new TempDirectory();
        var (code, _, stderr) = await RunAsync("watch", "--push", temp.Path);
        Assert.Equal(Cli.ExitUsage, code);
        Assert.Contains("GRIMOIRE_DEBUG_TOKEN", stderr, StringComparison.Ordinal);
    }

    [Fact]
    public async Task OptionsMayStandAnywhereAndInBothForms()
    {
        using var temp = new TempDirectory();
        var missing = Path.Combine(temp.Path, "no-sigilc");
        foreach (var arguments in new[]
        {
            new[] { "build", "--sigilc", missing, temp.Path },
            new[] { "build", $"--sigilc={missing}", temp.Path },
            new[] { "build", temp.Path, $"--sigilc={missing}", "--json", "--timings", "--allow-empty" },
        })
        {
            var (code, _, stderr) = await RunAsync(arguments);
            Assert.Equal(Cli.ExitUsage, code);
            Assert.Contains("no sigilc binary at", stderr, StringComparison.Ordinal);
        }
    }

    [Fact]
    public async Task DoubleDashEndsOptionParsing()
    {
        using var temp = new TempDirectory();
        var (code, _, stderr) = await RunAsync("build", "--", "--content-dir-that-starts-with-dashes");
        Assert.Equal(Cli.ExitUsage, code);
        Assert.Contains("does not exist", stderr, StringComparison.Ordinal);
    }

    private static async Task<(int Code, string Stdout, string Stderr)> RunAsync(params string[] arguments)
    {
        var stdout = new StringWriter();
        var stderr = new StringWriter();
        var code = await Cli.RunAsync(arguments, stdout, stderr, TestContext.Current.CancellationToken);
        return (code, stdout.ToString(), stderr.ToString());
    }
}
