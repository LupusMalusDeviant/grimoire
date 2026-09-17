using System;

namespace Grimoire.Formats.Debug;

/// <summary>
/// Constants of the Grimoire debug protocol v1 (contract §13) shared by every tool.
/// </summary>
public static class DebugProtocol
{
    /// <summary>
    /// Protocol version this build speaks.
    /// </summary>
    public const ushort ProtocolVersion = 1;

    /// <summary>
    /// Largest value of a frame's length field, including the 8 header bytes after it.
    /// </summary>
    public const uint MaxFrameLength = 16 * 1024 * 1024;

    /// <summary>
    /// Largest <c>SigilUnit</c> a swap or preview may carry.
    /// </summary>
    public const uint MaxUnitBytes = 8 * 1024 * 1024;

    /// <summary>
    /// Largest length field of the first frame of a connection, frozen for every protocol version.
    /// </summary>
    public const uint MaxHelloFrameLength = 1024;

    /// <summary>
    /// Bytes of the frame header counted by the length field: <c>id</c>, <c>flags</c>, <c>seq</c>.
    /// </summary>
    public const int HeaderLength = 8;

    /// <summary>
    /// Bytes of the length field itself.
    /// </summary>
    public const int LengthPrefixLength = 4;

    /// <summary>
    /// Time the engine waits for a tool's complete first frame.
    /// </summary>
    public static readonly TimeSpan HandshakeTimeout = TimeSpan.FromSeconds(5);

    /// <summary>
    /// Environment variable with the engine's debug address, <c>127.0.0.1:&lt;port&gt;</c>.
    /// </summary>
    public const string DebugAddressEnvironmentVariable = "GRIMOIRE_DEBUG_ADDR";

    /// <summary>
    /// Environment variable with the debug-link token, 64 hex digits.
    /// </summary>
    public const string DebugTokenEnvironmentVariable = "GRIMOIRE_DEBUG_TOKEN";

    /// <summary>
    /// Environment variable that enables tests with real sockets; only <c>1</c> enables them.
    /// </summary>
    public const string SocketTestsEnvironmentVariable = "GRIMOIRE_SOCKET_TESTS";

    /// <summary>
    /// Default debug-link port.
    /// </summary>
    public const ushort DefaultDebugPort = 47_474;

    /// <summary>
    /// Whether <c>GRIMOIRE_SOCKET_TESTS</c> is exactly <c>1</c>, as the engine's
    /// <c>socket_tests_enabled</c>.
    /// </summary>
    /// <returns><c>true</c> if socket tests may run.</returns>
    public static bool SocketTestsEnabled() =>
        string.Equals(Environment.GetEnvironmentVariable(SocketTestsEnvironmentVariable), "1", StringComparison.Ordinal);
}
