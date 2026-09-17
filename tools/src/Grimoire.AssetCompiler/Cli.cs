using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading;
using System.Threading.Tasks;
using Grimoire.Formats;
using Grimoire.Formats.Debug;
using Grimoire.LiveLink;

namespace Grimoire.AssetCompiler;

/// <summary>
/// The command line of <c>grimoire-ac</c> (Plan 0002 WP9.2).
/// </summary>
/// <remarks>
/// The commands, the option grammar and the three exit codes follow <c>sigilc</c> (sigil.md §13.1), so a
/// script drives both the same way: <c>0</c> success, <c>1</c> the command ran and found problems,
/// <c>2</c> the command could not run as asked. Diagnostics are printed in the text form of sigil.md §6.1
/// or, with <c>--json</c>, as the report document with every <c>sigilc</c> diagnostic unchanged inside.
/// </remarks>
public static class Cli
{
    /// <summary>
    /// Exit code for a successful run.
    /// </summary>
    public const int ExitSuccess = 0;

    /// <summary>
    /// Exit code for a run that found problems in the content.
    /// </summary>
    public const int ExitDiagnostics = 1;

    /// <summary>
    /// Exit code for a run that could not be carried out.
    /// </summary>
    public const int ExitUsage = 2;

    /// <summary>
    /// The usage text.
    /// </summary>
    public const string Usage = """
        grimoire-ac — asset compiler of the Grimoire engine (Plan 0002 WP9.2, project ADR-0010)

        Usage:
          grimoire-ac build <content-dir> [options]
          grimoire-ac push <file> --root <content-dir> [options]
          grimoire-ac watch --push <content-dir> [options]
          grimoire-ac --version | --help

        build   Compiles every `.sigil` file below <content-dir> with `sigilc build --json` and writes
                pack and manifest. Every diagnostic — the compiler's or the asset compiler's own — stops
                the pack.
        push    Compiles one file and swaps it into a running engine over the live link.
        watch   Watches <content-dir> and pushes every file that changes.

        Options:
          --out <dir>                Where pack and units go (default `packs`); a build artefact.
          --pack <name>              File name of the pack below --out (default `content.grimpack`).
          --root <dir>               Content root of `push` (default: the file's own directory).
          --sigilc <path>            The `sigilc` binary (default: $GRIMOIRE_SIGILC, else PATH).
          --behaviors <file>         Behaviour manifest for `behaviour = <name>` (sigil.md §13.3).
          --application <file>       Opaque application block of the manifest, at most 64 KiB.
          --address 127.0.0.1:<port> The engine's debug link (default: $GRIMOIRE_DEBUG_ADDR, else 7878).
          --token <64 hex digits>    The debug-link token (default: $GRIMOIRE_DEBUG_TOKEN).
          --swap-timeout <ms>        How long to wait for the engine's answer (default 2000).
          --debounce <ms>            How long `watch` collects further changes (default 150).
          --json                     Print the report document instead of text.
          --timings                  Print how long the phases took, to standard error.
          --allow-empty              Write an empty pack instead of failing on empty content.
          --allow-version-mismatch   Accept a `sigilc` of another version than this build's engine.

        Exit codes: 0 success, 1 diagnostics or a refused swap, 2 the command could not run.
        """;

    /// <summary>
    /// Runs one command.
    /// </summary>
    /// <param name="arguments">The command line without the program name.</param>
    /// <param name="stdout">Where output goes.</param>
    /// <param name="stderr">Where messages about the run itself go.</param>
    /// <param name="cancellationToken">Cancellation; <c>watch</c> stops when it is cancelled.</param>
    /// <returns>The exit code.</returns>
    public static async Task<int> RunAsync(IReadOnlyList<string> arguments, TextWriter stdout, TextWriter stderr, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(arguments);
        ArgumentNullException.ThrowIfNull(stdout);
        ArgumentNullException.ThrowIfNull(stderr);
        try
        {
            return await DispatchAsync(arguments, stdout, stderr, cancellationToken).ConfigureAwait(false);
        }
        catch (UsageException error)
        {
            await stderr.WriteLineAsync($"grimoire-ac: {error.Message}").ConfigureAwait(false);
            await stderr.WriteLineAsync("Run `grimoire-ac --help` for usage.").ConfigureAwait(false);
            return ExitUsage;
        }
        catch (AssetCompilerException error)
        {
            await stderr.WriteLineAsync($"grimoire-ac: {error.Message}").ConfigureAwait(false);
            return ExitUsage;
        }
        catch (OperationCanceledException)
        {
            return ExitSuccess;
        }
    }

