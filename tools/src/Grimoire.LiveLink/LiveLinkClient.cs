using System;
using System.Collections.Concurrent;
using System.Globalization;
using System.IO;
using System.Net.Sockets;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;
using Grimoire.Formats;
using Grimoire.Formats.Debug;

namespace Grimoire.LiveLink;

/// <summary>
/// State of a <see cref="LiveLinkClient"/>.
/// </summary>
public enum LinkState
{
    /// <summary>
    /// Not connected; waiting before the next attempt, or not started. Tools keep working on files.
    /// </summary>
    Disconnected,

    /// <summary>
    /// Connecting or waiting for the engine's handshake reply.
    /// </summary>
    Connecting,

    /// <summary>
    /// Handshake completed; messages flow.
    /// </summary>
    Connected,

    /// <summary>
    /// The engine refused the handshake for a reason a retry cannot fix (version, token, a malformed or
    /// oversized <c>Hello</c>); no further attempts until <see cref="LiveLinkClient.Start"/> is called again.
    /// </summary>
    Rejected,

    /// <summary>
    /// Stopped by <see cref="LiveLinkClient.StopAsync"/> or disposal.
    /// </summary>
    Stopped,
}

/// <summary>
/// Arguments of <see cref="LiveLinkClient.StateChanged"/>.
/// </summary>
public sealed class LinkStateChangedEventArgs : EventArgs
{
    /// <summary>
    /// Creates the arguments.
    /// </summary>
    /// <param name="state">New state.</param>
    /// <param name="reason">Why the state changed, if known.</param>
    public LinkStateChangedEventArgs(LinkState state, string? reason)
    {
        State = state;
        Reason = reason;
    }

    /// <summary>
    /// New state.
    /// </summary>
    public LinkState State { get; }

    /// <summary>
    /// Why the state changed, if known.
    /// </summary>
    public string? Reason { get; }
}

/// <summary>
/// Settings of a <see cref="LiveLinkClient"/>.
/// </summary>
public sealed class LiveLinkOptions
{
    /// <summary>
    /// Creates settings for <paramref name="identity"/>.
    /// </summary>
    /// <param name="identity">What the tool presents in its <c>Hello</c>.</param>
    public LiveLinkOptions(ToolIdentity identity)
    {
        ArgumentNullException.ThrowIfNull(identity);
        Identity = identity;
    }

    /// <summary>
    /// What the tool presents in its <c>Hello</c>.
    /// </summary>
    public ToolIdentity Identity { get; }

    /// <summary>
    /// Longest wait for a connection to open.
    /// </summary>
    public TimeSpan ConnectTimeout { get; init; } = TimeSpan.FromSeconds(2);

    /// <summary>
    /// Longest wait for the engine's handshake reply.
    /// </summary>
    public TimeSpan HandshakeTimeout { get; init; } = DebugProtocol.HandshakeTimeout;

    /// <summary>
    /// Wait before the first reconnect; doubles per failed attempt up to <see cref="MaxReconnectDelay"/>.
    /// </summary>
    public TimeSpan InitialReconnectDelay { get; init; } = TimeSpan.FromMilliseconds(250);

    /// <summary>
    /// Longest wait between reconnect attempts.
    /// </summary>
    public TimeSpan MaxReconnectDelay { get; init; } = TimeSpan.FromSeconds(5);

    /// <summary>
    /// Received messages kept for <see cref="LiveLinkClient.Messages"/>; the oldest are dropped beyond it.
    /// </summary>
    public int MessageBufferCapacity { get; init; } = 1024;

    /// <summary>
    /// Parses a debug-link token: exactly 64 hex digits.
    /// </summary>
    /// <param name="hex">Token text.</param>
    /// <returns>The 32 token bytes.</returns>
    /// <exception cref="FormatException">The text is not 64 hex digits.</exception>
    public static byte[] ParseToken(string hex)
    {
        ArgumentNullException.ThrowIfNull(hex);
        if (hex.Length != 64)
        {
            throw new FormatException("the debug-link token has exactly 64 hex digits");
        }

        var token = new byte[32];
        for (var i = 0; i < 32; i++)
        {
            if (!byte.TryParse(hex.AsSpan(i * 2, 2), NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture, out token[i]))
            {
                throw new FormatException("the debug-link token has exactly 64 hex digits");
            }
        }

        return token;
    }

