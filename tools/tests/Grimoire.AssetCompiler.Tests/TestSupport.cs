using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Grimoire.Formats;
using Grimoire.Formats.Pack;

namespace Grimoire.AssetCompiler.Tests;

/// <summary>
/// A directory that deletes itself.
/// </summary>
internal sealed class TempDirectory : IDisposable
{
    public TempDirectory()
    {
        Path = System.IO.Path.Combine(System.IO.Path.GetTempPath(), "grimoire-ac-" + Guid.NewGuid().ToString("n"));
        Directory.CreateDirectory(Path);
    }

    public string Path { get; }

    public string File(string relativePath, string content)
    {
        var full = System.IO.Path.Combine(Path, relativePath.Replace('/', System.IO.Path.DirectorySeparatorChar));
        Directory.CreateDirectory(System.IO.Path.GetDirectoryName(full)!);
        System.IO.File.WriteAllText(full, content, new UTF8Encoding(encoderShouldEmitUTF8Identifier: false));
        return full;
    }

    public string Sub(string relativePath)
    {
        var full = System.IO.Path.Combine(Path, relativePath.Replace('/', System.IO.Path.DirectorySeparatorChar));
        Directory.CreateDirectory(full);
        return full;
    }

    public void Dispose()
    {
        try
        {
            Directory.Delete(Path, recursive: true);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
            // A file is still open on Windows; the temporary directory stays behind, which is harmless.
        }
    }
}

/// <summary>
/// A compiler double: it writes unit bytes with a correct header (sigil.md §10.1) for every file it is
/// asked about, unless the test gave that file diagnostics. It records every invocation, so tests can see
/// how the asset compiler batches and how often the watch loop compiles.
/// </summary>
internal sealed class FakeSigilc : ISigilcRunner
{
    private readonly Dictionary<string, List<Diagnostic>> _diagnostics = new(StringComparer.Ordinal);
    private readonly Dictionary<string, byte[]> _payloads = new(StringComparer.Ordinal);
    private readonly Dictionary<string, ulong> _unitIds = new(StringComparer.Ordinal);

    public string Version { get; set; } = EngineBuild.EngineVersion;

    public List<List<string>> Invocations { get; } = new();

    public int WrittenUnits { get; private set; }

    public bool WriteNoUnits { get; set; }

    public void GiveDiagnostics(string commandLinePath, params Diagnostic[] diagnostics) =>
        _diagnostics[commandLinePath] = new List<Diagnostic>(diagnostics);

    public void GivePayload(string commandLinePath, byte[] payload) => _payloads[commandLinePath] = payload;

    public void GiveUnitId(string commandLinePath, ulong unitId) => _unitIds[commandLinePath] = unitId;

    public Task<SigilcBuildResult> BuildAsync(
        string contentRoot,
        string outputDirectory,
        string? behaviors,
        IReadOnlyList<string> files,
        CancellationToken cancellationToken)
    {
        Invocations.Add(new List<string>(files));
        var root = contentRoot.Replace('\\', '/').TrimEnd('/');
        var units = new List<SigilcUnit>(files.Count);
        var ok = true;
        foreach (var file in files)
        {
            if (_diagnostics.TryGetValue(file, out var diagnostics))
            {
                ok = false;
                units.Add(new SigilcUnit(file, null, null, null, null, null, diagnostics));
                continue;
            }

            var relative = root.Length == 0 || !file.StartsWith(root + "/", StringComparison.Ordinal)
                ? file
                : file.Substring(root.Length + 1);
            var path = AssetPath.Create(relative);
            var unitId = _unitIds.TryGetValue(file, out var forced) ? forced : AssetId.FromPath(path).Value;
            var payload = _payloads.TryGetValue(file, out var given) ? given : Encoding.UTF8.GetBytes("payload of " + relative);
            var bytes = Unit(unitId, payload);
            string? output = null;
            if (!WriteNoUnits)
            {
                output = Path.Combine(outputDirectory, Path.ChangeExtension(relative, ".unit")).Replace('\\', '/');
                Directory.CreateDirectory(Path.GetDirectoryName(output)!);
                File.WriteAllBytes(output, bytes);
                WrittenUnits++;
            }

            units.Add(new SigilcUnit(
                file,
                relative,
                output,
                unitId.ToString("x16", System.Globalization.CultureInfo.InvariantCulture),
                ContentHash(bytes).ToString("x16", System.Globalization.CultureInfo.InvariantCulture),
                bytes.Length,
                Array.Empty<Diagnostic>()));
        }

        return Task.FromResult(new SigilcBuildResult(ok, units));
    }

    /// <summary>
    /// A unit with a valid header: magic, format version 1, the id, the content hash over the bytes
    /// outside the hash field, and the payload length (sigil.md §10.1). The payload itself is opaque to
    /// the pack, which stores unit bytes unchanged.
    /// </summary>
    public static byte[] Unit(ulong unitId, byte[] payload)
    {
        ArgumentNullException.ThrowIfNull(payload);
        var bytes = new byte[40 + payload.Length];
        "GRIMSIGL"u8.CopyTo(bytes);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(8), 1);
        BinaryPrimitives.WriteUInt32LittleEndian(bytes.AsSpan(12), 0);
        BinaryPrimitives.WriteUInt64LittleEndian(bytes.AsSpan(16), unitId);
        BinaryPrimitives.WriteUInt64LittleEndian(bytes.AsSpan(32), (ulong)payload.Length);
        payload.CopyTo(bytes.AsSpan(40));
        BinaryPrimitives.WriteUInt64LittleEndian(bytes.AsSpan(24), ContentHash(bytes));
        return bytes;
    }

    private static ulong ContentHash(byte[] bytes)
    {
        var hasher = new StableHasher(0);
        hasher.WriteBytes(bytes.AsSpan(0, 24));
        hasher.WriteBytes(bytes.AsSpan(32));
        return hasher.Finish();
    }
}
