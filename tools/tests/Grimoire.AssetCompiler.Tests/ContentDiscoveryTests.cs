using System;
using System.IO;
using System.Linq;
using Xunit;

namespace Grimoire.AssetCompiler.Tests;

/// <summary>
/// Content discovery (Plan 0002 WP9.2): it takes the tree as it is, expects nothing at a fixed place,
/// and turns file-system paths into asset paths or says exactly why it cannot.
/// </summary>
public sealed class ContentDiscoveryTests
{
    [Fact]
    public void EverySigilFileBelowTheRootIsASourceInAssetPathOrder()
    {
        using var temp = new TempDirectory();
        temp.File("sigil/waves/b.sigil", "sigil 1\n");
        temp.File("sigil/a.sigil", "sigil 1\n");
        temp.File("z.sigil", "sigil 1\n");
        var result = ContentDiscovery.Scan(temp.Path);
        Assert.Equal(
            new[] { "sigil/a.sigil", "sigil/waves/b.sigil", "z.sigil" },
            result.Sources.Select(static source => source.Path.Value).ToArray());
        Assert.Empty(result.Diagnostics);
        Assert.Empty(result.Skipped);
    }

    [Fact]
    public void TheCommandLinePathKeepsTheRootAsTheUserWroteIt()
    {
        using var temp = new TempDirectory();
        temp.File("sigil/a.sigil", "sigil 1\n");
        var relative = Path.GetRelativePath(Directory.GetCurrentDirectory(), temp.Path).Replace('\\', '/');
        var result = ContentDiscovery.Scan(relative);
        Assert.Equal($"{relative}/sigil/a.sigil", Assert.Single(result.Sources).CommandLinePath);
    }

    [Fact]
    public void EveryOtherFileIsReportedAsSkippedWithItsReason()
    {
        using var temp = new TempDirectory();
        temp.File("a.sigil", "sigil 1\n");
        temp.File("README.md", "notes\n");
        temp.File("a.expected.json", "{}\n");
        var result = ContentDiscovery.Scan(temp.Path);
        Assert.Equal("a.sigil", Assert.Single(result.Sources).Path.Value);
        Assert.Equal(2, result.Skipped.Count);
        Assert.All(result.Skipped, static file => Assert.Equal("not a Sigil source (`.sigil`)", file.Reason));
        Assert.Empty(result.Diagnostics);
    }

    [Fact]
    public void AnExtensionInAnotherCaseIsStillASourceSoNothingIsMissedSilently()
    {
        using var temp = new TempDirectory();
        temp.File("Ring.SIGIL", "sigil 1\n");
        var result = ContentDiscovery.Scan(temp.Path);
        Assert.Empty(result.Sources);
        var diagnostic = Assert.Single(result.Diagnostics);
        Assert.Equal(AcCode.InvalidPath, diagnostic.Code);
    }

    [Theory]
    [InlineData("Ring.sigil")]
    [InlineData("sigil/Waves/ring.sigil")]
    [InlineData("ring pattern.sigil")]
    [InlineData("kürbis.sigil")]
    public void APathThatIsNoAssetPathIsAcZeroZeroZeroOneWithTheRenameAsFixHint(string name)
    {
        using var temp = new TempDirectory();
        temp.File(name, "sigil 1\n");
        var result = ContentDiscovery.Scan(temp.Path);
        Assert.Empty(result.Sources);
        var diagnostic = Assert.Single(result.Diagnostics);
        Assert.Equal(AcCode.InvalidPath, diagnostic.Code);
        Assert.Contains("lowercase ASCII", diagnostic.FixHint, StringComparison.Ordinal);
        Assert.EndsWith(name.Replace('\\', '/'), diagnostic.File, StringComparison.Ordinal);
    }

    [Fact]
    public void TwoFilesWithOneAssetPathAreAcZeroZeroZeroTwoAndOnlyOneEntersThePack()
    {
        // No file system reports one path twice, so the collision is classified directly: the pack must
        // never silently carry one of two files under an id that names both.
        var result = ContentDiscovery.Classify("content", new[]
        {
            ("/first/a.sigil", "a.sigil"),
            ("/second/a.sigil", "a.sigil"),
        });
        Assert.Equal("a.sigil", Assert.Single(result.Sources).Path.Value);
        var diagnostic = Assert.Single(result.Diagnostics);
        Assert.Equal(AcCode.PathCollision, diagnostic.Code);
        Assert.Contains("content/a.sigil", diagnostic.Message, StringComparison.Ordinal);
    }

    [Fact]
    public void ABackslashInANameIsNoSeparatorButAnInvalidPath()
    {
        // On Windows such a name cannot exist; on Linux it can, and turning it into a separator would
        // invent a path that no file has.
        var result = ContentDiscovery.Classify("content", new[] { ("/content/a\\b.sigil", "a\\b.sigil") });
        Assert.Empty(result.Sources);
        Assert.Equal(AcCode.InvalidPath, Assert.Single(result.Diagnostics).Code);
    }

    [Fact]
    public void HiddenDirectoriesAndFilesAreLeftAlone()
    {
        using var temp = new TempDirectory();
        temp.File(".git/objects/a.sigil", "sigil 1\n");
        temp.File(".hidden.sigil", "sigil 1\n");
        temp.File("a.sigil", "sigil 1\n");
        var result = ContentDiscovery.Scan(temp.Path);
        Assert.Equal("a.sigil", Assert.Single(result.Sources).Path.Value);
        Assert.Empty(result.Skipped);
    }

    [Fact]
    public void TheOutputDirectoryBelowTheRootIsNotScanned()
    {
        using var temp = new TempDirectory();
        temp.File("a.sigil", "sigil 1\n");
        temp.File("packs/units/a.unit", "not a source\n");
        temp.File("packs/copy.sigil", "sigil 1\n");
        var result = ContentDiscovery.Scan(temp.Path, Path.Combine(temp.Path, "packs"));
        Assert.Equal("a.sigil", Assert.Single(result.Sources).Path.Value);
        Assert.Empty(result.Skipped);
    }

    [Fact]
    public void AnEmptyRootIsNoError()
    {
        using var temp = new TempDirectory();
        var result = ContentDiscovery.Scan(temp.Path);
        Assert.Empty(result.Sources);
        Assert.Empty(result.Diagnostics);
    }

    [Fact]
    public void AMissingRootSaysSoInsteadOfCompilingNothing()
    {
        using var temp = new TempDirectory();
        var error = Assert.Throws<AssetCompilerException>(() => ContentDiscovery.Scan(Path.Combine(temp.Path, "gone")));
        Assert.Contains("does not exist", error.Message, StringComparison.Ordinal);
    }

    [Fact]
    public void AFileAsRootIsRefused()
    {
        using var temp = new TempDirectory();
        var file = temp.File("a.sigil", "sigil 1\n");
        Assert.Throws<AssetCompilerException>(() => ContentDiscovery.Scan(file));
    }

    [Fact]
    public void ADeepTreeIsWalkedCompletely()
    {
        using var temp = new TempDirectory();
        for (var depth = 1; depth <= 12; depth++)
        {
            var directory = string.Join('/', Enumerable.Repeat("d", depth));
            temp.File($"{directory}/a.sigil", "sigil 1\n");
        }

        var result = ContentDiscovery.Scan(temp.Path);
        Assert.Equal(12, result.Sources.Count);
        Assert.Equal("d/a.sigil", result.Sources[0].Path.Value);
    }
}
