using System;
using System.Buffers.Binary;
using System.Text;

namespace Grimoire.Formats;

/// <summary>
/// The engine's <c>StableHasher</c> version 1 (<c>grimoire_core</c>, contract §4): a 64-bit FxHash-style
/// mixer with a SplitMix64 finaliser, bit-identical to the Rust implementation. Tools need it for
/// <c>AssetId::from_path</c> (contract §12) and any other id the engine derives from strings.
/// </summary>
public struct StableHasher
{
    /// <summary>
    /// Version of the hashing algorithm; ids are only comparable within one version.
    /// </summary>
    public const uint AlgorithmVersion = 1;

    private const ulong MixConstant = 0x517c_c1b7_2722_0a95;

    private ulong _state;
    private ulong _length;

    /// <summary>
    /// Creates a hasher whose output is additionally keyed by <paramref name="seed"/>; the default
    /// value of the struct equals a seed of zero, as <c>StableHasher::new</c>.
    /// </summary>
    /// <param name="seed">Seed.</param>
    public StableHasher(ulong seed)
    {
        _state = seed;
        _length = 0;
    }

    /// <summary>
    /// Feeds an unsigned 8-bit integer.
    /// </summary>
    /// <param name="value">Value.</param>
    public void WriteU8(byte value)
    {
        Mix(value);
        _length += 1;
    }

    /// <summary>
    /// Feeds an unsigned 16-bit integer.
    /// </summary>
    /// <param name="value">Value.</param>
    public void WriteU16(ushort value)
    {
        Mix(value);
        _length += 2;
    }

    /// <summary>
    /// Feeds an unsigned 32-bit integer.
    /// </summary>
    /// <param name="value">Value.</param>
    public void WriteU32(uint value)
    {
        Mix(value);
        _length += 4;
    }

    /// <summary>
    /// Feeds an unsigned 64-bit integer (also <c>usize</c>, always widened to 64 bits).
    /// </summary>
    /// <param name="value">Value.</param>
    public void WriteU64(ulong value)
    {
        Mix(value);
        _length += 8;
    }

    /// <summary>
    /// Feeds raw bytes, prefixed with their length.
    /// </summary>
    /// <param name="bytes">Bytes.</param>
    public void WriteBytes(ReadOnlySpan<byte> bytes)
    {
        WriteU64((ulong)bytes.Length);
        var words = bytes.Length / 8;
        for (var i = 0; i < words; i++)
        {
            Mix(BinaryPrimitives.ReadUInt64LittleEndian(bytes.Slice(i * 8, 8)));
        }

        var rest = bytes.Slice(words * 8);
        if (!rest.IsEmpty)
        {
            Span<byte> word = stackalloc byte[8];
            word.Clear();
            rest.CopyTo(word);
            Mix(BinaryPrimitives.ReadUInt64LittleEndian(word));
        }

        _length += (ulong)bytes.Length;
    }

    /// <summary>
    /// Feeds a string as its UTF-8 bytes, length-prefixed.
    /// </summary>
    /// <param name="value">String.</param>
    public void WriteStr(string value)
    {
        ArgumentNullException.ThrowIfNull(value);
        WriteBytes(Encoding.UTF8.GetBytes(value));
    }

    /// <summary>
    /// The hash of everything written so far; the hasher stays usable.
    /// </summary>
    /// <returns>The hash.</returns>
    public readonly ulong Finish() => SplitMix64(_state ^ _length);

    private void Mix(ulong word) => _state = unchecked((ulong.RotateLeft(_state, 5) ^ word) * MixConstant);

    private static ulong SplitMix64(ulong value)
    {
        unchecked
        {
            var z = value + 0x9e37_79b9_7f4a_7c15;
            z = (z ^ (z >> 30)) * 0xbf58_476d_1ce4_e5b9;
            z = (z ^ (z >> 27)) * 0x94d0_49bb_1331_11eb;
            return z ^ (z >> 31);
        }
    }
}
