using System;
using System.Reflection;

namespace Grimoire.Formats;

/// <summary>
/// Identity of the engine this tool build belongs to, fixed at build time.
/// </summary>
/// <remarks>
/// <see cref="EngineVersion"/> is the workspace version from the engine's <c>Cargo.toml</c>, the same
/// string as the engine's <c>ENGINE_VERSION</c>; the debug-link handshake compares it byte for byte
/// (contract §13), so tools are rebuilt for every engine tag. <see cref="BuildHash"/> is
/// <c>GRIMOIRE_BUILD_HASH</c> at build time (40 lowercase hex digits), or <c>unknown</c>, like the
/// engine's <c>ENGINE_BUILD</c> (contract §8.1).
/// </remarks>
public static class EngineBuild
{
    /// <summary>
    /// The engine version the tools were built for, e.g. <c>0.4.0</c>.
    /// </summary>
    public static string EngineVersion { get; } = Metadata("GrimoireEngineVersion");

    /// <summary>
    /// The build hash of this tool build, or <c>unknown</c>.
    /// </summary>
    public static string BuildHash { get; } = Metadata("GrimoireBuildHash");

    /// <summary>
    /// The literal the debug protocol uses for an unrecorded build hash.
    /// </summary>
    public const string UnknownBuildHash = "unknown";

    private static string Metadata(string key)
    {
        foreach (var attribute in typeof(EngineBuild).Assembly.GetCustomAttributes<AssemblyMetadataAttribute>())
        {
            if (string.Equals(attribute.Key, key, StringComparison.Ordinal) && attribute.Value is { Length: > 0 } value)
            {
                return value;
            }
        }

        throw new InvalidOperationException($"assembly metadata `{key}` is missing from Grimoire.Formats");
    }
}
