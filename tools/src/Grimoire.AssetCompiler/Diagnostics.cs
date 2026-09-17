using System;
using System.Globalization;
using System.Text;
using System.Text.Json.Nodes;

namespace Grimoire.AssetCompiler;

/// <summary>
/// Codes of the asset compiler's own diagnostics. Sigil diagnostics keep their <c>SIG</c> codes and
/// travel through unchanged (project ADR-0010 building block 4); everything the asset compiler decides
/// itself — path normalisation, collisions, the pack — gets an <c>AC</c> code, so no code ever means two
/// things.
/// </summary>
public static class AcCode
{
    /// <summary>
    /// A file below the content root has no path that normalises to a valid <c>AssetPath</c>
    /// (contract §12).
    /// </summary>
    public const string InvalidPath = "AC0001";

    /// <summary>
    /// Two different source files normalise to the same asset path, so one would shadow the other.
    /// </summary>
    public const string PathCollision = "AC0002";

    /// <summary>
    /// A unit <c>sigilc</c> reported as written cannot be read back, or is not a <c>SigilUnit</c>.
    /// </summary>
    public const string UnitUnreadable = "AC0003";

    /// <summary>
    /// The pack cannot be written: a limit of contract §12 is exceeded.
    /// </summary>
    public const string PackRefused = "AC0004";

    /// <summary>
    /// The <c>sigilc</c> binary reports a different version than the engine version this tool was built
    /// for; its units could differ from the engine's expectation.
    /// </summary>
    public const string CompilerVersionMismatch = "AC0005";

    /// <summary>
    /// No source file was found below the content root.
    /// </summary>
    public const string NoSources = "AC0006";
}

/// <summary>
/// One diagnostic in the shape of <c>docs/formats/sigil.md</c> §6.2: a code, a severity, a file, a
/// 1-based line and column, a node path, the offending token, a message, a fix hint and an optional
/// related location.
/// </summary>
/// <remarks>
/// A diagnostic from <c>sigilc</c> keeps its parsed JSON object and is written out again exactly as it
/// arrived (project ADR-0010 building block 4: diagnostics pass through unchanged). The asset
/// compiler's own diagnostics are built in the same shape, so both render identically in text and in
/// JSON and tools need to understand only one form.
/// </remarks>
public sealed class Diagnostic
{
    private readonly JsonObject _json;

    private Diagnostic(JsonObject json)
    {
        _json = json;
    }

    /// <summary>
    /// The stable diagnostic code, e.g. <c>SIG0008</c> or <c>AC0001</c>.
    /// </summary>
    public string Code => Text("code");

    /// <summary>
    /// <c>error</c> or <c>warning</c>.
    /// </summary>
    public string Severity => Text("severity");

    /// <summary>
    /// The file as it was named on the command line.
    /// </summary>
    public string File => Text("file");

    /// <summary>
    /// The 1-based line.
    /// </summary>
    public int Line => Number("line");

    /// <summary>
    /// The 1-based column.
    /// </summary>
    public int Column => Number("column");

    /// <summary>
    /// The node path, possibly empty.
    /// </summary>
    public string NodePath => Text("node_path");

    /// <summary>
    /// The offending token, possibly empty.
    /// </summary>
    public string Token => Text("token");

    /// <summary>
    /// What went wrong.
    /// </summary>
    public string Message => Text("message");

    /// <summary>
    /// How to fix it, possibly empty.
    /// </summary>
    public string FixHint => Text("fix_hint");

    /// <summary>
    /// Whether this diagnostic stops the build (every diagnostic of severity <c>error</c> does).
    /// </summary>
    public bool IsError => string.Equals(Severity, "error", StringComparison.Ordinal);

    /// <summary>
    /// Reads a diagnostic from a <c>sigilc</c> JSON document, keeping the object for pass-through.
    /// </summary>
    /// <param name="node">The diagnostic object.</param>
    /// <returns>The diagnostic.</returns>
    /// <exception cref="AssetCompilerException">The object does not have the §6.2 shape.</exception>
    public static Diagnostic FromJson(JsonNode? node)
    {
        if (node is not JsonObject json)
        {
            throw new AssetCompilerException("a diagnostic in the sigilc output is not a JSON object");
        }

        var diagnostic = new Diagnostic(json);
        foreach (var key in new[] { "code", "severity", "file", "node_path", "token", "message", "fix_hint" })
        {
            _ = diagnostic.Text(key);
        }

        _ = diagnostic.Number("line");
        _ = diagnostic.Number("column");
        _ = diagnostic.Related;
        return diagnostic;
    }

