using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json.Nodes;
using Xunit;

namespace Grimoire.AssetCompiler.Tests;

/// <summary>
/// The diagnostic shape of <c>docs/formats/sigil.md</c> §6.1 and §6.2: a diagnostic from
/// <c>sigilc</c> is written out exactly as it arrived, and the asset compiler's own diagnostics use the
/// same shape (project ADR-0010 building block 4).
/// </summary>
public sealed class DiagnosticTests
{
    private const string Full = """
        {
          "code": "SIG0007",
          "severity": "error",
          "file": "content/sigil/ring.sigil",
          "line": 12,
          "column": 3,
          "node_path": "emitters.burst",
          "token": "{",
          "message": "Body opened with `{` is never closed.",
          "fix_hint": "Close the body with `}`.",
          "related": { "label": "opened here", "line": 8, "column": 1, "token": "{" }
        }
        """;

    [Fact]
    public void ADiagnosticRendersTheTextFormOfTheFormatDocumentation()
    {
        var diagnostic = Diagnostic.FromJson(JsonNode.Parse(Full));
        Assert.Equal(
            "content/sigil/ring.sigil:12:3: error[SIG0007]: Body opened with `{` is never closed.\n"
            + "  at emitters.burst\n"
            + "  fix: Close the body with `}`.\n"
            + "  related: opened here at content/sigil/ring.sigil:8:1: `{`",
            diagnostic.RenderText());
    }

    [Fact]
    public void AnEmptyNodePathFixHintAndRelatedLocationLeaveTheirLinesOut()
    {
        var json = JsonNode.Parse(Full)!.AsObject();
        json["node_path"] = string.Empty;
        json["fix_hint"] = string.Empty;
        json["related"] = null;
        var diagnostic = Diagnostic.FromJson(json);
        Assert.Equal("content/sigil/ring.sigil:12:3: error[SIG0007]: Body opened with `{` is never closed.", diagnostic.RenderText());
    }

    [Fact]
    public void TheSeverityIsTakenFromTheDocumentNotAssumed()
    {
        var json = JsonNode.Parse(Full)!.AsObject();
        json["severity"] = "warning";
        var diagnostic = Diagnostic.FromJson(json);
        Assert.StartsWith("content/sigil/ring.sigil:12:3: warning[SIG0007]:", diagnostic.RenderText(), StringComparison.Ordinal);
        Assert.False(diagnostic.IsError);
    }

    [Fact]
    public void ADiagnosticIsWrittenOutExactlyAsItArrived()
    {
        var original = JsonNode.Parse(Full)!.AsObject();
        var diagnostic = Diagnostic.FromJson(original.DeepClone());
        Assert.Equal(original.ToJsonString(), diagnostic.ToJson().ToJsonString(), StringComparer.Ordinal);
    }

    [Fact]
    public void AnUnknownKeyTravelsThroughUntouched()
    {
        var json = JsonNode.Parse(Full)!.AsObject();
        json["future_key"] = 42;
        var diagnostic = Diagnostic.FromJson(json.DeepClone());
        var written = diagnostic.ToJson().AsObject();
        Assert.Equal(42, written["future_key"]!.GetValue<int>());
        Assert.Equal(json.ToJsonString(), written.ToJsonString(), StringComparer.Ordinal);
    }

    [Fact]
    public void TheSameDiagnosticCanBeEmbeddedIntoSeveralDocuments()
    {
        var diagnostic = Diagnostic.FromJson(JsonNode.Parse(Full));
        var first = new JsonArray { diagnostic.ToJson() };
        var second = new JsonArray { diagnostic.ToJson() };
        Assert.Equal(first.ToJsonString(), second.ToJsonString(), StringComparer.Ordinal);
    }

    [Theory]
    [InlineData("code")]
    [InlineData("severity")]
    [InlineData("file")]
    [InlineData("line")]
    [InlineData("column")]
    [InlineData("node_path")]
    [InlineData("token")]
    [InlineData("message")]
    [InlineData("fix_hint")]
    public void AMissingKeyIsRefusedInsteadOfGuessed(string key)
    {
        var json = JsonNode.Parse(Full)!.AsObject();
        json.Remove(key);
        var error = Assert.Throws<AssetCompilerException>(() => Diagnostic.FromJson(json));
        Assert.Contains(key, error.Message, StringComparison.Ordinal);
    }

    [Fact]
    public void AWronglyTypedFieldIsRefused()
    {
        var json = JsonNode.Parse(Full)!.AsObject();
        json["line"] = "twelve";
        Assert.Throws<AssetCompilerException>(() => Diagnostic.FromJson(json));
    }

    [Fact]
    public void ARelatedLocationThatIsNoObjectIsRefused()
    {
        var json = JsonNode.Parse(Full)!.AsObject();
        json["related"] = "opened here";
        Assert.Throws<AssetCompilerException>(() => Diagnostic.FromJson(json));
    }

    [Fact]
    public void ADiagnosticThatIsNoObjectIsRefused()
    {
        Assert.Throws<AssetCompilerException>(() => Diagnostic.FromJson(JsonNode.Parse("[]")));
        Assert.Throws<AssetCompilerException>(() => Diagnostic.FromJson(null));
    }

    [Fact]
    public void TheAssetCompilersOwnDiagnosticsUseTheSameShape()
    {
        var diagnostic = Diagnostic.Create(
            AcCode.InvalidPath,
            "content/Sigil/Ring.sigil",
            "`Sigil/Ring.sigil` is no asset path below the content root: path contains a byte outside ASCII [a-z0-9_.-] and '/'.",
            "Rename the file.",
            token: "Sigil/Ring.sigil");
        Assert.Equal(
            "content/Sigil/Ring.sigil:1:1: error[AC0001]: `Sigil/Ring.sigil` is no asset path below the content root: "
            + "path contains a byte outside ASCII [a-z0-9_.-] and '/'.\n"
            + "  fix: Rename the file.",
            diagnostic.RenderText());
        var json = diagnostic.ToJson().AsObject();
        Assert.Equal(
            new[] { "code", "severity", "file", "line", "column", "node_path", "token", "message", "fix_hint", "related" },
            json.Select(static pair => pair.Key).ToArray());
        Assert.True(diagnostic.IsError);
    }

    [Fact]
    public void EveryCodeOfTheAssetCompilerIsDistinct()
    {
        var codes = new[]
        {
            AcCode.InvalidPath,
            AcCode.PathCollision,
            AcCode.UnitUnreadable,
            AcCode.PackRefused,
            AcCode.CompilerVersionMismatch,
            AcCode.NoSources,
        };
        Assert.Equal(codes.Length, new HashSet<string>(codes, StringComparer.Ordinal).Count);
        Assert.All(codes, static code => Assert.Matches("^AC[0-9]{4}$", code));
    }
}
