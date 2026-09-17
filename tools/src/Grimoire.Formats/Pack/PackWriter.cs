using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.Security.Cryptography;

namespace Grimoire.Formats.Pack;

/// <summary>
/// Writes a pack v1 byte for byte like <c>grimoire_assets::PackWriter</c> (contract §12): entries sorted
/// by id, payloads aligned to 16 bytes with zero padding between them, the manifest right after the last
/// payload, no timestamp. Identical input gives identical bytes regardless of the order of
/// <see cref="Add"/> calls.
/// </summary>
public sealed class PackWriter
{
    private readonly string _compiler;
    private readonly string _compilerVersion;
    private readonly List<WriterEntry> _entries = new();
    private byte[] _application = Array.Empty<byte>();

    /// <summary>
    /// Creates a writer.
    /// </summary>
    /// <param name="compiler">Name of the writing tool, at most 64 UTF-8 bytes.</param>
    /// <param name="compilerVersion">Version of that tool, at most 64 UTF-8 bytes.</param>
    public PackWriter(string compiler, string compilerVersion)
    {
        ArgumentNullException.ThrowIfNull(compiler);
        ArgumentNullException.ThrowIfNull(compilerVersion);
        _compiler = compiler;
        _compilerVersion = compilerVersion;
    }

    /// <summary>
    /// Adds an entry.
    /// </summary>
    /// <param name="path">Asset path.</param>
    /// <param name="kind">Kind; 1 or 0x8000–0xFFFF.</param>
    /// <param name="kindVersion">Kind version.</param>
    /// <param name="bytes">Payload.</param>
    /// <returns>The entry's id.</returns>
    /// <exception cref="PackFormatException">Too many entries, a duplicate id, an invalid kind or a payload that is too large.</exception>
    public AssetId Add(AssetPath path, AssetKind kind, uint kindVersion, ReadOnlySpan<byte> bytes)
    {
        ArgumentNullException.ThrowIfNull(path);
        if (_entries.Count >= PackFormat.MaxEntries)
        {
            throw new PackFormatException(PackErrorKind.TooManyEntries, $"{_entries.Count + 1} entries, at most {PackFormat.MaxEntries} allowed");
        }

        var id = AssetId.FromPath(path);
        foreach (var entry in _entries)
        {
            if (entry.Id == id)
            {
                throw new PackFormatException(PackErrorKind.Manifest, $"duplicate asset id {id} (path `{path}`) in the same pack");
            }
        }

        var index = _entries.Count;
        PackFormat.ClassifyKind(kind.Value, index);
        if ((ulong)bytes.Length > PackFormat.MaxEntryLength)
        {
            throw new PackFormatException(PackErrorKind.EntryTooLarge, $"entry {index} has {bytes.Length} bytes", index);
        }

        _entries.Add(new WriterEntry(id, path, kind, kindVersion, bytes.ToArray(), SHA256.HashData(bytes)));
        return id;
    }

    /// <summary>
    /// Sets the opaque application block, at most 64 KiB.
    /// </summary>
    /// <param name="bytes">Block.</param>
    public void Application(byte[] bytes)
    {
        ArgumentNullException.ThrowIfNull(bytes);
        _application = (byte[])bytes.Clone();
    }

    /// <summary>
    /// Writes the pack.
    /// </summary>
    /// <returns>The pack bytes.</returns>
    /// <exception cref="PackFormatException">The manifest does not encode or is too large.</exception>
    public byte[] Finish()
    {
        var entries = _entries.ToArray();
        Array.Sort(entries, static (a, b) => a.Id.CompareTo(b.Id));
        var entryCount = (uint)entries.Length;

        var payloadStart = (ulong)PackFormat.HeaderLength + ((ulong)entryCount * PackFormat.TocEntryLength);
        var offsets = new ulong[entries.Length];
        var offset = payloadStart;
        for (var index = 0; index < entries.Length; index++)
        {
            offsets[index] = offset;
            offset += (ulong)entries[index].Bytes.Length;
            if (index + 1 < entries.Length)
            {
                offset = AlignUp(offset);
            }
        }

        var manifestOffset = offset;
        var body = new PackManifestBody
        {
            ManifestVersion = PackFormat.ManifestVersion,
            Compiler = _compiler,
            CompilerVersion = _compilerVersion,
            EntryCount = entryCount,
            Application = _application,
        };
        foreach (var entry in entries)
        {
            body.Paths.Add(entry.Path.Value);
        }

        byte[] manifest;
        try
        {
            manifest = body.Encode();
        }
        catch (WireFormatException error)
        {
            throw new PackFormatException(PackErrorKind.Manifest, $"manifest does not encode: {error.Message}");
        }

        if ((ulong)manifest.Length > PackFormat.MaxManifestLength)
        {
            throw new PackFormatException(PackErrorKind.Manifest, $"manifest length {manifest.Length} exceeds {PackFormat.MaxManifestLength}");
        }

        var fileLength = manifestOffset + (ulong)manifest.Length;
        var output = new byte[fileLength];
        var span = output.AsSpan();
        PackFormat.Magic.CopyTo(span);
        BinaryPrimitives.WriteUInt32LittleEndian(span.Slice(8), PackFormat.Version);
        BinaryPrimitives.WriteUInt32LittleEndian(span.Slice(12), PackFormat.HeaderLength);
        BinaryPrimitives.WriteUInt64LittleEndian(span.Slice(16), fileLength);
        BinaryPrimitives.WriteUInt64LittleEndian(span.Slice(24), PackFormat.HeaderLength);
        BinaryPrimitives.WriteUInt32LittleEndian(span.Slice(32), entryCount);
        BinaryPrimitives.WriteUInt32LittleEndian(span.Slice(36), 0);
        BinaryPrimitives.WriteUInt64LittleEndian(span.Slice(40), manifestOffset);
        BinaryPrimitives.WriteUInt64LittleEndian(span.Slice(48), (ulong)manifest.Length);
        BinaryPrimitives.WriteUInt64LittleEndian(span.Slice(56), 0);

        for (var index = 0; index < entries.Length; index++)
        {
            var entry = entries[index];
            var toc = span.Slice(PackFormat.HeaderLength + (index * PackFormat.TocEntryLength), PackFormat.TocEntryLength);
            BinaryPrimitives.WriteUInt64LittleEndian(toc, entry.Id.Value);
            BinaryPrimitives.WriteUInt16LittleEndian(toc.Slice(8), entry.Kind.Value);
            BinaryPrimitives.WriteUInt16LittleEndian(toc.Slice(10), 0);
            BinaryPrimitives.WriteUInt32LittleEndian(toc.Slice(12), entry.KindVersion);
            BinaryPrimitives.WriteUInt64LittleEndian(toc.Slice(16), offsets[index]);
            BinaryPrimitives.WriteUInt64LittleEndian(toc.Slice(24), (ulong)entry.Bytes.Length);
            entry.Sha256.CopyTo(toc.Slice(32));
            entry.Bytes.CopyTo(span.Slice((int)offsets[index]));
        }

        manifest.CopyTo(span.Slice((int)manifestOffset));
        return output;
    }

    private static ulong AlignUp(ulong value)
    {
        var remainder = value % PackFormat.Alignment;
        return remainder == 0 ? value : value + (PackFormat.Alignment - remainder);
    }

    private sealed record WriterEntry(AssetId Id, AssetPath Path, AssetKind Kind, uint KindVersion, byte[] Bytes, byte[] Sha256);
}
