using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Text.Json.Nodes;
using System.Threading;
using System.Threading.Tasks;
using Grimoire.Formats.Pack;

namespace Grimoire.AssetCompiler;

/// <summary>
/// What to build.
/// </summary>
public sealed class BuildOptions
{
    /// <summary>
    /// Creates the options.
    /// </summary>
    /// <param name="contentRoot">The content root, as the user wrote it.</param>
    public BuildOptions(string contentRoot)
    {
        ArgumentNullException.ThrowIfNull(contentRoot);
        ContentRoot = contentRoot;
    }

    /// <summary>
    /// The content root, as the user wrote it; it appears in every diagnostic that way.
    /// </summary>
    public string ContentRoot { get; }

    /// <summary>
    /// Where pack and units go; a build artefact.
    /// </summary>
    public string OutputDirectory { get; init; } = "packs";

    /// <summary>
    /// File name of the pack below <see cref="OutputDirectory"/>.
    /// </summary>
    public string PackName { get; init; } = "content.grimpack";

    /// <summary>
    /// A behaviour manifest (sigil.md §13.3), or <c>null</c>.
    /// </summary>
    public string? Behaviors { get; init; }

    /// <summary>
    /// A file with the opaque application block of the manifest (at most 64 KiB), or <c>null</c>.
    /// </summary>
    public string? ApplicationFile { get; init; }

    /// <summary>
    /// Whether an empty content root is allowed instead of <see cref="AcCode.NoSources"/>.
    /// </summary>
    public bool AllowEmpty { get; init; }

    /// <summary>
    /// Whether a <c>sigilc</c> of another version than this build's engine version is allowed instead of
    /// <see cref="AcCode.CompilerVersionMismatch"/>.
    /// </summary>
    public bool AllowVersionMismatch { get; init; }

    /// <summary>
    /// The engine version this tool build belongs to; the <c>sigilc</c> version is compared against it.
    /// </summary>
    public string EngineVersion { get; init; } = Grimoire.Formats.EngineBuild.EngineVersion;
}

/// <summary>
/// One compiled unit in the pack.
/// </summary>
/// <param name="Source">The source file as it was named on the command line.</param>
/// <param name="AssetPath">Its asset path, which is also the canonical content path.</param>
/// <param name="AssetId">The asset id of that path, as 16 hex digits.</param>
/// <param name="UnitId">The unit id in the unit's header, as 16 hex digits, or <c>null</c> if no unit was
/// built.</param>
/// <param name="ContentHash">The unit's content hash, as 16 hex digits, or <c>null</c>.</param>
/// <param name="Size">The unit's length in bytes, or <c>null</c>.</param>
/// <param name="KindVersion">The <c>SigilUnit</c> format version of the unit, or <c>null</c>.</param>
/// <param name="Diagnostics">The diagnostics of this source, unchanged from <c>sigilc</c>.</param>
public sealed record BuiltUnit(
    string Source,
    string AssetPath,
    string AssetId,
    string? UnitId,
    string? ContentHash,
    long? Size,
    uint? KindVersion,
    IReadOnlyList<Diagnostic> Diagnostics);

/// <summary>
/// How long the phases of a build took. Wall clock, for the rebuild goal of PRD-0016 (&lt; 60 s); it never
/// reaches the report document or the pack, both of which stay a pure function of the input.
/// </summary>
/// <param name="Discovery">Content discovery.</param>
/// <param name="Compile">Every <c>sigilc</c> invocation.</param>
/// <param name="Pack">Reading the units back, writing and verifying the pack.</param>
/// <param name="Total">The whole build.</param>
public readonly record struct BuildTimings(TimeSpan Discovery, TimeSpan Compile, TimeSpan Pack, TimeSpan Total);

/// <summary>
/// The result of one build.
/// </summary>
public sealed class BuildResult
{
    internal BuildResult(
        bool ok,
        string contentRoot,
        string compiler,
        string compilerVersion,
        IReadOnlyList<BuiltUnit> units,
        IReadOnlyList<Diagnostic> diagnostics,
        IReadOnlyList<SkippedFile> skipped,
        string? packPath,
        long packBytes,
        string? packContentHash,
        BuildTimings timings)
    {
        Ok = ok;
        ContentRoot = contentRoot;
        Compiler = compiler;
        CompilerVersion = compilerVersion;
        Units = units;
        Diagnostics = diagnostics;
        Skipped = skipped;
        PackPath = packPath;
        PackBytes = packBytes;
        PackContentHash = packContentHash;
        Timings = timings;
    }