    /// <summary>
    /// Builds a diagnostic of the asset compiler itself.
    /// </summary>
    /// <param name="code">An <see cref="AcCode"/>.</param>
    /// <param name="file">The file the diagnostic is about, as the user named it.</param>
    /// <param name="message">What went wrong.</param>
    /// <param name="fixHint">How to fix it, or an empty string.</param>
    /// <param name="nodePath">A node path, or an empty string.</param>
    /// <param name="token">The offending text, or an empty string.</param>
    /// <returns>The diagnostic.</returns>
    public static Diagnostic Create(string code, string file, string message, string fixHint = "", string nodePath = "", string token = "")
    {
        ArgumentNullException.ThrowIfNull(code);
        ArgumentNullException.ThrowIfNull(file);
        ArgumentNullException.ThrowIfNull(message);
        ArgumentNullException.ThrowIfNull(fixHint);
        ArgumentNullException.ThrowIfNull(nodePath);
        ArgumentNullException.ThrowIfNull(token);
        var json = new JsonObject
        {
            ["code"] = code,
            ["severity"] = "error",
            ["file"] = file,
            ["line"] = 1,
            ["column"] = 1,
            ["node_path"] = nodePath,
            ["token"] = token,
            ["message"] = message,
            ["fix_hint"] = fixHint,
            ["related"] = null,
        };
        return new Diagnostic(json);
    }

    /// <summary>
    /// The related location, or <c>null</c>.
    /// </summary>
    public RelatedLocation? Related
    {
        get
        {
            if (!_json.TryGetPropertyValue("related", out var node) || node is null)
            {
                return null;
            }

            if (node is not JsonObject related)
            {
                throw new AssetCompilerException("`related` in a sigilc diagnostic is neither null nor an object");
            }

            return new RelatedLocation(
                StringOf(related, "label"),
                NumberOf(related, "line"),
                NumberOf(related, "column"),
                StringOf(related, "token"));
        }
    }

    /// <summary>
    /// A copy of the JSON object, ready to be embedded into another document.
    /// </summary>
    /// <returns>The copy.</returns>
    public JsonNode ToJson() => _json.DeepClone();

    /// <summary>
    /// Renders the diagnostic in the text form of <c>docs/formats/sigil.md</c> §6.1, the same block
    /// <c>sigilc check</c> prints.
    /// </summary>
    /// <returns>The block, without a trailing newline.</returns>
    public string RenderText()
    {
        var text = new StringBuilder();
        text.Append(CultureInfo.InvariantCulture, $"{File}:{Line}:{Column}: {Severity}[{Code}]: {Message}");
        if (NodePath.Length > 0)
        {
            text.Append(CultureInfo.InvariantCulture, $"\n  at {NodePath}");
        }

        if (FixHint.Length > 0)
        {
            text.Append(CultureInfo.InvariantCulture, $"\n  fix: {FixHint}");
        }

        if (Related is { } related)
        {
            text.Append(CultureInfo.InvariantCulture, $"\n  related: {related.Label} at {File}:{related.Line}:{related.Column}: `{related.Token}`");
        }

        return text.ToString();
    }

    private string Text(string key) => StringOf(_json, key);

    private int Number(string key) => NumberOf(_json, key);

    private static string StringOf(JsonObject json, string key)
    {
        if (!json.TryGetPropertyValue(key, out var node) || node is null)
        {
            throw new AssetCompilerException($"a sigilc diagnostic has no `{key}`");
        }

        try
        {
            return node.GetValue<string>();
        }
        catch (Exception error) when (error is InvalidOperationException or FormatException)
        {
            throw new AssetCompilerException($"`{key}` in a sigilc diagnostic is not a string");
        }
    }

    private static int NumberOf(JsonObject json, string key)
    {
        if (!json.TryGetPropertyValue(key, out var node) || node is null)
        {
            throw new AssetCompilerException($"a sigilc diagnostic has no `{key}`");
        }

        try
        {
            return node.GetValue<int>();
        }
        catch (Exception error) when (error is InvalidOperationException or FormatException)
        {
            throw new AssetCompilerException($"`{key}` in a sigilc diagnostic is not an integer");
        }
    }
}

/// <summary>
/// The secondary location of a diagnostic, e.g. the delimiter an unmatched one fails to match.
/// </summary>
/// <param name="Label">What the location is.</param>
/// <param name="Line">Its 1-based line.</param>
/// <param name="Column">Its 1-based column.</param>
/// <param name="Token">The text at it.</param>
public readonly record struct RelatedLocation(string Label, int Line, int Column, string Token);

/// <summary>
/// The asset compiler cannot do what it was asked: a usage error, an unreadable path, or output from
/// <c>sigilc</c> it does not understand. Everything that is a problem *in the content* is a
/// <see cref="Diagnostic"/> instead.
/// </summary>
public sealed class AssetCompilerException : Exception
{
    /// <summary>
    /// Creates the exception.
    /// </summary>
    /// <param name="message">What went wrong.</param>
    public AssetCompilerException(string message)
        : base(message)
    {
    }

    /// <summary>
    /// Creates the exception.
    /// </summary>
    /// <param name="message">What went wrong.</param>
    /// <param name="inner">The cause.</param>
    public AssetCompilerException(string message, Exception inner)
        : base(message, inner)
    {
    }
}