    /// <summary>
    /// Settings with the token from <c>GRIMOIRE_DEBUG_TOKEN</c> and this build's identity, or <c>null</c> if
    /// the variable is not set (the engine never binds without a token either).
    /// </summary>
    /// <param name="statsIntervalFrames"><c>Stats</c> cadence to request.</param>
    /// <returns>The settings, or <c>null</c>.</returns>
    /// <exception cref="FormatException">The variable is set but not 64 hex digits.</exception>
    public static LiveLinkOptions? FromEnvironment(ushort statsIntervalFrames = 0)
    {
        var text = Environment.GetEnvironmentVariable(DebugProtocol.DebugTokenEnvironmentVariable);
        return string.IsNullOrEmpty(text)
            ? null
            : new LiveLinkOptions(new ToolIdentity(ParseToken(text), statsIntervalFrames: statsIntervalFrames));
    }
}

/// <summary>
/// How a message send went.
/// </summary>
/// <param name="Sent">Whether the frame was written to the connection.</param>
/// <param name="Seq">Its sequence number, or <c>0</c> if not sent.</param>
public readonly record struct SendResult(bool Sent, uint Seq);

/// <summary>
/// Outcome of <see cref="LiveLinkClient.SwapSigilUnitAsync"/>.
/// </summary>
public enum SwapStatus
{
    /// <summary>
    /// The engine applied the unit (<c>SwapAck</c> status 0).
    /// </summary>
    Applied,

    /// <summary>
    /// The engine rejected the unit (<c>SwapAck</c> status 1); see the reason.
    /// </summary>
    Rejected,

    /// <summary>
    /// A later swap of the same unit replaced this one before a tick boundary (<c>SwapAck</c> status 2).
    /// </summary>
    Superseded,

    /// <summary>
    /// The engine answered with an <c>Error</c>.
    /// </summary>
    EngineError,

    /// <summary>
    /// No link: the swap was not sent. The tool's file stays the source of truth (degraded to file work).
    /// </summary>
    NotConnected,

    /// <summary>
    /// The link dropped before the engine answered.
    /// </summary>
    ConnectionLost,

    /// <summary>
    /// The engine did not answer in time.
    /// </summary>
    TimedOut,
}

/// <summary>
/// Result of a unit swap.
/// </summary>
/// <param name="Status">Outcome.</param>
/// <param name="Ack">The engine's acknowledgement, if any.</param>
/// <param name="Error">The engine's error, if any.</param>
public sealed record SwapResult(SwapStatus Status, SwapAck? Ack = null, ErrorMsg? Error = null);

/// <summary>
/// Live-link client of the debug protocol v1 (PRD-0016 FR-03, contract §13): connects, runs the tool
/// side of the handshake, reconnects with backoff when the link drops, sends and receives typed
/// messages, and never throws because the engine is gone (PRD-0016: a link loss degrades to file
/// work).
/// </summary>
public sealed class LiveLinkClient : IAsyncDisposable
{
    private readonly ILinkConnector _connector;
    private readonly LiveLinkOptions _options;
    private readonly Channel<Message> _messages;
    private readonly ConcurrentDictionary<uint, TaskCompletionSource<SwapResult>> _pendingSwaps = new();
    private readonly SemaphoreSlim _sendLock = new(1, 1);
    private readonly object _gate = new();
    private CancellationTokenSource? _runCancellation;
    private Task? _runTask;
    private Stream? _stream;
    private uint _nextSeq;
    private int _state = (int)LinkState.Disconnected;

    /// <summary>
    /// Creates a client; call <see cref="Start"/> to connect.
    /// </summary>
    /// <param name="connector">Opens connections to the engine.</param>
    /// <param name="options">Settings.</param>
    public LiveLinkClient(ILinkConnector connector, LiveLinkOptions options)
    {
        ArgumentNullException.ThrowIfNull(connector);
        ArgumentNullException.ThrowIfNull(options);
        _connector = connector;
        _options = options;
        _messages = Channel.CreateBounded<Message>(new BoundedChannelOptions(Math.Max(1, options.MessageBufferCapacity))
        {
            FullMode = BoundedChannelFullMode.DropOldest,
            SingleWriter = true,
        });
    }

    /// <summary>
    /// Raised on every state change, on the client's background task.
    /// </summary>
    public event EventHandler<LinkStateChangedEventArgs>? StateChanged;

    /// <summary>
    /// Current state.
    /// </summary>
    public LinkState State => (LinkState)Volatile.Read(ref _state);

    /// <summary>
    /// The engine's <c>Hello</c> of the current or last connection.
    /// </summary>
    public Hello? EngineHello { get; private set; }

    /// <summary>
    /// The engine's <c>Error</c> that put the client into <see cref="LinkState.Rejected"/>, if any.
    /// </summary>
    public ErrorMsg? Rejection { get; private set; }

