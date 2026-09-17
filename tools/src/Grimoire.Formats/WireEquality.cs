using System;
using System.Collections.Generic;

namespace Grimoire.Formats;

/// <summary>
/// Value equality helpers for the generated types: arrays and lists compare element by element, and
/// floats compare bit for bit, like the bytes they encode to.
/// </summary>
public static class WireEquality
{
    /// <summary>
    /// Whether two floats have the same bit pattern.
    /// </summary>
    /// <param name="left">First value.</param>
    /// <param name="right">Second value.</param>
    /// <returns><c>true</c> if equal.</returns>
    public static bool Float(float left, float right) =>
        BitConverter.SingleToInt32Bits(left) == BitConverter.SingleToInt32Bits(right);

    /// <summary>
    /// Whether two optional floats are both absent or have the same bit pattern.
    /// </summary>
    /// <param name="left">First value.</param>
    /// <param name="right">Second value.</param>
    /// <returns><c>true</c> if equal.</returns>
    public static bool OptionalFloat(float? left, float? right) =>
        left is null ? right is null : right is not null && Float(left.Value, right.Value);

    /// <summary>
    /// Whether two byte arrays are both <c>null</c> or have the same contents.
    /// </summary>
    /// <param name="left">First array.</param>
    /// <param name="right">Second array.</param>
    /// <returns><c>true</c> if equal.</returns>
    public static bool Bytes(byte[]? left, byte[]? right) =>
        left is null ? right is null : right is not null && left.AsSpan().SequenceEqual(right);

    /// <summary>
    /// Whether two float arrays are both <c>null</c> or have the same bit patterns.
    /// </summary>
    /// <param name="left">First array.</param>
    /// <param name="right">Second array.</param>
    /// <returns><c>true</c> if equal.</returns>
    public static bool Floats(float[]? left, float[]? right)
    {
        if (left is null || right is null)
        {
            return left is null && right is null;
        }

        if (left.Length != right.Length)
        {
            return false;
        }

        for (var i = 0; i < left.Length; i++)
        {
            if (!Float(left[i], right[i]))
            {
                return false;
            }
        }

        return true;
    }

    /// <summary>
    /// Whether two lists are both <c>null</c> or have equal elements in the same order.
    /// </summary>
    /// <typeparam name="T">Element type.</typeparam>
    /// <param name="left">First list.</param>
    /// <param name="right">Second list.</param>
    /// <returns><c>true</c> if equal.</returns>
    public static bool Lists<T>(List<T>? left, List<T>? right)
    {
        if (left is null || right is null)
        {
            return left is null && right is null;
        }

        if (left.Count != right.Count)
        {
            return false;
        }

        var comparer = EqualityComparer<T>.Default;
        for (var i = 0; i < left.Count; i++)
        {
            if (!comparer.Equals(left[i], right[i]))
            {
                return false;
            }
        }

        return true;
    }
}