    private static async Task<int> DispatchAsync(IReadOnlyList<string> arguments, TextWriter stdout, TextWriter stderr, CancellationToken cancellationToken)
    {
        if (arguments.Count == 0)
        {
            await stderr.WriteLineAsync(Usage).ConfigureAwait(false);
            return ExitUsage;
        }

        switch (arguments[0])
        {
            case "--help" or "-h" or "help":
                await stdout.WriteLineAsync(Usage).ConfigureAwait(false);
                return ExitSuccess;
            case "--version":
                await stdout.WriteLineAsync($"grimoire-ac {EngineBuild.EngineVersion} (engine build {EngineBuild.BuildHash})").ConfigureAwait(false);
                return ExitSuccess;
            case "build":
                return await BuildCommandAsync(Options.Parse(arguments, 1), stdout, stderr, cancellationToken).ConfigureAwait(false);
            case "push":
                return await PushCommandAsync(Options.Parse(arguments, 1), stdout, stderr, cancellationToken).ConfigureAwait(false);
            case "watch":
                return await WatchCommandAsync(Options.Parse(arguments, 1), stdout, stderr, cancellationToken).ConfigureAwait(false);
            default:
                throw new UsageException($"unknown command `{arguments[0]}`");
        }
    }

    private static async Task<int> BuildCommandAsync(Options options, TextWriter stdout, TextWriter stderr, CancellationToken cancellationToken)
    {
        var contentRoot = options.SinglePositional("build", "<content-dir>");
        options.Reject("address", "token", "debounce", "swap-timeout", "root", "push");
        var buildOptions = new BuildOptions(contentRoot)
        {
            OutputDirectory = options.Value("out") ?? "packs",
            PackName = options.Value("pack") ?? "content.grimpack",
            Behaviors = options.Value("behaviors"),
            ApplicationFile = options.Value("application"),
            AllowEmpty = options.Flag("allow-empty"),
            AllowVersionMismatch = options.Flag("allow-version-mismatch"),
        };
        var json = options.Flag("json");
        var timings = options.Flag("timings");
        var sigilcPath = options.Value("sigilc");
        options.Finish();
        if (!Directory.Exists(contentRoot))
        {
            throw new AssetCompilerException($"the content root `{contentRoot}` does not exist or is not a directory");
        }

        var sigilc = new SigilcProcess(SigilcProcess.Locate(sigilcPath));
        var result = await BuildPipeline.RunAsync(buildOptions, sigilc, cancellationToken).ConfigureAwait(false);
        if (json)
        {
            await stdout.WriteLineAsync(Render(result.ToJson())).ConfigureAwait(false);
        }
        else
        {
            foreach (var diagnostic in result.AllDiagnostics())
            {
                await stdout.WriteLineAsync(diagnostic.RenderText()).ConfigureAwait(false);
            }

            if (result.Ok)
            {
                await stdout.WriteLineAsync(
                    $"built {result.Units.Count.ToString(CultureInfo.InvariantCulture)} unit(s) -> {result.PackPath} " +
                    $"({result.PackBytes.ToString(CultureInfo.InvariantCulture)} bytes, content hash {result.PackContentHash}, compiler {result.Compiler} {result.CompilerVersion})")
                    .ConfigureAwait(false);
            }
            else
            {
                await stderr.WriteLineAsync(
                    $"grimoire-ac: {result.AllDiagnostics().Count.ToString(CultureInfo.InvariantCulture)} diagnostic(s); no pack written")
                    .ConfigureAwait(false);
            }
        }

        if (timings)
        {
            var t = result.Timings;
            await stderr.WriteLineAsync(
                $"grimoire-ac timings: discovery {Milliseconds(t.Discovery)} ms, compile {Milliseconds(t.Compile)} ms, " +
                $"pack {Milliseconds(t.Pack)} ms, total {Milliseconds(t.Total)} ms " +
                $"({result.Units.Count.ToString(CultureInfo.InvariantCulture)} unit(s), {result.PackBytes.ToString(CultureInfo.InvariantCulture)} pack bytes)")
                .ConfigureAwait(false);
        }

        return result.Ok ? ExitSuccess : ExitDiagnostics;
    }

