using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;
using Grimoire.Formats.Pack;
using Grimoire.LiveLink;

namespace Grimoire.AssetCompiler;

/// <summary>
/// What <c>watch --push</c> should do.
/// </summary>
public sealed class WatchOptions
{
    /// <summary>
    /// Creates the options.
    /// </summary>
    /// <param name="contentRoot">The content root, as the user wrote it.</param>
    public WatchOptions(string contentRoot)
    {
        ArgumentNullException.ThrowIfNull(contentRoot);
        ContentRoot = contentRoot;
    }

    /// <summary>
    /// The content root, as the user wrote it.
    /// </summary>
    public string ContentRoot { get; }

    /// <summary>
    /// Where the compiled units go; a build artefact, as in a full build.
    /// </summary>
    public string OutputDirectory { get; init; } = "packs";

    /// <summary>
    /// A behaviour manifest (sigil.md §13.3), or <c>null</c>.
    /// </summary>
    public string? Behaviors { get; init; }

    /// <summary>
    /// How long to wait for the engine's <c>SwapAck</c>.
    /// </summary>
    public TimeSpan SwapTimeout { get; init; } = TimeSpan.FromSeconds(2);

    /// <summary>
    /// How long to collect further changes before compiling, so one save that arrives as several file
    /// events is compiled once.
    /// </summary>
    public TimeSpan Debounce { get; init; } = TimeSpan.FromMilliseconds(150);
}

/// <summary>
/// What happened to one changed file.
/// </summary>
public enum WatchOutcomeKind
{
    /// <summary>
    /// The file is gone, or its path is no asset path; nothing was compiled.
    /// </summary>
    Skipped,

    /// <summary>
    /// The compiler reported diagnostics; the engine keeps the unit it has.
    /// </summary>
    Diagnostics,

    /// <summary>
    /// The unit was compiled and the swap was answered by the engine.
    /// </summary>
    Pushed,
}

/// <summary>
/// The result of one compile-and-push round.
/// </summary>
/// <param name="Kind">What happened.</param>
/// <param name="CommandLinePath">The file, as it appears in diagnostics.</param>
/// <param name="AssetPath">Its asset path, or <c>null</c> if none could be formed.</param>
/// <param name="UnitBytes">The compiled unit's length, or <c>0</c>.</param>
/// <param name="Diagnostics">The diagnostics, unchanged.</param>
/// <param name="Swap">The swap result, or <c>null</c> if no swap was attempted.</param>
/// <param name="Line">The line the session printed.</param>
public sealed record WatchOutcome(
    WatchOutcomeKind Kind,
    string CommandLinePath,
    string? AssetPath,
    int UnitBytes,
    IReadOnlyList<Diagnostic> Diagnostics,
    SwapResult? Swap,
    string Line);

/// <summary>
/// The <c>watch --push</c> mode (Plan 0002 WP9.2): compile the one unit that changed and swap it into the
/// running engine over the live link.
/// </summary>
/// <remarks>
/// The session never throws because of the engine or the content: a diagnostic keeps the engine's current
/// unit and is printed, and a missing link yields <see cref="SwapStatus.NotConnected"/>, which means the
/// compiled unit on disk stays the source of truth — the tool degrades to file work (PRD-0016 FR-03). The
/// changed paths arrive through a channel, so the loop is driven by a real
/// <see cref="FileSystemWatcher"/> in the CLI and directly by the tests.
/// </remarks>
public sealed class WatchSession
{
    private readonly WatchOptions _options;
    private readonly ISigilcRunner _sigilc;
    private readonly LiveLinkClient? _client;
    private readonly TextWriter _output;
    private readonly Action<WatchOutcome>? _observer;
    private readonly string _fullRoot;
    private readonly string _displayRoot;
    private readonly string _unitDirectory;

    /// <summary>
    /// Creates a session.
    /// </summary>
    /// <param name="options">What to watch.</param>
    /// <param name="sigilc">The compiler.</param>
    /// <param name="client">The live link, or <c>null</c> to compile without pushing.</param>
    /// <param name="output">Where lines go.</param>
    /// <param name="observer">Called with every outcome, for tests and for a later UI.</param>
    public WatchSession(WatchOptions options, ISigilcRunner sigilc, LiveLinkClient? client, TextWriter output, Action<WatchOutcome>? observer = null)
    {
        ArgumentNullException.ThrowIfNull(options);
        ArgumentNullException.ThrowIfNull(sigilc);
        ArgumentNullException.ThrowIfNull(output);
        _options = options;
        _sigilc = sigilc;
        _client = client;
        _output = output;
        _observer = observer;
        _fullRoot = Path.GetFullPath(options.ContentRoot);
        _displayRoot = options.ContentRoot.Replace('\\', '/').TrimEnd('/');
        _unitDirectory = Path.Combine(options.OutputDirectory, "units");
    }

