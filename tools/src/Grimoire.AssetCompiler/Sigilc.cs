using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading;
using System.Threading.Tasks;

namespace Grimoire.AssetCompiler;

/// <summary>
/// What <c>sigilc build --json</c> reported about one file (sigil.md §13.4).
/// </summary>
/// <param name="Source">The file as it was named on the command line.</param>
/// <param name="UnitPath">The canonical content path, or <c>null</c> if none could be formed.</param>
/// <param name="Output">Where the unit was written, or <c>null</c> if it was not.</param>
/// <param name="UnitId">The unit id as 16 hex digits, or <c>null</c>.</param>
/// <param name="ContentHash">The unit's content hash as 16 hex digits, or <c>null</c>.</param>
/// <param name="Size">The unit's length in bytes, or <c>null</c>.</param>
/// <param name="Diagnostics">Its diagnostics, unchanged.</param>
public sealed record SigilcUnit(
    string Source,
    string? UnitPath,
    string? Output,
    string? UnitId,
    string? ContentHash,
    long? Size,
    IReadOnlyList<Diagnostic> Diagnostics);

/// <summary>
/// What one or more <c>sigilc build --json</c> runs reported.
/// </summary>
/// <param name="Ok"><c>true</c> if no file has a diagnostic.</param>
/// <param name="Units">One entry per file, in the order the files were given.</param>
public sealed record SigilcBuildResult(bool Ok, IReadOnlyList<SigilcUnit> Units);

/// <summary>
/// The Sigil compiler as the asset compiler uses it. <c>grimoire_sigilc</c> owns lexer, parser,
/// validation and the unit bytes (project ADR-0010); this is the only way the asset compiler reaches
/// them, and the seam tests replace.
/// </summary>
public interface ISigilcRunner
{
    /// <summary>
    /// How the compiler identifies itself, e.g. <c>0.4.0</c> from <c>sigilc --version</c>.
    /// </summary>
    string Version { get; }

    /// <summary>
    /// Compiles <paramref name="files"/> and writes each unit below <paramref name="outputDirectory"/>.
    /// </summary>
    /// <param name="contentRoot">The content root, as it should appear in diagnostics.</param>
    /// <param name="outputDirectory">Where units go.</param>
    /// <param name="behaviors">A behaviour manifest (sigil.md §13.3), or <c>null</c>.</param>
    /// <param name="files">The files, as they should appear in diagnostics.</param>
    /// <param name="cancellationToken">Cancellation.</param>
    /// <returns>What the compiler reported.</returns>
    /// <exception cref="AssetCompilerException">The compiler could not run, or its output is not the
    /// documented document.</exception>
    Task<SigilcBuildResult> BuildAsync(
        string contentRoot,
        string outputDirectory,
        string? behaviors,
        IReadOnlyList<string> files,
        CancellationToken cancellationToken);
}

/// <summary>
/// Runs the real <c>sigilc</c> binary.
/// </summary>
/// <remarks>
/// The binary is located once, its version is read once, and every build is one or more
/// <c>sigilc build --json</c> invocations whose JSON documents are merged in argument order. Long file
/// lists are split into batches, so a content tree stays compilable when its arguments would exceed the
/// platform's command-line limit; each unit is independent, so the split changes no byte.
/// </remarks>
public sealed class SigilcProcess : ISigilcRunner
{
    /// <summary>
    /// Environment variable naming the binary when no <c>--sigilc</c> is given.
    /// </summary>
    public const string PathEnvironmentVariable = "GRIMOIRE_SIGILC";

    /// <summary>
    /// Largest number of files in one invocation.
    /// </summary>
    public const int MaxFilesPerInvocation = 128;

    /// <summary>
    /// Largest total length of the file arguments of one invocation, well below the platform limits.
    /// </summary>
    public const int MaxArgumentBytesPerInvocation = 16_000;

    private readonly Lazy<string> _version;

