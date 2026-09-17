using System;
using System.Threading;
using System.Threading.Tasks;

namespace Grimoire.AssetCompiler;

/// <summary>
/// The <c>grimoire-ac</c> entry point: it hands the arguments and the standard streams to
/// <see cref="Cli.RunAsync"/> and turns Ctrl+C into cancellation, so <c>watch</c> stops the link and the
/// watcher instead of being killed.
/// </summary>
public static class Program
{
    /// <summary>
    /// Runs the command line.
    /// </summary>
    /// <param name="args">The arguments.</param>
    /// <returns>The exit code (sigil.md §13.1: 0, 1 or 2).</returns>
    public static async Task<int> Main(string[] args)
    {
        using var cancellation = new CancellationTokenSource();
        ConsoleCancelEventHandler handler = (_, e) =>
        {
            e.Cancel = true;
            cancellation.Cancel();
        };
        Console.CancelKeyPress += handler;
        try
        {
            return await Cli.RunAsync(args, Console.Out, Console.Error, cancellation.Token).ConfigureAwait(false);
        }
        finally
        {
            Console.CancelKeyPress -= handler;
            await Console.Out.FlushAsync().ConfigureAwait(false);
            await Console.Error.FlushAsync().ConfigureAwait(false);
        }
    }
}