    private static async Task<int> PushCommandAsync(Options options, TextWriter stdout, TextWriter stderr, CancellationToken cancellationToken)
    {
        var file = options.SinglePositional("push", "<file>");
        options.Reject("pack", "application", "allow-empty", "allow-version-mismatch", "json", "timings", "debounce", "push");
        var root = options.Value("root") ?? Path.GetDirectoryName(Path.GetFullPath(file)) ?? ".";
        var watchOptions = new WatchOptions(root)
        {
            OutputDirectory = options.Value("out") ?? "packs",
            Behaviors = options.Value("behaviors"),
            SwapTimeout = TimeSpan.FromMilliseconds(options.Integer("swap-timeout", 2000, 1, 600_000)),
            Debounce = TimeSpan.Zero,
        };
        var addressText = options.Value("address");
        var tokenText = options.Value("token");
        var sigilcPath = options.Value("sigilc");
        options.Finish();

        var (connector, address) = Connector(addressText);
        var linkOptions = LinkOptions(tokenText);
        var sigilc = new SigilcProcess(SigilcProcess.Locate(sigilcPath));
        await using var client = new LiveLinkClient(connector, linkOptions);
        client.Start();
        var session = new WatchSession(watchOptions, sigilc, client, stdout);
        await stderr.WriteLineAsync($"grimoire-ac: pushing to {address}").ConfigureAwait(false);
        var outcome = await session.CompileAndPushAsync(file, cancellationToken).ConfigureAwait(false);
        await stdout.WriteLineAsync(outcome.Line).ConfigureAwait(false);
        await client.StopAsync().ConfigureAwait(false);
        return outcome.Kind == WatchOutcomeKind.Pushed && outcome.Swap is { Status: SwapStatus.Applied }
            ? ExitSuccess
            : ExitDiagnostics;
    }

    private static async Task<int> WatchCommandAsync(Options options, TextWriter stdout, TextWriter stderr, CancellationToken cancellationToken)
    {
        if (!options.Flag("push"))
        {
            throw new UsageException("`watch` needs `--push`: it compiles a changed unit and swaps it into a running engine");
        }

        var contentRoot = options.SinglePositional("watch", "<content-dir>");
        options.Reject("pack", "application", "allow-empty", "allow-version-mismatch", "json", "timings", "root");
        var watchOptions = new WatchOptions(contentRoot)
        {
            OutputDirectory = options.Value("out") ?? "packs",
            Behaviors = options.Value("behaviors"),
            SwapTimeout = TimeSpan.FromMilliseconds(options.Integer("swap-timeout", 2000, 1, 600_000)),
            Debounce = TimeSpan.FromMilliseconds(options.Integer("debounce", 150, 0, 60_000)),
        };
        var addressText = options.Value("address");
        var tokenText = options.Value("token");
        var sigilcPath = options.Value("sigilc");
        options.Finish();

        var (connector, address) = Connector(addressText);
        var linkOptions = LinkOptions(tokenText);
        if (!Directory.Exists(contentRoot))
        {
            throw new AssetCompilerException($"the content root `{contentRoot}` does not exist or is not a directory");
        }

        var sigilc = new SigilcProcess(SigilcProcess.Locate(sigilcPath));
        await using var client = new LiveLinkClient(connector, linkOptions);
        client.StateChanged += (_, e) => stderr.WriteLine($"grimoire-ac: link {e.State.ToString().ToLowerInvariant()}{(e.Reason is null ? string.Empty : $" ({e.Reason})")}");
        client.Start();
        using var watcher = new FileChangeWatcher(contentRoot, stderr);
        await stderr.WriteLineAsync($"grimoire-ac: watching {contentRoot} and pushing to {address}; stop with Ctrl+C").ConfigureAwait(false);
        var session = new WatchSession(watchOptions, sigilc, client, stdout);
        await session.RunAsync(watcher.Changes, cancellationToken).ConfigureAwait(false);
        await client.StopAsync().ConfigureAwait(false);
        return ExitSuccess;
    }

    private static (ILinkConnector Connector, string Address) Connector(string? text)
    {
        if (text is null)
        {
            try
            {
                var fromEnvironment = TcpLinkConnector.FromEnvironment();
                return (fromEnvironment, $"127.0.0.1:{fromEnvironment.Port.ToString(CultureInfo.InvariantCulture)}");
            }
            catch (FormatException badAddress)
            {
                throw new AssetCompilerException(badAddress.Message, badAddress);
            }
        }

        if (!TcpLinkConnector.TryParseAddress(text, out var port, out var error))
        {
            throw new UsageException($"--address: {error}");
        }

        return (new TcpLinkConnector(port), $"127.0.0.1:{port.ToString(CultureInfo.InvariantCulture)}");
    }