    /// <summary>
    /// Wraps the binary at <paramref name="binaryPath"/>.
    /// </summary>
    /// <param name="binaryPath">Path to <c>sigilc</c>.</param>
    public SigilcProcess(string binaryPath)
    {
        ArgumentNullException.ThrowIfNull(binaryPath);
        BinaryPath = binaryPath;
        _version = new Lazy<string>(ReadVersion);
    }

    /// <summary>
    /// The binary this runner calls.
    /// </summary>
    public string BinaryPath { get; }

    /// <inheritdoc/>
    public string Version => _version.Value;

    /// <summary>
    /// Finds the binary: the explicit path, else <see cref="PathEnvironmentVariable"/>, else
    /// <c>sigilc</c> on <c>PATH</c>.
    /// </summary>
    /// <param name="explicitPath">Path from <c>--sigilc</c>, or <c>null</c>.</param>
    /// <returns>The path to the binary.</returns>
    /// <exception cref="AssetCompilerException">No binary was found.</exception>
    public static string Locate(string? explicitPath)
    {
        if (explicitPath is { Length: > 0 })
        {
            return File.Exists(explicitPath)
                ? Path.GetFullPath(explicitPath)
                : throw new AssetCompilerException($"no sigilc binary at `{explicitPath}`");
        }

        var fromEnvironment = Environment.GetEnvironmentVariable(PathEnvironmentVariable);
        if (fromEnvironment is { Length: > 0 })
        {
            return File.Exists(fromEnvironment)
                ? Path.GetFullPath(fromEnvironment)
                : throw new AssetCompilerException($"{PathEnvironmentVariable} names `{fromEnvironment}`, which is no file");
        }

        var name = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "sigilc.exe" : "sigilc";
        foreach (var directory in (Environment.GetEnvironmentVariable("PATH") ?? string.Empty).Split(Path.PathSeparator))
        {
            if (directory.Length == 0)
            {
                continue;
            }

            string candidate;
            try
            {
                candidate = Path.Combine(directory, name);
            }
            catch (ArgumentException)
            {
                continue;
            }

            if (File.Exists(candidate))
            {
                return Path.GetFullPath(candidate);
            }
        }

        throw new AssetCompilerException(
            $"no sigilc binary found: pass `--sigilc <path>`, set {PathEnvironmentVariable}, or put `{name}` on PATH. " +
            "The game's CI builds it from the pinned engine tag (`cargo build --release -p grimoire_sigilc`).");
    }

    /// <inheritdoc/>
    public async Task<SigilcBuildResult> BuildAsync(
        string contentRoot,
        string outputDirectory,
        string? behaviors,
        IReadOnlyList<string> files,
        CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(contentRoot);
        ArgumentNullException.ThrowIfNull(outputDirectory);
        ArgumentNullException.ThrowIfNull(files);
        var units = new List<SigilcUnit>(files.Count);
        var ok = true;
        foreach (var batch in Batches(files))
        {
            var arguments = new List<string> { "build", "--json", "--root", contentRoot, "--out", outputDirectory };
            if (behaviors is { Length: > 0 })
            {
                arguments.Add("--behaviors");
                arguments.Add(behaviors);
            }

            arguments.Add("--");
            arguments.AddRange(batch);
            var (exitCode, stdout, stderr) = await RunAsync(arguments, cancellationToken).ConfigureAwait(false);
            if (exitCode is not (0 or 1))
            {
                throw new AssetCompilerException(
                    $"sigilc could not run (exit code {exitCode.ToString(CultureInfo.InvariantCulture)}): {Trim(stderr)}");
            }

            var document = ParseBuildDocument(stdout);
            ok &= document.Ok;
            units.AddRange(document.Units);
        }

        return new SigilcBuildResult(ok, units);
    }

