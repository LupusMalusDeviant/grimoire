using System;
using System.Linq;
using Xunit;

namespace Grimoire.AssetCompiler.Tests;

/// <summary>
/// Reading the <c>sigilc build --json</c> document of <c>docs/formats/sigil.md</c> §13.4. The document is
/// a versioned interface (project ADR-0010 building block 6): anything that is not it must be refused,
/// never guessed.
/// </summary>
public sealed class SigilcDocumentTests
{
    private const string Green = """
        {
          "schema": "grimoire.sigilc.build",
          "schema_version": 1,
          "command": "build",
          "ok": true,
          "units": [
            {
              "source": "tests/corpus/valid/01-ring-burst.sigil",
              "unit_path": "01-ring-burst.sigil",
              "output": "out/01-ring-burst.unit",
              "unit_id": "0bf46d12ddd3b2f6",
              "content_hash": "9fc51e542fd70081",
              "size": 222,
              "diagnostics": []
            }
          ]
        }
        """;

    [Fact]
    public void TheDocumentOfTheFormatDocumentationIsRead()
    {
        var result = SigilcProcess.ParseBuildDocument(Green);
        Assert.True(result.Ok);
        var unit = Assert.Single(result.Units);
        Assert.Equal("tests/corpus/valid/01-ring-burst.sigil", unit.Source);
        Assert.Equal("01-ring-burst.sigil", unit.UnitPath);
        Assert.Equal("out/01-ring-burst.unit", unit.Output);
        Assert.Equal("0bf46d12ddd3b2f6", unit.UnitId);
        Assert.Equal("9fc51e542fd70081", unit.ContentHash);
        Assert.Equal(222, unit.Size);
        Assert.Empty(unit.Diagnostics);
    }

    [Fact]
    public void ACheckDocumentIsReadTooSoTheSameCodeCanValidateWithoutWriting()
    {
        var result = SigilcProcess.ParseBuildDocument(Green
            .Replace("grimoire.sigilc.build", "grimoire.sigilc.check", StringComparison.Ordinal)
            .Replace("\"command\": \"build\"", "\"command\": \"check\"", StringComparison.Ordinal));
        Assert.True(result.Ok);
    }

    [Fact]
    public void ANullUnitPathAndOutputMeanNoUnitWasWritten()
    {
        var result = SigilcProcess.ParseBuildDocument("""
            {
              "schema": "grimoire.sigilc.build",
              "schema_version": 1,
              "command": "build",
              "ok": false,
              "units": [
                {
                  "source": "Ring.sigil",
                  "unit_path": null,
                  "output": null,
                  "unit_id": null,
                  "content_hash": null,
                  "size": null,
                  "diagnostics": [
                    {
                      "code": "SIG0025",
                      "severity": "error",
                      "file": "Ring.sigil",
                      "line": 1,
                      "column": 1,
                      "node_path": "",
                      "token": "Ring.sigil",
                      "message": "The file is not a canonical content path below the root.",
                      "fix_hint": "Use lowercase ASCII.",
                      "related": null
                    }
                  ]
                }
              ]
            }
            """);
        Assert.False(result.Ok);
        var unit = Assert.Single(result.Units);
        Assert.Null(unit.UnitPath);
        Assert.Null(unit.Output);
        Assert.Null(unit.Size);
        Assert.Equal("SIG0025", Assert.Single(unit.Diagnostics).Code);
    }

    [Theory]
    [InlineData("not json at all")]
    [InlineData("[]")]
    [InlineData("""{ "schema": "grimoire.sigilc.parse", "schema_version": 1, "command": "build", "ok": true, "units": [] }""")]
    [InlineData("""{ "schema": "grimoire.sigilc.build", "schema_version": 2, "command": "build", "ok": true, "units": [] }""")]
    [InlineData("""{ "schema": "grimoire.sigilc.build", "schema_version": 1, "command": "build", "ok": true }""")]
    [InlineData("""{ "schema": "grimoire.sigilc.build", "schema_version": 1, "command": "build", "ok": "yes", "units": [] }""")]
    [InlineData("""{ "schema": "grimoire.sigilc.build", "schema_version": 1, "command": "build", "ok": true, "units": [ { "source": "a.sigil" } ] }""")]
    [InlineData("""{ "schema": "grimoire.sigilc.build", "schema_version": 1, "command": "build", "ok": true, "units": [ { "source": "a.sigil", "unit_id": "NOTHEX0000000000", "diagnostics": [] } ] }""")]
    [InlineData("""{ "schema": "grimoire.sigilc.build", "schema_version": 1, "command": "build", "ok": true, "units": [ { "source": "a.sigil", "unit_id": "0bf4", "diagnostics": [] } ] }""")]
    public void AnythingElseIsRefused(string json) =>
        Assert.Throws<AssetCompilerException>(() => SigilcProcess.ParseBuildDocument(json));

    [Fact]
    public void AnUnknownFutureKeyDoesNotStopTheRun()
    {
        var result = SigilcProcess.ParseBuildDocument(Green.Replace(
            "\"command\": \"build\"",
            "\"command\": \"build\", \"future_key\": 7",
            StringComparison.Ordinal));
        Assert.True(result.Ok);
    }

    [Fact]
    public void ALongFileListIsSplitIntoBatchesThatStayBelowTheCommandLineLimit()
    {
        var files = Enumerable.Range(0, 400)
            .Select(index => $"content/sigil/{new string('p', 120)}{index.ToString("D4", System.Globalization.CultureInfo.InvariantCulture)}.sigil")
            .ToList();
        var batches = SigilcProcess.Batches(files).ToList();
        Assert.True(batches.Count > 1);
        Assert.Equal(files, batches.SelectMany(static batch => batch).ToList());
        Assert.All(batches, static batch =>
        {
            Assert.InRange(batch.Count, 1, SigilcProcess.MaxFilesPerInvocation);
            Assert.True(batch.Sum(static file => file.Length + 1) <= SigilcProcess.MaxArgumentBytesPerInvocation + 128);
        });
    }

    [Fact]
    public void ASingleVeryLongPathStillBecomesItsOwnInvocation()
    {
        var files = new[] { new string('p', SigilcProcess.MaxArgumentBytesPerInvocation * 2) + ".sigil" };
        Assert.Single(Assert.Single(SigilcProcess.Batches(files)));
    }

    [Fact]
    public void NoBinaryIsInventedWhenNoneIsThere()
    {
        var error = Assert.Throws<AssetCompilerException>(() => SigilcProcess.Locate("does-not-exist-sigilc"));
        Assert.Contains("does-not-exist-sigilc", error.Message, StringComparison.Ordinal);
    }
}