    /// <summary>
    /// Messages the engine sent after the handshake (<c>Stats</c>, <c>Log</c>, <c>SwapAck</c>, <c>Error</c>),
    /// in arrival order.
    /// </summary>
    public ChannelReader<Message> Messages => _messages.Reader;

    /// <summary>
    /// Starts connecting in the background; restarts after <see cref="LinkState.Rejected"/>. Does nothing
    /// while already running.
    /// </summary>
    /// <exception cref="ObjectDisposedException">The client was stopped.</exception>
    public void Start()
    {
        lock (_gate)
        {
            if (State == LinkState.Stopped)
            {
                throw new ObjectDisposedException(nameof(LiveLinkClient));
            }

            if (_runTask is { IsCompleted: false })
            {
                return;
            }

            _runCancellation?.Dispose();
            _runCancellation = new CancellationTokenSource();
            Rejection = null;
            _runTask = Task.Run(() => RunAsync(_runCancellation.Token));
        }
    }

    /// <summary>
    /// Waits until <see cref="State"/> is <paramref name="state"/>.
    /// </summary>
    /// <param name="state">State to wait for.</param>
    /// <param name="cancellationToken">Cancels the wait.</param>
    /// <returns>A task that completes when the state is reached.</returns>
    public async Task WaitForStateAsync(LinkState state, CancellationToken cancellationToken)
    {
        var reached = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        void Handler(object? sender, LinkStateChangedEventArgs args)
        {
            if (args.State == state)
            {
                reached.TrySetResult();
            }
        }

        StateChanged += Handler;
        try
        {
            if (State == state)
            {
                return;
            }

            await reached.Task.WaitAsync(cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            StateChanged -= Handler;
        }
    }

    /// <summary>
    /// Sends <paramref name="message"/> if connected. Never throws because the link is down.
    /// </summary>
    /// <param name="message">Message.</param>
    /// <param name="cancellationToken">Cancels the send.</param>
    /// <returns>Whether it was sent, and its sequence number.</returns>
    /// <exception cref="WireFormatException">The message does not encode.</exception>
    public async Task<SendResult> SendAsync(Message message, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(message);
        var payload = message.EncodePayload();
        await _sendLock.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var stream = _stream;
            if (stream is null || State != LinkState.Connected)
            {
                return new SendResult(false, 0);
            }

            var seq = ++_nextSeq;
            var bytes = new Frame(message.Id, seq, payload).Encode();
            try
            {
                await stream.WriteAsync(bytes, cancellationToken).ConfigureAwait(false);
                await stream.FlushAsync(cancellationToken).ConfigureAwait(false);
                return new SendResult(true, seq);
            }
            catch (Exception error) when (IsLinkFailure(error))
            {
                CloseStream(stream);
                return new SendResult(false, 0);
            }
        }
        finally
        {
            _sendLock.Release();
        }
    }

    /// <summary>
    /// Asks the engine to hot-swap a Sigil unit and waits for its acknowledgement. Without a link it returns
    /// <see cref="SwapStatus.NotConnected"/> at once: the caller keeps working on the file.
    /// </summary>
    /// <param name="unitPath">Asset path of the unit.</param>
    /// <param name="unitBytes">Compiled unit bytes.</param>
    /// <param name="timeout">Longest wait for the acknowledgement.</param>
    /// <param name="cancellationToken">Cancels the wait.</param>
    /// <returns>The result.</returns>
    /// <exception cref="WireFormatException">The path or unit exceeds its maximum.</exception>
    public async Task<SwapResult> SwapSigilUnitAsync(string unitPath, byte[] unitBytes, TimeSpan timeout, CancellationToken cancellationToken = default)
    {
        var message = Message.Of(new SwapSigilUnit { UnitPath = unitPath, UnitBytes = unitBytes });
        message.EncodePayload();
        if (State != LinkState.Connected)
        {
            return new SwapResult(SwapStatus.NotConnected);
        }

        var completion = new TaskCompletionSource<SwapResult>(TaskCreationOptions.RunContinuationsAsynchronously);
        uint seq;
        await _sendLock.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var stream = _stream;
            if (stream is null || State != LinkState.Connected)
            {
                return new SwapResult(SwapStatus.NotConnected);
            }

            seq = ++_nextSeq;
            _pendingSwaps[seq] = completion;
            try
            {
                await stream.WriteAsync(message.ToFrame(seq).Encode(), cancellationToken).ConfigureAwait(false);
                await stream.FlushAsync(cancellationToken).ConfigureAwait(false);
            }
            catch (Exception error) when (IsLinkFailure(error))
            {
                _pendingSwaps.TryRemove(seq, out _);
                CloseStream(stream);
                return new SwapResult(SwapStatus.NotConnected);
            }
        }
        finally
        {
            _sendLock.Release();
        }