    /// <summary>
    /// Whether the build found no problem at all and wrote the pack.
    /// </summary>
    public bool Ok { get; }

    /// <summary>
    /// The content root as the user wrote it.
    /// </summary>
    public string ContentRoot { get; }

    /// <summary>
    /// The compiler name in the pack manifest.
    /// </summary>
    public string Compiler { get; }

    /// <summary>
    /// The compiler version in the pack manifest: the version the <c>sigilc</c> binary reports, because it
    /// produced the unit bytes (project ADR-0010 building block 3).
    /// </summary>
    public string CompilerVersion { get; }

    /// <summary>
    /// The units, ascending by asset path.
    /// </summary>
    public IReadOnlyList<BuiltUnit> Units { get; }

    /// <summary>
    /// Diagnostics of the asset compiler itself.
    /// </summary>
    public IReadOnlyList<Diagnostic> Diagnostics { get; }

    /// <summary>
    /// Files below the content root that are not sources.
    /// </summary>
    public IReadOnlyList<SkippedFile> Skipped { get; }

    /// <summary>
    /// Where the pack was written, or <c>null</c> if none was.
    /// </summary>
    public string? PackPath { get; }

    /// <summary>
    /// The pack's length in bytes, or <c>0</c>.
    /// </summary>
    public long PackBytes { get; }

    /// <summary>
    /// The pack's content hash (contract §12) as 64 hex digits, or <c>null</c>.
    /// </summary>
    public string? PackContentHash { get; }

    /// <summary>
    /// How long the build took.
    /// </summary>
    public BuildTimings Timings { get; }

    /// <summary>
    /// Every diagnostic of the run, the asset compiler's own first, then the sources' in pack order.
    /// </summary>
    /// <returns>The diagnostics.</returns>
    public IReadOnlyList<Diagnostic> AllDiagnostics()
    {
        var all = new List<Diagnostic>(Diagnostics);
        foreach (var unit in Units)
        {
            all.AddRange(unit.Diagnostics);
        }

        return all;
    }

    /// <summary>
    /// The report document <c>grimoire.ac.build</c> v1: fixed key order, ids and hashes as lowercase hex,
    /// no timing and no timestamp, so identical input gives an identical document (contract §2 rule 11).
    /// </summary>
    /// <returns>The document.</returns>
    public JsonObject ToJson()
    {
        var units = new JsonArray();
        foreach (var unit in Units)
        {
            var diagnostics = new JsonArray();
            foreach (var diagnostic in unit.Diagnostics)
            {
                diagnostics.Add(diagnostic.ToJson());
            }

            units.Add(new JsonObject
            {
                ["source"] = unit.Source,
                ["asset_path"] = unit.AssetPath,
                ["asset_id"] = unit.AssetId,
                ["unit_id"] = unit.UnitId,
                ["content_hash"] = unit.ContentHash,
                ["size"] = unit.Size,
                ["kind_version"] = unit.KindVersion,
                ["diagnostics"] = diagnostics,
            });
        }

        var own = new JsonArray();
        foreach (var diagnostic in Diagnostics)
        {
            own.Add(diagnostic.ToJson());
        }

        var skipped = new JsonArray();
        foreach (var file in Skipped)
        {
            skipped.Add(new JsonObject
            {
                ["path"] = file.CommandLinePath,
                ["reason"] = file.Reason,
            });
        }

        return new JsonObject
        {
            ["schema"] = "grimoire.ac.build",
            ["schema_version"] = 1,
            ["command"] = "build",
            ["ok"] = Ok,
            ["content_root"] = ContentRoot,
            ["compiler"] = Compiler,
            ["compiler_version"] = CompilerVersion,
            ["pack"] = PackPath,
            ["pack_bytes"] = PackPath is null ? null : PackBytes,
            ["pack_content_hash"] = PackContentHash,
            ["units"] = units,
            ["diagnostics"] = own,
            ["skipped"] = skipped,
        };
    }
}

