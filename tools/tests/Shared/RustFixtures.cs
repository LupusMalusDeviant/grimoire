using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text.RegularExpressions;

namespace Grimoire.Tools.Tests;

/// <summary>
/// Reads the engine's golden fixtures, which the test projects copy next to their assemblies from
/// the Rust crates (see the test .csproj files): binary fixture files, hand-derived hex listings and
/// byte literals in Rust test sources. Reading the Rust sources themselves keeps the C# conformance
/// tests on the exact bytes the engine checks, without a second copy that could drift.
/// </summary>
internal static class RustFixtures
{
    private static readonly Regex ByteToken = new(@"0x[0-9A-Fa-f]{2}\b|b'(\\.|[^\\'])'", RegexOptions.CultureInvariant);

    public static string Path(params string[] parts) =>
        System.IO.Path.Combine(AppContext.BaseDirectory, "fixtures", System.IO.Path.Combine(parts));

    public static byte[] Bytes(params string[] parts) => File.ReadAllBytes(Path(parts));

    /// <summary>
    /// The bytes of <c>const NAME: &amp;[u8] = &amp;[ ... ];</c> in a Rust source: hex literals and byte
    /// character literals, with <c>//</c> comments ignored.
    /// </summary>
    public static byte[] ByteLiteral(string sourceFile, string constName)
    {
        var lines = File.ReadAllLines(Path(sourceFile));
        var start = Array.FindIndex(lines, line => line.StartsWith($"const {constName}: &[u8] = &[", StringComparison.Ordinal));
        if (start < 0)
        {
            throw new InvalidOperationException($"{constName} not found in {sourceFile}");
        }

        var bytes = new List<byte>();
        for (var i = start + 1; i < lines.Length; i++)
        {
            var line = lines[i];
            if (line.TrimStart().StartsWith("];", StringComparison.Ordinal))
            {
                return bytes.ToArray();
            }

            var comment = line.IndexOf("//", StringComparison.Ordinal);
            var code = comment >= 0 ? line.Substring(0, comment) : line;
            foreach (Match match in ByteToken.Matches(code))
            {
                var token = match.Value;
                if (token.StartsWith("0x", StringComparison.Ordinal))
                {
                    bytes.Add(byte.Parse(token.AsSpan(2), NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture));
                }
                else
                {
                    var character = token[2] == '\\' ? token[3] : token[2];
                    bytes.Add(checked((byte)character));
                }
            }
        }

        throw new InvalidOperationException($"{constName} in {sourceFile} is not terminated");
    }

    /// <summary>
    /// The bytes of a hand-derived hex listing: hex byte pairs separated by whitespace, <c>#</c> starts a
    /// comment.
    /// </summary>
    public static byte[] HexListing(params string[] parts)
    {
        var bytes = new List<byte>();
        foreach (var line in File.ReadAllLines(Path(parts)))
        {
            var comment = line.IndexOf('#', StringComparison.Ordinal);
            var code = comment >= 0 ? line.Substring(0, comment) : line;
            foreach (var token in code.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries))
            {
                bytes.Add(byte.Parse(token, NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture));
            }
        }

        return bytes.ToArray();
    }
}