    private static LiveLinkOptions LinkOptions(string? token)
    {
        if (token is not null)
        {
            try
            {
                return new LiveLinkOptions(new ToolIdentity(LiveLinkOptions.ParseToken(token)));
            }
            catch (FormatException error)
            {
                throw new UsageException($"--token: {error.Message}");
            }
        }

        LiveLinkOptions? fromEnvironment;
        try
        {
            fromEnvironment = LiveLinkOptions.FromEnvironment();
        }
        catch (FormatException error)
        {
            throw new AssetCompilerException($"{DebugProtocol.DebugTokenEnvironmentVariable}: {error.Message}", error);
        }

        return fromEnvironment
            ?? throw new AssetCompilerException(
                $"no debug-link token: pass `--token <64 hex digits>` or set {DebugProtocol.DebugTokenEnvironmentVariable}. " +
                "The engine binds no debug link without one either (contract §13).");
    }

    private static string Milliseconds(TimeSpan span) =>
        span.TotalMilliseconds.ToString("F1", CultureInfo.InvariantCulture);

    private static string Render(JsonObject document) =>
        document.ToJsonString(new JsonSerializerOptions { WriteIndented = true });

    /// <summary>
    /// The command line of one command: options in <c>--name value</c> or <c>--name=value</c> form
    /// anywhere after the command, <c>--</c> ending option parsing, and every option read at most once.
    /// </summary>
    private sealed class Options
    {
        private readonly Dictionary<string, string?> _options = new(StringComparer.Ordinal);
        private readonly List<string> _positional = new();
        private readonly HashSet<string> _read = new(StringComparer.Ordinal);

        public static Options Parse(IReadOnlyList<string> arguments, int start)
        {
            var options = new Options();
            var literal = false;
            for (var index = start; index < arguments.Count; index++)
            {
                var argument = arguments[index];
                if (literal || !argument.StartsWith("--", StringComparison.Ordinal))
                {
                    options._positional.Add(argument);
                    continue;
                }

                if (argument.Length == 2)
                {
                    literal = true;
                    continue;
                }

                var body = argument.Substring(2);
                var equals = body.IndexOf('=', StringComparison.Ordinal);
                var name = equals < 0 ? body : body.Substring(0, equals);
                var value = equals < 0 ? null : body.Substring(equals + 1);
                if (value is null && index + 1 < arguments.Count && ValueOptions.Contains(name))
                {
                    value = arguments[++index];
                }

                if (!options._options.TryAdd(name, value))
                {
                    throw new UsageException($"the option `--{name}` is given more than once");
                }
            }

            return options;
        }

        private static readonly HashSet<string> ValueOptions = new(StringComparer.Ordinal)
        {
            "out", "pack", "root", "sigilc", "behaviors", "application", "address", "token", "swap-timeout", "debounce",
        };

        public string SinglePositional(string command, string what)
        {
            if (_positional.Count == 0)
            {
                throw new UsageException($"`{command}` needs {what}");
            }

            if (_positional.Count > 1)
            {
                throw new UsageException($"`{command}` takes one {what}, not {_positional.Count.ToString(CultureInfo.InvariantCulture)} arguments");
            }

            return _positional[0];
        }

        public string? Value(string name)
        {
            _read.Add(name);
            if (!_options.TryGetValue(name, out var value))
            {
                return null;
            }

            return value ?? throw new UsageException($"the option `--{name}` needs a value");
        }

        public bool Flag(string name)
        {
            _read.Add(name);
            if (!_options.TryGetValue(name, out var value))
            {
                return false;
            }

            return value is null ? true : throw new UsageException($"the option `--{name}` takes no value");
        }

        public long Integer(string name, long fallback, long min, long max)
        {
            var text = Value(name);
            if (text is null)
            {
                return fallback;
            }

            if (!long.TryParse(text, NumberStyles.None, CultureInfo.InvariantCulture, out var value) || value < min || value > max)
            {
                throw new UsageException($"--{name}: `{text}` is not a number from {min.ToString(CultureInfo.InvariantCulture)} to {max.ToString(CultureInfo.InvariantCulture)}");
            }

            return value;
        }

        public void Reject(params string[] names)
        {
            foreach (var name in names)
            {
                _read.Add(name);
                if (_options.ContainsKey(name))
                {
                    throw new UsageException($"the option `--{name}` does not belong to this command");
                }
            }
        }

        public void Finish()
        {
            foreach (var name in _options.Keys)
            {
                if (!_read.Contains(name))
                {
                    throw new UsageException($"unknown option `--{name}`");
                }
            }
        }
    }

    /// <summary>
    /// The command line is not usable; the run ends with <see cref="ExitUsage"/>.
    /// </summary>
    private sealed class UsageException : Exception
    {
        public UsageException(string message)
            : base(message)
        {
        }
    }
}
