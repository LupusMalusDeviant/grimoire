using System;
using System.Globalization;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;
using Grimoire.Formats.Debug;

namespace Grimoire.LiveLink;

/// <summary>
/// Opens one byte stream to the engine per call; the client reconnects by calling it again.
/// </summary>
public interface ILinkConnector
{
    /// <summary>
    /// Opens a connection.
    /// </summary>
    /// <param name="cancellationToken">Cancels the attempt.</param>
    /// <returns>A duplex stream; the client disposes it.</returns>
    /// <exception cref="IOException">The engine is not reachable.</exception>
    /// <exception cref="SocketException">The engine is not reachable.</exception>
    ValueTask<Stream> ConnectAsync(CancellationToken cancellationToken);
}

/// <summary>
/// Connects over TCP to <c>127.0.0.1</c> only, the engine's debug-link address (contract §13).
/// </summary>
public sealed class TcpLinkConnector : ILinkConnector
{
    /// <summary>
    /// Creates a connector for <c>127.0.0.1:<paramref name="port"/></c>.
    /// </summary>
    /// <param name="port">Port, 1 to 65535.</param>
    public TcpLinkConnector(int port)
    {
        if (port is < 1 or > ushort.MaxValue)
        {
            throw new ArgumentOutOfRangeException(nameof(port));
        }

        Port = port;
    }

    /// <summary>
    /// Port on <c>127.0.0.1</c>.
    /// </summary>
    public int Port { get; }

    /// <summary>
    /// The connector for <c>GRIMOIRE_DEBUG_ADDR</c>, or for the default port if it is not set.
    /// </summary>
    /// <returns>The connector.</returns>
    /// <exception cref="FormatException">The variable is set but not <c>127.0.0.1:&lt;port&gt;</c>.</exception>
    public static TcpLinkConnector FromEnvironment()
    {
        var text = Environment.GetEnvironmentVariable(DebugProtocol.DebugAddressEnvironmentVariable);
        if (string.IsNullOrEmpty(text))
        {
            return new TcpLinkConnector(DebugProtocol.DefaultDebugPort);
        }

        return TryParseAddress(text, out var port, out var error)
            ? new TcpLinkConnector(port)
            : throw new FormatException($"{DebugProtocol.DebugAddressEnvironmentVariable}: {error}");
    }

    /// <summary>
    /// Parses <c>127.0.0.1:&lt;port&gt;</c> with a port from 1 to 65535. Any other host, including
    /// <c>localhost</c>, <c>0.0.0.0</c>, other <c>127.x</c> addresses and <c>::1</c>, is rejected, as the
    /// engine rejects them.
    /// </summary>
    /// <param name="text">Address text.</param>
    /// <param name="port">The port, if valid.</param>
    /// <param name="error">Why the text is invalid, otherwise <c>null</c>.</param>
    /// <returns><c>true</c> if valid.</returns>
    public static bool TryParseAddress(string text, out int port, out string? error)
    {
        ArgumentNullException.ThrowIfNull(text);
        port = 0;
        const string prefix = "127.0.0.1:";
        if (!text.StartsWith(prefix, StringComparison.Ordinal))
        {
            error = $"`{text}` is not 127.0.0.1:<port>; the debug link only uses the IPv4 loopback address";
            return false;
        }

        var portText = text.Substring(prefix.Length);
        if (portText.Length == 0 || portText.Length > 5 || !int.TryParse(portText, NumberStyles.None, CultureInfo.InvariantCulture, out port)
            || port is < 1 or > ushort.MaxValue)
        {
            port = 0;
            error = $"`{portText}` is not a port from 1 to 65535";
            return false;
        }

        error = null;
        return true;
    }

    /// <inheritdoc/>
    public async ValueTask<Stream> ConnectAsync(CancellationToken cancellationToken)
    {
        var socket = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp) { NoDelay = true };
        try
        {
            await socket.ConnectAsync(new IPEndPoint(IPAddress.Loopback, Port), cancellationToken).ConfigureAwait(false);
            return new NetworkStream(socket, ownsSocket: true);
        }
        catch
        {
            socket.Dispose();
            throw;
        }
    }
}

/// <summary>
/// In-process connector: every <see cref="ConnectAsync"/> creates a connected pair of streams and hands
/// the engine end to <see cref="AcceptAsync"/>. For tests, previews and editors without a running engine;
/// no socket is involved.
/// </summary>
public sealed class InMemoryLinkConnector : ILinkConnector
{
    private readonly Channel<Stream> _pending = Channel.CreateUnbounded<Stream>();
    private int _connections;

    /// <summary>
    /// When set, <see cref="ConnectAsync"/> fails as if no engine were listening.
    /// </summary>
    public bool Refuse { get; set; }