    /// <summary>
    /// Reads a <c>sigilc build --json</c> or <c>check --json</c> document (sigil.md §13.4).
    /// </summary>
    /// <param name="json">The document.</param>
    /// <returns>Its content.</returns>
    /// <exception cref="AssetCompilerException">The document is not the documented one.</exception>
    public static SigilcBuildResult ParseBuildDocument(string json)
    {
        ArgumentNullException.ThrowIfNull(json);
        JsonNode? node;
        try
        {
            node = JsonNode.Parse(json);
        }
        catch (JsonException error)
        {
            throw new AssetCompilerException($"the sigilc output is no JSON document: {error.Message}", error);
        }

        if (node is not JsonObject root)
        {
            throw new AssetCompilerException("the sigilc output is no JSON object");
        }

        var schema = StringValue(root, "schema");
        if (schema is not ("grimoire.sigilc.build" or "grimoire.sigilc.check"))
        {
            throw new AssetCompilerException($"the sigilc output has schema `{schema}`, not `grimoire.sigilc.build`");
        }

        if (IntValue(root, "schema_version") != 1)
        {
            throw new AssetCompilerException("the sigilc output has a schema_version other than 1; rebuild the tools for this engine tag");
        }

        var ok = BoolValue(root, "ok");
        if (!root.TryGetPropertyValue("units", out var unitsNode) || unitsNode is not JsonArray unitsArray)
        {
            throw new AssetCompilerException("the sigilc output has no `units` array");
        }

        var units = new List<SigilcUnit>(unitsArray.Count);
        foreach (var entry in unitsArray)
        {
            if (entry is not JsonObject unit)
            {
                throw new AssetCompilerException("an entry of `units` is no JSON object");
            }

            if (!unit.TryGetPropertyValue("diagnostics", out var diagnosticsNode) || diagnosticsNode is not JsonArray diagnosticsArray)
            {
                throw new AssetCompilerException("a unit of the sigilc output has no `diagnostics` array");
            }

            var diagnostics = new List<Diagnostic>(diagnosticsArray.Count);
            foreach (var diagnostic in diagnosticsArray)
            {
                diagnostics.Add(Diagnostic.FromJson(diagnostic));
            }

            units.Add(new SigilcUnit(
                StringValue(unit, "source"),
                OptionalString(unit, "unit_path"),
                OptionalString(unit, "output"),
                OptionalHash(unit, "unit_id"),
                OptionalHash(unit, "content_hash"),
                OptionalLong(unit, "size"),
                diagnostics));
        }

        return new SigilcBuildResult(ok, units);
    }

    /// <summary>
    /// Splits a file list into invocations that stay below the platform's command-line limit.
    /// </summary>
    /// <param name="files">The files, in order.</param>
    /// <returns>The batches, in the same order.</returns>
    internal static IEnumerable<List<string>> Batches(IReadOnlyList<string> files)
    {
        var batch = new List<string>();
        var length = 0;
        foreach (var file in files)
        {
            if (batch.Count > 0 && (batch.Count >= MaxFilesPerInvocation || length + file.Length + 1 > MaxArgumentBytesPerInvocation))
            {
                yield return batch;
                batch = new List<string>();
                length = 0;
            }

            batch.Add(file);
            length += file.Length + 1;
        }

        if (batch.Count > 0)
        {
            yield return batch;
        }
    }

    private string ReadVersion()
    {
        var (exitCode, stdout, stderr) = RunAsync(new List<string> { "--version" }, CancellationToken.None)
            .ConfigureAwait(false).GetAwaiter().GetResult();
        if (exitCode != 0)
        {
            throw new AssetCompilerException($"`{BinaryPath} --version` failed with exit code {exitCode.ToString(CultureInfo.InvariantCulture)}: {Trim(stderr)}");
        }

        var text = stdout.Trim();
        const string prefix = "sigilc ";
        if (!text.StartsWith(prefix, StringComparison.Ordinal) || text.Length == prefix.Length)
        {
            throw new AssetCompilerException($"`{BinaryPath} --version` printed `{text}`, not `sigilc <version>`");
        }

        return text.Substring(prefix.Length);
    }

    /// <summary>
    /// Runs the binary with the given arguments and collects its streams, for the tests that compare the
    /// asset compiler's output against what <c>sigilc</c> itself prints.
    /// </summary>
    /// <param name="arguments">The arguments.</param>
    /// <param name="cancellationToken">Cancellation.</param>
    /// <returns>Exit code, standard output and standard error.</returns>
    internal async Task<(int ExitCode, string Stdout, string Stderr)> InvokeAsync(List<string> arguments, CancellationToken cancellationToken) =>
        await RunAsync(arguments, cancellationToken).ConfigureAwait(false);