/// <summary>
/// The asset compiler's build: discover content, drive <c>sigilc</c>, write pack and manifest
/// (Plan 0002 WP9.2, project ADR-0010 building block 4).
/// </summary>
/// <remarks>
/// Nothing here knows Sigil. The pipeline turns file-system paths into asset paths, hands the files to
/// <c>sigilc build --json</c>, passes its diagnostics through unchanged, and assembles the pack from the
/// unit bytes byte for byte as <c>grimoire_assets::PackWriter</c> would. Every diagnostic, whether from
/// the compiler or from the asset compiler itself, stops the pack: a pack either matches its sources
/// completely or is not written.
/// </remarks>
public static class BuildPipeline
{
    /// <summary>
    /// The compiler name written into every pack manifest by this tool.
    /// </summary>
    public const string CompilerName = "grimoire-ac";

    /// <summary>
    /// Largest application block of the manifest (contract §12).
    /// </summary>
    public const int MaxApplicationBytes = 64 * 1024;

    /// <summary>
    /// Runs a build.
    /// </summary>
    /// <param name="options">What to build.</param>
    /// <param name="sigilc">The compiler.</param>
    /// <param name="cancellationToken">Cancellation.</param>
    /// <returns>The result; <see cref="BuildResult.Ok"/> tells whether a pack was written.</returns>
    /// <exception cref="AssetCompilerException">The build could not run: an unreadable root, a
    /// <c>sigilc</c> that could not run, or output that cannot be written.</exception>
    public static async Task<BuildResult> RunAsync(BuildOptions options, ISigilcRunner sigilc, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(options);
        ArgumentNullException.ThrowIfNull(sigilc);
        var total = Stopwatch.StartNew();
        var diagnostics = new List<Diagnostic>();
        var compilerVersion = sigilc.Version;
        if (!options.AllowVersionMismatch && !string.Equals(compilerVersion, options.EngineVersion, StringComparison.Ordinal))
        {
            // The units are the compiler's bytes, so a tool built for another engine tag may write a pack
            // the engine refuses. The manifest records which sigilc ran either way.
            diagnostics.Add(Diagnostic.Create(
                AcCode.CompilerVersionMismatch,
                options.ContentRoot,
                $"the sigilc binary reports version `{compilerVersion}`, but this tool was built for engine version `{options.EngineVersion}`.",
                "Build sigilc from the engine tag this tool was built for, or pass `--allow-version-mismatch` if the difference is intended.",
                token: compilerVersion));
        }

        var application = ReadApplicationBlock(options.ApplicationFile);
        var outputDirectory = options.OutputDirectory;
        var unitDirectory = Path.Combine(outputDirectory, "units");
        var packPath = Path.Combine(outputDirectory, options.PackName).Replace('\\', '/');

        var discoveryWatch = Stopwatch.StartNew();
        var discovery = ContentDiscovery.Scan(options.ContentRoot, outputDirectory);
        discoveryWatch.Stop();
        diagnostics.AddRange(discovery.Diagnostics);
        if (discovery.Sources.Count == 0 && !options.AllowEmpty)
        {
            diagnostics.Add(Diagnostic.Create(
                AcCode.NoSources,
                options.ContentRoot,
                $"no `{ContentDiscovery.SourceExtension}` file below `{options.ContentRoot}`.",
                "Point the build at the content root, or pass `--allow-empty` to write an empty pack on purpose."));
        }

        var compileWatch = new Stopwatch();
        var results = new List<SigilcUnit>();
        if (discovery.Sources.Count > 0)
        {
            CreateDirectory(unitDirectory);
            var files = new List<string>(discovery.Sources.Count);
            foreach (var source in discovery.Sources)
            {
                files.Add(source.CommandLinePath);
            }

            compileWatch.Start();
            var compiled = await sigilc.BuildAsync(options.ContentRoot, unitDirectory, options.Behaviors, files, cancellationToken)
                .ConfigureAwait(false);
            compileWatch.Stop();
            results.AddRange(compiled.Units);
        }

        var reported = new Dictionary<string, SigilcUnit>(StringComparer.Ordinal);
        foreach (var unit in results)
        {
            reported[unit.Source] = unit;
        }

        var packWatch = new Stopwatch();
        var units = new List<BuiltUnit>(discovery.Sources.Count);
        var payloads = new List<(AssetPath Path, uint KindVersion, byte[] Bytes)>(discovery.Sources.Count);
        packWatch.Start();
        foreach (var source in discovery.Sources)
        {
            if (!reported.TryGetValue(source.CommandLinePath, out var unit))
            {
                throw new AssetCompilerException($"sigilc reported nothing about `{source.CommandLinePath}`");
            }

            var assetId = AssetId.FromPath(source.Path);
            if (unit.Diagnostics.Count > 0)
            {
                units.Add(new BuiltUnit(source.CommandLinePath, source.Path.Value, assetId.ToString(), null, null, null, null, unit.Diagnostics));
                continue;
            }

            var unitDiagnostics = new List<Diagnostic>();
            byte[]? bytes = null;
            uint? kindVersion = null;
            if (unit.Output is null)
            {
                unitDiagnostics.Add(Diagnostic.Create(
                    AcCode.UnitUnreadable,
                    source.CommandLinePath,
                    "sigilc reported no output file for this source, although it reported no diagnostic either.",
                    "Report this: it is a bug in the compiler or in the asset compiler, not in the content."));
            }
            else
            {
                bytes = ReadUnit(unit.Output, source, assetId, unit, unitDiagnostics, out kindVersion);
            }

            units.Add(new BuiltUnit(
                source.CommandLinePath,
                source.Path.Value,
                assetId.ToString(),
                unit.UnitId,
                unit.ContentHash,
                unit.Size,
                kindVersion,
                unitDiagnostics));
            if (bytes is not null && unitDiagnostics.Count == 0 && kindVersion is { } version)
            {
                payloads.Add((source.Path, version, bytes));
            }
        }

        var ok = diagnostics.Count == 0;
        foreach (var unit in units)
        {
            ok &= unit.Diagnostics.Count == 0;
        }

        string? writtenPack = null;
        long packBytes = 0;
        string? contentHash = null;
        if (ok)
        {
            var writer = new PackWriter(CompilerName, compilerVersion);
            writer.Application(application);
            try
            {
                foreach (var payload in payloads)
                {
                    writer.Add(payload.Path, AssetKind.Sigil, payload.KindVersion, payload.Bytes);
                }

                var pack = writer.Finish();
                var reader = PackReader.FromBytes(pack);
                CreateDirectory(outputDirectory);
                WriteFile(packPath, pack);
                writtenPack = packPath;
                packBytes = pack.Length;
                contentHash = Convert.ToHexStringLower(reader.ContentHash());
            }
            catch (PackFormatException error)
            {
                diagnostics.Add(Diagnostic.Create(
                    AcCode.PackRefused,
                    packPath,
                    $"the pack cannot be written: {error.Message}.",
                    "Reduce the content below the limits of contract §12 (at most 65536 entries, 256 MiB per entry, 1 GiB per pack)."));
                ok = false;
            }
        }

        packWatch.Stop();
        total.Stop();
        return new BuildResult(
            ok,
            options.ContentRoot,
            CompilerName,
            compilerVersion,
            units,
            diagnostics,
            discovery.Skipped,
            writtenPack,
            packBytes,
            contentHash,
            new BuildTimings(discoveryWatch.Elapsed, compileWatch.Elapsed, packWatch.Elapsed, total.Elapsed));
    }