    /// <summary>
    /// Reads changed paths until <paramref name="cancellationToken"/> is cancelled or the channel
    /// completes, coalescing the events of one save.
    /// </summary>
    /// <param name="changes">Changed paths, absolute or relative to the working directory.</param>
    /// <param name="cancellationToken">Cancellation.</param>
    /// <returns>A task that completes when the channel does.</returns>
    public async Task RunAsync(ChannelReader<string> changes, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(changes);
        try
        {
            while (true)
            {
                if (!await changes.WaitToReadAsync(cancellationToken).ConfigureAwait(false))
                {
                    return;
                }

                var pending = new SortedSet<string>(StringComparer.Ordinal);
                Drain(changes, pending);
                if (_options.Debounce > TimeSpan.Zero)
                {
                    await Task.Delay(_options.Debounce, cancellationToken).ConfigureAwait(false);
                    Drain(changes, pending);
                }

                foreach (var path in pending)
                {
                    if (cancellationToken.IsCancellationRequested)
                    {
                        return;
                    }

                    var outcome = await CompileAndPushAsync(path, cancellationToken).ConfigureAwait(false);
                    await _output.WriteLineAsync(outcome.Line).ConfigureAwait(false);
                    await _output.FlushAsync(cancellationToken).ConfigureAwait(false);
                    _observer?.Invoke(outcome);
                }
            }
        }
        catch (OperationCanceledException)
        {
            // Ctrl+C, or the caller stopping the watch: the loop ends, the link and the watcher are closed
            // by their owners, and nothing is left half-written — a unit is either compiled or it is not.
        }
    }

    /// <summary>
    /// Compiles one file and, if it compiled, swaps it into the engine.
    /// </summary>
    /// <param name="path">The changed file.</param>
    /// <param name="cancellationToken">Cancellation.</param>
    /// <returns>What happened.</returns>
    /// <exception cref="AssetCompilerException">The compiler could not run at all.</exception>
    public async Task<WatchOutcome> CompileAndPushAsync(string path, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(path);
        var fullPath = Path.GetFullPath(path);
        var relative = Path.GetRelativePath(_fullRoot, fullPath).Replace(Path.DirectorySeparatorChar, '/');
        var commandLinePath = _displayRoot.Length == 0 ? relative : $"{_displayRoot}/{relative}";
        if (relative.StartsWith("../", StringComparison.Ordinal) || relative == "..")
        {
            return Skip(commandLinePath, null, $"{commandLinePath}: outside the content root, not compiled");
        }

        if (!File.Exists(fullPath))
        {
            return Skip(commandLinePath, null, $"{commandLinePath}: gone, nothing pushed (the engine keeps the unit it has)");
        }

        if (!AssetPath.TryCreate(relative, out var assetPath, out var reason))
        {
            var diagnostic = Diagnostic.Create(
                AcCode.InvalidPath,
                commandLinePath,
                $"`{relative}` is no asset path below the content root: {reason}.",
                "Rename the file and its directories to lowercase ASCII `[a-z0-9_.-]`.",
                token: relative);
            await _output.WriteLineAsync(diagnostic.RenderText()).ConfigureAwait(false);
            return Skip(commandLinePath, null, $"{commandLinePath}: no asset path, nothing pushed");
        }

        Directory.CreateDirectory(_unitDirectory);
        var compiled = await _sigilc
            .BuildAsync(_options.ContentRoot, _unitDirectory, _options.Behaviors, new[] { commandLinePath }, cancellationToken)
            .ConfigureAwait(false);
        if (compiled.Units.Count != 1)
        {
            throw new AssetCompilerException($"sigilc reported {compiled.Units.Count.ToString(CultureInfo.InvariantCulture)} units for one file");
        }

        var unit = compiled.Units[0];
        if (unit.Diagnostics.Count > 0)
        {
            foreach (var diagnostic in unit.Diagnostics)
            {
                await _output.WriteLineAsync(diagnostic.RenderText()).ConfigureAwait(false);
            }

            return new WatchOutcome(
                WatchOutcomeKind.Diagnostics,
                commandLinePath,
                assetPath!.Value,
                0,
                unit.Diagnostics,
                null,
                $"{commandLinePath}: {unit.Diagnostics.Count.ToString(CultureInfo.InvariantCulture)} diagnostic(s), nothing pushed (the engine keeps the unit it has)");
        }

        if (unit.Output is null)
        {
            return Skip(commandLinePath, assetPath!.Value, $"{commandLinePath}: sigilc wrote no unit, nothing pushed");
        }

        byte[] bytes;
        try
        {
            bytes = await File.ReadAllBytesAsync(unit.Output, cancellationToken).ConfigureAwait(false);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
            return Skip(commandLinePath, assetPath!.Value, $"{commandLinePath}: cannot read `{unit.Output}` ({error.Message}), nothing pushed");
        }

        var headerProblem = BuildPipeline.TryReadUnitHeader(bytes, out _, out _, out var contentHash);
        if (headerProblem is not null)
        {
            return Skip(commandLinePath, assetPath!.Value, $"{commandLinePath}: `{unit.Output}` is no SigilUnit ({headerProblem}), nothing pushed");
        }

        var swap = _client is null
            ? new SwapResult(SwapStatus.NotConnected)
            : await _client.SwapSigilUnitAsync(assetPath!.Value, bytes, _options.SwapTimeout, cancellationToken).ConfigureAwait(false);
        var line = new StringBuilder()
            .Append(CultureInfo.InvariantCulture, $"{commandLinePath} -> {assetPath!.Value} ")
            .Append(CultureInfo.InvariantCulture, $"(content hash {contentHash.ToString("x16", CultureInfo.InvariantCulture)}, {bytes.Length.ToString(CultureInfo.InvariantCulture)} bytes): ")
            .Append(Describe(swap))
            .ToString();
        return new WatchOutcome(WatchOutcomeKind.Pushed, commandLinePath, assetPath.Value, bytes.Length, Array.Empty<Diagnostic>(), swap, line);
    }

