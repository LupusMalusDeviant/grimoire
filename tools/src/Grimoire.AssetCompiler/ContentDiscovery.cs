using System;
using System.Collections.Generic;
using System.IO;
using Grimoire.Formats.Pack;

namespace Grimoire.AssetCompiler;

/// <summary>
/// One discovered source file.
/// </summary>
/// <param name="FullPath">Where the file is on disk.</param>
/// <param name="CommandLinePath">How the file is named on the <c>sigilc</c> command line, and therefore in
/// every diagnostic about it: the content root as the user wrote it, plus the file's relative path.</param>
/// <param name="Path">The asset path of the compiled unit; its <c>AssetId</c> is the unit id
/// (contract §11.1, §12).</param>
public sealed record ContentSource(string FullPath, string CommandLinePath, AssetPath Path);

/// <summary>
/// A file below the content root that is not a source.
/// </summary>
/// <param name="CommandLinePath">The file, named like a source would be.</param>
/// <param name="Reason">Why it was passed over.</param>
public sealed record SkippedFile(string CommandLinePath, string Reason);

/// <summary>
/// What a scan of the content root found.
/// </summary>
/// <param name="Sources">The sources, ascending by asset path.</param>
/// <param name="Skipped">Files that are not sources, ascending by path.</param>
/// <param name="Diagnostics">Problems with the files that are there (<see cref="AcCode.InvalidPath"/>,
/// <see cref="AcCode.PathCollision"/>).</param>
public sealed record DiscoveryResult(
    IReadOnlyList<ContentSource> Sources,
    IReadOnlyList<SkippedFile> Skipped,
    IReadOnlyList<Diagnostic> Diagnostics);

/// <summary>
/// Finds the compilable content below a content root (Plan 0002 WP9.2).
/// </summary>
/// <remarks>
/// The scan takes the tree as it is: every <c>.sigil</c> file anywhere below the root is a source, every
/// other file is reported as skipped, and nothing is expected at a fixed place. Path separators are
/// normalised to <c>/</c> (project ADR-0010 building block 4: the asset compiler normalises), and a path
/// that still is no valid <c>AssetPath</c> — an uppercase letter, a space, a non-ASCII character — is
/// <see cref="AcCode.InvalidPath"/> and names the rename that fixes it, because the unit id derives from
/// that path and <c>sigilc</c> refuses to guess one (contract §12, sigil.md §13.2).
/// </remarks>
public static class ContentDiscovery
{
    /// <summary>
    /// Extension of a Sigil source, compared without regard to case.
    /// </summary>
    public const string SourceExtension = ".sigil";

    /// <summary>
    /// Scans <paramref name="contentRoot"/>.
    /// </summary>
    /// <param name="contentRoot">The content root as the user wrote it.</param>
    /// <param name="excludedDirectory">A directory not to descend into, e.g. the output directory when it
    /// sits below the content root; <c>null</c> for none.</param>
    /// <returns>What was found.</returns>
    /// <exception cref="AssetCompilerException">The root does not exist, is not a directory, or cannot be
    /// read.</exception>
    public static DiscoveryResult Scan(string contentRoot, string? excludedDirectory = null)
    {
        ArgumentNullException.ThrowIfNull(contentRoot);
        string fullRoot;
        try
        {
            fullRoot = Path.GetFullPath(contentRoot);
        }
        catch (Exception error) when (error is ArgumentException or NotSupportedException or PathTooLongException)
        {
            throw new AssetCompilerException($"`{contentRoot}` is not a usable path: {error.Message}", error);
        }

        if (!Directory.Exists(fullRoot))
        {
            throw new AssetCompilerException($"the content root `{contentRoot}` does not exist or is not a directory");
        }

        var excluded = excludedDirectory is null ? null : Path.GetFullPath(excludedDirectory);
        var files = new List<string>();
        Collect(fullRoot, excluded, files);
        files.Sort(StringComparer.Ordinal);
        var found = new List<(string FullPath, string Relative)>(files.Count);
        foreach (var file in files)
        {
            // Only the platform's own separator becomes `/`: on Linux a backslash is a legal character in a
            // file name and must stay one, or two different files could claim one asset path.
            found.Add((file, Path.GetRelativePath(fullRoot, file).Replace(Path.DirectorySeparatorChar, '/')));
        }

        return Classify(contentRoot, found);
    }

    /// <summary>
    /// Turns relative paths into sources, skipped files and diagnostics, without touching the file system.
    /// </summary>
    /// <param name="contentRoot">The content root as the user wrote it.</param>
    /// <param name="found">The files below it, with their relative paths (separator <c>/</c>).</param>
    /// <returns>What they are.</returns>
    internal static DiscoveryResult Classify(string contentRoot, IReadOnlyList<(string FullPath, string Relative)> found)
    {
        var display = contentRoot.Replace('\\', '/').TrimEnd('/');
        var sources = new List<ContentSource>();
        var skipped = new List<SkippedFile>();
        var diagnostics = new List<Diagnostic>();
        var byPath = new Dictionary<string, ContentSource>(StringComparer.Ordinal);

        foreach (var (file, relative) in found)
        {
            var commandLinePath = display.Length == 0 ? relative : $"{display}/{relative}";
            if (!file.EndsWith(SourceExtension, StringComparison.OrdinalIgnoreCase))
            {
                skipped.Add(new SkippedFile(commandLinePath, $"not a Sigil source (`{SourceExtension}`)"));
                continue;
            }

            if (!AssetPath.TryCreate(relative, out var path, out var reason))
            {
                diagnostics.Add(Diagnostic.Create(
                    AcCode.InvalidPath,
                    commandLinePath,
                    $"`{relative}` is no asset path below the content root: {reason}.",
                    "Rename the file and its directories to lowercase ASCII `[a-z0-9_.-]`; the unit id derives from this path, so the compiler does not rename it for you.",
                    token: relative));
                continue;
            }

            var source = new ContentSource(file, commandLinePath, path!);
            if (byPath.TryGetValue(path!.Value, out var first))
            {
                diagnostics.Add(Diagnostic.Create(
                    AcCode.PathCollision,
                    commandLinePath,
                    $"`{relative}` has the same asset path as `{first.CommandLinePath}`, so one would shadow the other in the pack.",
                    "Give the two files different paths below the content root.",
                    token: path.Value));
                continue;
            }

            byPath.Add(path.Value, source);
            sources.Add(source);
        }

        sources.Sort(static (a, b) => string.CompareOrdinal(a.Path.Value, b.Path.Value));
        return new DiscoveryResult(sources, skipped, diagnostics);
    }

    private static void Collect(string directory, string? excluded, List<string> files)
    {
        IEnumerable<string> entries;
        try
        {
            entries = Directory.EnumerateFileSystemEntries(directory);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
            throw new AssetCompilerException($"cannot read the directory `{directory}`: {error.Message}", error);
        }

        foreach (var entry in entries)
        {
            var name = Path.GetFileName(entry);
            if (name.StartsWith('.'))
            {
                continue;
            }

            if (Directory.Exists(entry))
            {
                if (excluded is not null && string.Equals(Path.GetFullPath(entry), excluded, StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }

                Collect(entry, excluded, files);
                continue;
            }

            files.Add(entry);
        }
    }
}