        try
        {
            return await completion.Task.WaitAsync(timeout, cancellationToken).ConfigureAwait(false);
        }
        catch (TimeoutException)
        {
            return new SwapResult(SwapStatus.TimedOut);
        }
        finally
        {
            _pendingSwaps.TryRemove(seq, out _);
        }
    }

    /// <summary>
    /// Stops connecting and closes the link. The client cannot be started again.
    /// </summary>
    /// <returns>A task that completes when the background task has ended.</returns>
    public async Task StopAsync()
    {
        Task? run;
        lock (_gate)
        {
            if (State == LinkState.Stopped)
            {
                return;
            }

            _runCancellation?.Cancel();
            run = _runTask;
        }

        if (_stream is { } stream)
        {
            CloseStream(stream);
        }

        if (run is not null)
        {
            try
            {
                await run.ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
            }
        }

        SetState(LinkState.Stopped, "stopped");
        _messages.Writer.TryComplete();
    }

    /// <inheritdoc/>
    public async ValueTask DisposeAsync()
    {
        await StopAsync().ConfigureAwait(false);
        _runCancellation?.Dispose();
        _sendLock.Dispose();
    }

    private async Task RunAsync(CancellationToken cancellationToken)
    {
        var delay = _options.InitialReconnectDelay;
        while (!cancellationToken.IsCancellationRequested)
        {
            var outcome = await ConnectOnceAsync(cancellationToken).ConfigureAwait(false);
            switch (outcome)
            {
                case Outcome.Rejected:
                    return;
                case Outcome.WasConnected:
                    delay = _options.InitialReconnectDelay;
                    break;
            }

            if (cancellationToken.IsCancellationRequested)
            {
                return;
            }

            try
            {
                await Task.Delay(delay, cancellationToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                return;
            }

            var doubled = delay + delay;
            delay = doubled < _options.MaxReconnectDelay ? doubled : _options.MaxReconnectDelay;
        }
    }

    private async Task<Outcome> ConnectOnceAsync(CancellationToken cancellationToken)
    {
        SetState(LinkState.Connecting, null);
        Stream stream;
        try
        {
            using var connectTimeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            connectTimeout.CancelAfter(_options.ConnectTimeout);
            stream = await _connector.ConnectAsync(connectTimeout.Token).ConfigureAwait(false);
        }
        catch (Exception error) when (IsLinkFailure(error) || error is OperationCanceledException)
        {
            SetState(LinkState.Disconnected, $"connect failed: {error.Message}");
            return Outcome.Failed;
        }

        var decoder = new FrameDecoder();
        var buffer = new byte[64 * 1024];
        try
        {
            var reply = await HandshakeAsync(stream, decoder, buffer, cancellationToken).ConfigureAwait(false);
            if (reply is null)
            {
                return Outcome.Failed;
            }

            switch (reply.Outcome)
            {
                case HandshakeOutcome.Accepted:
                    break;
                case HandshakeOutcome.Rejected when reply.Error!.Code is ErrorCode.Busy or ErrorCode.HandshakeRequired:
                    SetState(LinkState.Disconnected, $"engine refused the connection for now: {reply.Error.Code}");
                    return Outcome.Failed;
                case HandshakeOutcome.Rejected:
                    Rejection = reply.Error;
                    SetState(LinkState.Rejected, $"engine rejected the handshake: {reply.Error!.Code}: {reply.Error.Message}");
                    return Outcome.Rejected;
                default:
                    SetState(LinkState.Rejected, "the engine's handshake reply is malformed");
                    return Outcome.Rejected;
            }

            await _sendLock.WaitAsync(cancellationToken).ConfigureAwait(false);
            try
            {
                EngineHello = reply.EngineHello;
                _nextSeq = ToolHandshake.HelloSeq;
                _stream = stream;
                SetState(LinkState.Connected, null);
            }
            finally
            {
                _sendLock.Release();
            }

            var reason = await ReadLoopAsync(stream, decoder, buffer, cancellationToken).ConfigureAwait(false);
            SetState(LinkState.Disconnected, reason);
            return Outcome.WasConnected;
        }
        catch (OperationCanceledException)
        {
            return Outcome.Failed;
        }
        finally
        {
            CloseStream(stream);
            FailPendingSwaps();
        }
    }

    /// <summary>
    /// Sends the tool's <c>Hello</c> and reads the engine's first frame; <c>null</c> (state already set to
    /// <see cref="LinkState.Disconnected"/>) if the link failed or timed out.
    /// </summary>
    private async Task<HandshakeReply?> HandshakeAsync(Stream stream, FrameDecoder decoder, byte[] buffer, CancellationToken cancellationToken)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(_options.HandshakeTimeout);
        try
        {
            await stream.WriteAsync(ToolHandshake.HelloFrame(_options.Identity).Encode(), timeout.Token).ConfigureAwait(false);
            await stream.FlushAsync(timeout.Token).ConfigureAwait(false);
            while (true)
            {
                if (decoder.TryReadFrame(out var frame))
                {
                    return ToolHandshake.Interpret(frame!);
                }

                var read = await stream.ReadAsync(buffer, timeout.Token).ConfigureAwait(false);
                if (read == 0)
                {
                    SetState(LinkState.Disconnected, "the engine closed the connection during the handshake");
                    return null;
                }

                decoder.Push(buffer.AsSpan(0, read));
            }
        }
        catch (WireFormatException error)
        {
            SetState(LinkState.Disconnected, $"invalid frame during the handshake: {error.Message}");
            return null;
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            SetState(LinkState.Disconnected, "the engine did not answer the handshake in time");
            return null;
        }
        catch (Exception error) when (IsLinkFailure(error))
        {
            SetState(LinkState.Disconnected, $"link failed during the handshake: {error.Message}");
            return null;
        }
    }

    /// <summary>
    /// Dispatches frames until the link ends; returns why it ended.
    /// </summary>
    private async Task<string> ReadLoopAsync(Stream stream, FrameDecoder decoder, byte[] buffer, CancellationToken cancellationToken)
    {
        try
        {
            while (true)
            {
                while (decoder.TryReadFrame(out var frame))
                {
                    Dispatch(frame!);
                }

                var read = await stream.ReadAsync(buffer, cancellationToken).ConfigureAwait(false);
                if (read == 0)
                {
                    return "the engine closed the connection";
                }

                decoder.Push(buffer.AsSpan(0, read));
            }
        }
        catch (WireFormatException error) when (error.Kind is WireErrorKind.FrameTooShort or WireErrorKind.FrameTooLarge)
        {
            return $"the engine sent an invalid frame length: {error.Message}";
        }
        catch (Exception error) when (IsLinkFailure(error))
        {
            return $"link failed: {error.Message}";
        }
    }

    private void Dispatch(Frame frame)
    {
        Message message;
        try
        {
            message = Message.FromFrame(frame);
        }
        catch (WireFormatException)
        {
            // Unknown ids and undecodable payloads are skipped; the stream stays in sync because the frame
            // length was valid (contract §13), and a tool does not answer the engine with errors.
            return;
        }

        switch (message)
        {
            case Message<SwapAck> ack when _pendingSwaps.TryRemove(ack.Payload.InReplyTo, out var swap):
                swap.TrySetResult(new SwapResult(
                    ack.Payload.Status switch
                    {
                        0 => SwapStatus.Applied,
                        2 => SwapStatus.Superseded,
                        _ => SwapStatus.Rejected,
                    },
                    Ack: ack.Payload));
                break;
            case Message<ErrorMsg> error when _pendingSwaps.TryRemove(error.Payload.InReplyTo, out var swap):
                swap.TrySetResult(new SwapResult(SwapStatus.EngineError, Error: error.Payload));
                break;
        }

        _messages.Writer.TryWrite(message);
    }

    private void FailPendingSwaps()
    {
        foreach (var seq in _pendingSwaps.Keys)
        {
            if (_pendingSwaps.TryRemove(seq, out var swap))
            {
                swap.TrySetResult(new SwapResult(SwapStatus.ConnectionLost));
            }
        }
    }

    private void CloseStream(Stream stream)
    {
        Interlocked.CompareExchange(ref _stream, null, stream);
        try
        {
            stream.Dispose();
        }
        catch (Exception error) when (IsLinkFailure(error))
        {
        }
    }

    private void SetState(LinkState state, string? reason)
    {
        var previous = (LinkState)Interlocked.Exchange(ref _state, (int)state);
        if (previous == LinkState.Stopped && state != LinkState.Stopped)
        {
            Volatile.Write(ref _state, (int)LinkState.Stopped);
            return;
        }

        if (previous != state || reason is not null)
        {
            StateChanged?.Invoke(this, new LinkStateChangedEventArgs(state, reason));
        }
    }

    private static bool IsLinkFailure(Exception error) =>
        error is IOException or SocketException or ObjectDisposedException or InvalidOperationException;

    private enum Outcome
    {
        Failed,
        WasConnected,
        Rejected,
    }
}