    /// <summary>
    /// Says in one phrase how a swap went, including that the file on disk stays the source of truth when
    /// there is no link.
    /// </summary>
    /// <param name="swap">The swap result.</param>
    /// <returns>The phrase.</returns>
    public static string Describe(SwapResult swap)
    {
        ArgumentNullException.ThrowIfNull(swap);
        return swap.Status switch
        {
            SwapStatus.Applied => $"applied at tick {(swap.Ack?.AppliedTick ?? 0).ToString(CultureInfo.InvariantCulture)}, content epoch {(swap.Ack?.ContentManifest ?? 0).ToString("x16", CultureInfo.InvariantCulture)}",
            SwapStatus.Rejected => $"rejected by the engine: {swap.Ack?.Reason ?? "(no reason)"}",
            SwapStatus.Superseded => "superseded by a later swap of the same unit",
            SwapStatus.EngineError => $"engine error: {swap.Error?.Message ?? "(no message)"}",
            SwapStatus.NotConnected => "no link, the unit on disk stays the source of truth",
            SwapStatus.ConnectionLost => "the link dropped before the engine answered; the unit on disk stays the source of truth",
            SwapStatus.TimedOut => "the engine did not answer in time; the unit on disk stays the source of truth",
            _ => "unknown swap result",
        };
    }

    private static void Drain(ChannelReader<string> changes, SortedSet<string> pending)
    {
        while (changes.TryRead(out var path))
        {
            pending.Add(path);
        }
    }

    private WatchOutcome Skip(string commandLinePath, string? assetPath, string line) =>
        new(WatchOutcomeKind.Skipped, commandLinePath, assetPath, 0, Array.Empty<Diagnostic>(), null, line);
}

/// <summary>
/// Turns file-system events below a content root into a channel of changed paths.
/// </summary>
/// <remarks>
/// Only <c>*.sigil</c> files are reported, and a path is reported as often as the operating system raises
/// an event for it; <see cref="WatchSession"/> coalesces them. Watching is best effort: if the watcher
/// overflows its buffer, the error is reported and watching continues.
/// </remarks>
public sealed class FileChangeWatcher : IDisposable
{
    private readonly FileSystemWatcher _watcher;
    private readonly Channel<string> _changes = Channel.CreateUnbounded<string>(new UnboundedChannelOptions
    {
        SingleReader = true,
    });

    /// <summary>
    /// Starts watching.
    /// </summary>
    /// <param name="contentRoot">The directory to watch.</param>
    /// <param name="errors">Where watcher errors are reported.</param>
    /// <exception cref="AssetCompilerException">The directory cannot be watched.</exception>
    public FileChangeWatcher(string contentRoot, TextWriter errors)
    {
        ArgumentNullException.ThrowIfNull(contentRoot);
        ArgumentNullException.ThrowIfNull(errors);
        try
        {
            _watcher = new FileSystemWatcher(Path.GetFullPath(contentRoot), "*" + ContentDiscovery.SourceExtension)
            {
                IncludeSubdirectories = true,
                NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.FileName | NotifyFilters.Size,
            };
        }
        catch (Exception error) when (error is ArgumentException or FileNotFoundException or IOException)
        {
            throw new AssetCompilerException($"cannot watch `{contentRoot}`: {error.Message}", error);
        }

        _watcher.Changed += (_, e) => _changes.Writer.TryWrite(e.FullPath);
        _watcher.Created += (_, e) => _changes.Writer.TryWrite(e.FullPath);
        _watcher.Renamed += (_, e) => _changes.Writer.TryWrite(e.FullPath);
        _watcher.Deleted += (_, e) => _changes.Writer.TryWrite(e.FullPath);
        _watcher.Error += (_, e) => errors.WriteLine($"grimoire-ac: the file watcher reported `{e.GetException().Message}`; watching continues");
        _watcher.EnableRaisingEvents = true;
    }

    /// <summary>
    /// The changed paths.
    /// </summary>
    public ChannelReader<string> Changes => _changes.Reader;

    /// <inheritdoc/>
    public void Dispose()
    {
        _watcher.EnableRaisingEvents = false;
        _watcher.Dispose();
        _changes.Writer.TryComplete();
    }
}