    /// <summary>
    /// Reads the header of a compiled unit (sigil.md §10.1): the format version, which becomes the pack
    /// entry's kind version, and the unit id, which must be the asset id of the unit's path.
    /// </summary>
    /// <param name="bytes">The unit.</param>
    /// <param name="formatVersion">Its format version.</param>
    /// <param name="unitId">Its unit id.</param>
    /// <param name="contentHash">Its content hash.</param>
    /// <returns><c>null</c> if the header is a unit header, else why it is not.</returns>
    public static string? TryReadUnitHeader(ReadOnlySpan<byte> bytes, out uint formatVersion, out ulong unitId, out ulong contentHash)
    {
        formatVersion = 0;
        unitId = 0;
        contentHash = 0;
        if (bytes.Length < 40)
        {
            return $"it has {bytes.Length.ToString(CultureInfo.InvariantCulture)} bytes, less than the 40-byte header";
        }

        if (!bytes.Slice(0, 8).SequenceEqual("GRIMSIGL"u8))
        {
            return "it does not start with the magic `GRIMSIGL`";
        }

        formatVersion = BinaryPrimitives.ReadUInt32LittleEndian(bytes.Slice(8));
        unitId = BinaryPrimitives.ReadUInt64LittleEndian(bytes.Slice(16));
        contentHash = BinaryPrimitives.ReadUInt64LittleEndian(bytes.Slice(24));
        var payloadLength = BinaryPrimitives.ReadUInt64LittleEndian(bytes.Slice(32));
        if (payloadLength != (ulong)bytes.Length - 40)
        {
            return $"its payload length {payloadLength.ToString(CultureInfo.InvariantCulture)} does not match its {bytes.Length.ToString(CultureInfo.InvariantCulture)} bytes";
        }

        return unitId == 0 ? "its unit id is zero" : null;
    }