    /// <summary>
    /// Largest chunk one read returns; smaller values exercise partial frames.
    /// </summary>
    public int MaxReadChunk { get; set; } = int.MaxValue;

    /// <summary>
    /// Number of connection attempts so far, refused ones included.
    /// </summary>
    public int ConnectionAttempts => Volatile.Read(ref _connections);

    /// <inheritdoc/>
    public ValueTask<Stream> ConnectAsync(CancellationToken cancellationToken)
    {
        Interlocked.Increment(ref _connections);
        cancellationToken.ThrowIfCancellationRequested();
        if (Refuse)
        {
            throw new IOException("no engine is listening");
        }

        var (tool, engine) = InMemoryDuplexStream.CreatePair(MaxReadChunk);
        _pending.Writer.TryWrite(engine);
        return new ValueTask<Stream>(tool);
    }

    /// <summary>
    /// Waits for the engine end of the next connection.
    /// </summary>
    /// <param name="cancellationToken">Cancels the wait.</param>
    /// <returns>The engine end.</returns>
    public ValueTask<Stream> AcceptAsync(CancellationToken cancellationToken) => _pending.Reader.ReadAsync(cancellationToken);
}

/// <summary>
/// One end of an in-memory duplex byte stream. Disposing either end ends both directions: reads on the
/// other end return 0 once buffered bytes are consumed, writes throw <see cref="IOException"/>.
/// </summary>
internal sealed class InMemoryDuplexStream : Stream
{
    private readonly Channel<byte[]> _incoming;
    private readonly Channel<byte[]> _outgoing;
    private readonly int _maxReadChunk;
    private byte[] _current = Array.Empty<byte>();
    private int _currentOffset;
    private int _closed;

    private InMemoryDuplexStream(Channel<byte[]> incoming, Channel<byte[]> outgoing, int maxReadChunk)
    {
        _incoming = incoming;
        _outgoing = outgoing;
        _maxReadChunk = maxReadChunk;
    }

    public override bool CanRead => true;

    public override bool CanSeek => false;

    public override bool CanWrite => true;

    public override long Length => throw new NotSupportedException();

    public override long Position
    {
        get => throw new NotSupportedException();
        set => throw new NotSupportedException();
    }

    public static (InMemoryDuplexStream Tool, InMemoryDuplexStream Engine) CreatePair(int maxReadChunk)
    {
        var toEngine = Channel.CreateUnbounded<byte[]>();
        var toTool = Channel.CreateUnbounded<byte[]>();
        var tool = new InMemoryDuplexStream(toTool, toEngine, maxReadChunk);
        var engine = new InMemoryDuplexStream(toEngine, toTool, maxReadChunk);
        return (tool, engine);
    }

    public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
    {
        if (buffer.IsEmpty)
        {
            return 0;
        }

        while (_currentOffset == _current.Length)
        {
            if (Volatile.Read(ref _closed) != 0)
            {
                return 0;
            }

            try
            {
                if (!await _incoming.Reader.WaitToReadAsync(cancellationToken).ConfigureAwait(false))
                {
                    return 0;
                }
            }
            catch (ChannelClosedException)
            {
                return 0;
            }

            if (_incoming.Reader.TryRead(out var next))
            {
                _current = next;
                _currentOffset = 0;
            }
        }

        var count = Math.Min(Math.Min(buffer.Length, _current.Length - _currentOffset), _maxReadChunk);
        _current.AsMemory(_currentOffset, count).CopyTo(buffer);
        _currentOffset += count;
        return count;
    }

    public override int Read(byte[] buffer, int offset, int count) =>
        ReadAsync(buffer.AsMemory(offset, count)).AsTask().GetAwaiter().GetResult();

    public override ValueTask WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken cancellationToken = default)
    {
        cancellationToken.ThrowIfCancellationRequested();
        Write(buffer.Span);
        return ValueTask.CompletedTask;
    }

    public override void Write(ReadOnlySpan<byte> buffer)
    {
        if (Volatile.Read(ref _closed) != 0 || !_outgoing.Writer.TryWrite(buffer.ToArray()))
        {
            throw new IOException("the in-memory link is closed");
        }
    }

    public override void Write(byte[] buffer, int offset, int count) => Write(buffer.AsSpan(offset, count));

    public override Task FlushAsync(CancellationToken cancellationToken) => Task.CompletedTask;

    public override void Flush()
    {
    }

    public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();

    public override void SetLength(long value) => throw new NotSupportedException();

    protected override void Dispose(bool disposing)
    {
        if (Interlocked.Exchange(ref _closed, 1) == 0)
        {
            _outgoing.Writer.TryComplete();
            _incoming.Writer.TryComplete();
        }

        base.Dispose(disposing);
    }
}