    private async Task<(int ExitCode, string Stdout, string Stderr)> RunAsync(List<string> arguments, CancellationToken cancellationToken)
    {
        var startInfo = new ProcessStartInfo(BinaryPath)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = false,
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        foreach (var argument in arguments)
        {
            startInfo.ArgumentList.Add(argument);
        }

        using var process = new Process { StartInfo = startInfo };
        try
        {
            if (!process.Start())
            {
                throw new AssetCompilerException($"cannot start `{BinaryPath}`");
            }
        }
        catch (Exception error) when (error is System.ComponentModel.Win32Exception or InvalidOperationException)
        {
            throw new AssetCompilerException($"cannot start `{BinaryPath}`: {error.Message}", error);
        }

        var stdout = process.StandardOutput.ReadToEndAsync(cancellationToken);
        var stderr = process.StandardError.ReadToEndAsync(cancellationToken);
        try
        {
            await process.WaitForExitAsync(cancellationToken).ConfigureAwait(false);
            return (process.ExitCode, await stdout.ConfigureAwait(false), await stderr.ConfigureAwait(false));
        }
        catch (OperationCanceledException)
        {
            try
            {
                process.Kill(entireProcessTree: true);
            }
            catch (Exception error) when (error is InvalidOperationException or NotSupportedException or System.ComponentModel.Win32Exception)
            {
                // The process ended on its own; nothing left to stop.
            }

            throw;
        }
    }

    private static string Trim(string text)
    {
        var trimmed = text.Trim();
        return trimmed.Length == 0 ? "(no message)" : trimmed;
    }

    private static string StringValue(JsonObject json, string key) =>
        OptionalString(json, key) ?? throw new AssetCompilerException($"the sigilc output has no `{key}`");

    private static string? OptionalString(JsonObject json, string key)
    {
        if (!json.TryGetPropertyValue(key, out var node) || node is null)
        {
            return null;
        }

        try
        {
            return node.GetValue<string>();
        }
        catch (Exception error) when (error is InvalidOperationException or FormatException)
        {
            throw new AssetCompilerException($"`{key}` in the sigilc output is not a string");
        }
    }

    private static string? OptionalHash(JsonObject json, string key)
    {
        var text = OptionalString(json, key);
        if (text is null)
        {
            return null;
        }

        if (text.Length != 16)
        {
            throw new AssetCompilerException($"`{key}` in the sigilc output is not 16 hex digits");
        }

        foreach (var c in text)
        {
            if (c is not (>= '0' and <= '9' or >= 'a' and <= 'f'))
            {
                throw new AssetCompilerException($"`{key}` in the sigilc output is not 16 lowercase hex digits");
            }
        }

        return text;
    }

    private static long? OptionalLong(JsonObject json, string key)
    {
        if (!json.TryGetPropertyValue(key, out var node) || node is null)
        {
            return null;
        }

        try
        {
            return node.GetValue<long>();
        }
        catch (Exception error) when (error is InvalidOperationException or FormatException)
        {
            throw new AssetCompilerException($"`{key}` in the sigilc output is not an integer");
        }
    }

    private static int IntValue(JsonObject json, string key)
    {
        var value = OptionalLong(json, key) ?? throw new AssetCompilerException($"the sigilc output has no `{key}`");
        return value is >= int.MinValue and <= int.MaxValue
            ? (int)value
            : throw new AssetCompilerException($"`{key}` in the sigilc output is out of range");
    }

    private static bool BoolValue(JsonObject json, string key)
    {
        if (!json.TryGetPropertyValue(key, out var node) || node is null)
        {
            throw new AssetCompilerException($"the sigilc output has no `{key}`");
        }

        try
        {
            return node.GetValue<bool>();
        }
        catch (Exception error) when (error is InvalidOperationException or FormatException)
        {
            throw new AssetCompilerException($"`{key}` in the sigilc output is not a boolean");
        }
    }
}