    private static byte[]? ReadUnit(
        string output,
        ContentSource source,
        AssetId assetId,
        SigilcUnit unit,
        List<Diagnostic> diagnostics,
        out uint? kindVersion)
    {
        kindVersion = null;
        byte[] bytes;
        try
        {
            bytes = File.ReadAllBytes(output);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or ArgumentException or NotSupportedException)
        {
            diagnostics.Add(Diagnostic.Create(
                AcCode.UnitUnreadable,
                source.CommandLinePath,
                $"the unit `{output}` cannot be read: {error.Message}.",
                "Make sure the output directory is writable and nothing removes units while the build runs.",
                token: output));
            return null;
        }

        var reason = TryReadUnitHeader(bytes, out var formatVersion, out var unitId, out _);
        if (reason is not null)
        {
            diagnostics.Add(Diagnostic.Create(
                AcCode.UnitUnreadable,
                source.CommandLinePath,
                $"the unit `{output}` is no SigilUnit: {reason}.",
                "Report this: the compiler wrote something the pack cannot carry.",
                token: output));
            return null;
        }

        var unitIdHex = unitId.ToString("x16", CultureInfo.InvariantCulture);
        if (unit.UnitId is { } reportedId && !string.Equals(reportedId, unitIdHex, StringComparison.Ordinal))
        {
            diagnostics.Add(Diagnostic.Create(
                AcCode.UnitUnreadable,
                source.CommandLinePath,
                $"the unit `{output}` carries the id `{unitIdHex}`, but sigilc reported `{reportedId}`.",
                "Report this: compiler output and unit bytes disagree.",
                token: output));
            return null;
        }

        if (assetId.Value != unitId)
        {
            diagnostics.Add(Diagnostic.Create(
                AcCode.UnitUnreadable,
                source.CommandLinePath,
                $"the unit's id `{unitIdHex}` is not the asset id `{assetId}` of `{source.Path.Value}`, so the engine would not find it in the pack.",
                "Report this: the unit id and the asset id follow the same rule (contract §11.1, §12) and must agree.",
                token: source.Path.Value));
            return null;
        }

        if (unit.Size is { } size && size != bytes.Length)
        {
            diagnostics.Add(Diagnostic.Create(
                AcCode.UnitUnreadable,
                source.CommandLinePath,
                $"the unit `{output}` has {bytes.Length.ToString(CultureInfo.InvariantCulture)} bytes, but sigilc reported {size.ToString(CultureInfo.InvariantCulture)}.",
                "Make sure nothing writes into the output directory while the build runs.",
                token: output));
            return null;
        }

        kindVersion = formatVersion;
        return bytes;
    }

    private static byte[] ReadApplicationBlock(string? path)
    {
        if (path is null or { Length: 0 })
        {
            return Array.Empty<byte>();
        }

        byte[] bytes;
        try
        {
            bytes = File.ReadAllBytes(path);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or ArgumentException or NotSupportedException)
        {
            throw new AssetCompilerException($"cannot read the application block `{path}`: {error.Message}", error);
        }

        return bytes.Length <= MaxApplicationBytes
            ? bytes
            : throw new AssetCompilerException(
                $"the application block `{path}` has {bytes.Length.ToString(CultureInfo.InvariantCulture)} bytes, more than the 64 KiB of contract §12");
    }

    private static void CreateDirectory(string path)
    {
        try
        {
            Directory.CreateDirectory(path);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or ArgumentException or NotSupportedException)
        {
            throw new AssetCompilerException($"cannot create the directory `{path}`: {error.Message}", error);
        }
    }

    private static void WriteFile(string path, byte[] bytes)
    {
        try
        {
            File.WriteAllBytes(path, bytes);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or ArgumentException or NotSupportedException)
        {
            throw new AssetCompilerException($"cannot write `{path}`: {error.Message}", error);
        }
    }
}
