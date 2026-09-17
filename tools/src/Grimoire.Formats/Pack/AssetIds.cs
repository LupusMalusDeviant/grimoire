using System;

namespace Grimoire.Formats.Pack;

/// <summary>
/// A validated asset path (contract §12): ASCII <c>[a-z0-9_.-]</c> segments separated by <c>/</c>,
/// 1 to 255 bytes, no leading or trailing <c>/</c>, no empty, <c>.</c> or <c>..</c> segment.
/// </summary>
public sealed class AssetPath : IEquatable<AssetPath>
{
    /// <summary>
    /// Longest path in bytes.
    /// </summary>
    public const int MaxLength = 255;

    private AssetPath(string value)
    {
        Value = value;
    }

    /// <summary>
    /// The path string.
    /// </summary>
    public string Value { get; }

    /// <summary>
    /// Validates <paramref name="path"/>.
    /// </summary>
    /// <param name="path">Candidate path.</param>
    /// <param name="assetPath">The path, if valid.</param>
    /// <param name="reason">Why it is invalid, otherwise <c>null</c>.</param>
    /// <returns><c>true</c> if valid.</returns>
    public static bool TryCreate(string path, out AssetPath? assetPath, out string? reason)
    {
        ArgumentNullException.ThrowIfNull(path);
        assetPath = null;
        reason = Validate(path);
        if (reason is not null)
        {
            return false;
        }

        assetPath = new AssetPath(path);
        return true;
    }

    /// <summary>
    /// Validates <paramref name="path"/>.
    /// </summary>
    /// <param name="path">Candidate path.</param>
    /// <returns>The path.</returns>
    /// <exception cref="ArgumentException">The path is invalid.</exception>
    public static AssetPath Create(string path) =>
        TryCreate(path, out var assetPath, out var reason)
            ? assetPath!
            : throw new ArgumentException($"invalid asset path `{path}`: {reason}", nameof(path));

    /// <inheritdoc/>
    public bool Equals(AssetPath? other) => other is not null && string.Equals(Value, other.Value, StringComparison.Ordinal);

    /// <inheritdoc/>
    public override bool Equals(object? obj) => Equals(obj as AssetPath);

    /// <inheritdoc/>
    public override int GetHashCode() => StringComparer.Ordinal.GetHashCode(Value);

    /// <inheritdoc/>
    public override string ToString() => Value;

    private static string? Validate(string path)
    {
        if (path.Length == 0)
        {
            return "path is empty";
        }

        if (path.Length > MaxLength)
        {
            return "path exceeds 255 bytes";
        }

        if (path[0] == '/' || path[^1] == '/')
        {
            return "path has a leading or trailing '/'";
        }

        foreach (var segment in path.Split('/'))
        {
            if (segment.Length == 0)
            {
                return "path has an empty segment";
            }

            if (segment is "." or "..")
            {
                return "path has a '.' or '..' segment";
            }
        }

        foreach (var c in path)
        {
            var allowed = c is >= 'a' and <= 'z' or >= '0' and <= '9' or '_' or '.' or '-' or '/';
            if (!allowed)
            {
                return "path contains a byte outside ASCII [a-z0-9_.-] and '/'";
            }
        }

        return null;
    }
}

/// <summary>
/// Stable asset id (contract §12): the <c>StableHasher</c> v1 hash of <c>grimoire.asset-id.v1</c> and the
/// path. The engine's <c>UnitId</c> of a Sigil unit follows the same rule.
/// </summary>
/// <param name="Value">The 64-bit id.</param>
public readonly record struct AssetId(ulong Value) : IComparable<AssetId>
{
    /// <summary>
    /// Domain string hashed before the path.
    /// </summary>
    public const string Domain = "grimoire.asset-id.v1";

    /// <summary>
    /// The id of <paramref name="path"/>.
    /// </summary>
    /// <param name="path">Asset path.</param>
    /// <returns>The id.</returns>
    public static AssetId FromPath(AssetPath path)
    {
        ArgumentNullException.ThrowIfNull(path);
        var hasher = new StableHasher(0);
        hasher.WriteStr(Domain);
        hasher.WriteStr(path.Value);
        return new AssetId(hasher.Finish());
    }

    /// <inheritdoc/>
    public int CompareTo(AssetId other) => Value.CompareTo(other.Value);

    /// <summary>
    /// Sixteen lowercase hex digits, the engine's JSON convention for <c>u64</c> ids.
    /// </summary>
    /// <returns>The hex string.</returns>
    public override string ToString() => Value.ToString("x16", System.Globalization.CultureInfo.InvariantCulture);
}

/// <summary>
/// Kind tag of a pack entry (contract §12).
/// </summary>
/// <param name="Value">The raw kind.</param>
public readonly record struct AssetKind(ushort Value)
{
    /// <summary>
    /// A compiled Sigil unit; the kind version is the <c>SigilUnit</c> format version.
    /// </summary>
    public static readonly AssetKind Sigil = new(1);

    /// <summary>
    /// First application-defined kind; <c>0x8000</c> to <c>0xFFFF</c> pass through untouched.
    /// </summary>
    public const ushort FirstApplicationKind = 0x8000;
}
