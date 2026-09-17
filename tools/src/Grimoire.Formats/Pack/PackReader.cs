using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.IO;
using System.Security.Cryptography;

namespace Grimoire.Formats.Pack;

/// <summary>
/// Constants of the pack v1 format (contract §12, <c>docs/formats/pack.md</c>).
/// </summary>
public static class PackFormat
{
    /// <summary>
    /// File magic, <c>GRIMPACK</c>.
    /// </summary>
    public static ReadOnlySpan<byte> Magic => "GRIMPACK"u8;

    /// <summary>
    /// Format version.
    /// </summary>
    public const uint Version = 1;

    /// <summary>
    /// Header length in bytes, also the table-of-contents offset.
    /// </summary>
    public const int HeaderLength = 64;

    /// <summary>
    /// Length of one table-of-contents entry.
    /// </summary>
    public const int TocEntryLength = 64;

    /// <summary>
    /// Alignment of every payload offset.
    /// </summary>
    public const ulong Alignment = 16;

    /// <summary>
    /// Most entries a pack may hold.
    /// </summary>
    public const uint MaxEntries = 65_536;

    /// <summary>
    /// Longest entry payload.
    /// </summary>
    public const ulong MaxEntryLength = 256UL * 1024 * 1024;

    /// <summary>
    /// Longest manifest.
    /// </summary>
    public const ulong MaxManifestLength = 16UL * 1024 * 1024;

    /// <summary>
    /// Longest pack file.
    /// </summary>
    public const long MaxPackLength = 1024L * 1024 * 1024;

    /// <summary>
    /// Manifest format version.
    /// </summary>
    public const uint ManifestVersion = 1;

    /// <summary>
    /// Classifies a raw kind as the reader and the writer do: 1 and 0x8000–0xFFFF are allowed.
    /// </summary>
    /// <param name="raw">Raw kind.</param>
    /// <param name="index">Entry index for the error.</param>
    /// <returns>The kind.</returns>
    /// <exception cref="PackFormatException">Kind 0 or 6–0x7FFF (invalid), 2–5 (reserved).</exception>
    public static AssetKind ClassifyKind(ushort raw, long index) => raw switch
    {
        0 => throw new PackFormatException(PackErrorKind.InvalidKind, $"entry {index} has invalid kind {raw}", index),
        1 => AssetKind.Sigil,
        >= 2 and <= 5 => throw new PackFormatException(PackErrorKind.ReservedKind, $"entry {index} has reserved kind {raw}", index),
        < AssetKind.FirstApplicationKind => throw new PackFormatException(PackErrorKind.InvalidKind, $"entry {index} has unknown kind {raw}", index),
        _ => new AssetKind(raw),
    };
}

/// <summary>
/// One table-of-contents entry of a pack.
/// </summary>
public sealed class PackEntry
{
    internal PackEntry(AssetId id, AssetKind kind, uint kindVersion, ulong length, byte[] sha256, ulong offset)
    {
        Id = id;
        Kind = kind;
        KindVersion = kindVersion;
        Length = length;
        Sha256Bytes = sha256;
        Offset = offset;
    }

    /// <summary>
    /// Asset id.
    /// </summary>
    public AssetId Id { get; }

    /// <summary>
    /// Kind.
    /// </summary>
    public AssetKind Kind { get; }

    /// <summary>
    /// Kind version, e.g. the <c>SigilUnit</c> format version.
    /// </summary>
    public uint KindVersion { get; }

    /// <summary>
    /// Payload length in bytes.
    /// </summary>
    public ulong Length { get; }

    /// <summary>
    /// SHA-256 of the payload.
    /// </summary>
    public ReadOnlySpan<byte> Sha256 => Sha256Bytes;

    /// <summary>
    /// Offset of the payload in the pack.
    /// </summary>
    public ulong Offset { get; }

    internal byte[] Sha256Bytes { get; }
}

/// <summary>
/// Reads a pack v1 (contract §12). <see cref="FromBytes"/> validates the whole structure without
/// hashing payloads; <see cref="Read"/> verifies an entry's SHA-256 on every call. Malformed input only
/// ever throws <see cref="PackFormatException"/>.
/// </summary>
public sealed class PackReader
{
    private readonly byte[] _bytes;
    private readonly PackEntry[] _entries;
    private readonly AssetPath[] _paths;

    private PackReader(byte[] bytes, PackEntry[] entries, AssetPath[] paths, string compiler, string compilerVersion, byte[] application)
    {
        _bytes = bytes;
        _entries = entries;
        _paths = paths;
        Compiler = compiler;
        CompilerVersion = compilerVersion;
        Application = application;
    }

    /// <summary>
    /// Entries in ascending id order.
    /// </summary>
    public IReadOnlyList<PackEntry> Entries => _entries;

    /// <summary>
    /// Name of the tool that wrote the pack.
    /// </summary>
    public string Compiler { get; }

    /// <summary>
    /// Version of that tool.
    /// </summary>
    public string CompilerVersion { get; }

    /// <summary>
    /// The opaque application block.
    /// </summary>
    public ReadOnlyMemory<byte> Application { get; }

    /// <summary>
    /// Reads and validates the pack file at <paramref name="path"/>, refusing files longer than
    /// <see cref="PackFormat.MaxPackLength"/> before reading them.
    /// </summary>
    /// <param name="path">File path.</param>
    /// <returns>The reader.</returns>
    /// <exception cref="PackFormatException">The file is too large or not a valid pack.</exception>
    /// <exception cref="IOException">The file cannot be read.</exception>
    public static PackReader Open(string path)
    {
        var length = new FileInfo(path).Length;
        if (length > PackFormat.MaxPackLength)
        {
            throw new PackFormatException(PackErrorKind.OutOfBounds, $"pack file {path} has {length} bytes, at most {PackFormat.MaxPackLength} allowed");
        }

        return FromBytes(File.ReadAllBytes(path));
    }

    /// <summary>
    /// Validates <paramref name="bytes"/> as a pack. The reader keeps the array; do not change it.
    /// </summary>
    /// <param name="bytes">Pack bytes.</param>
    /// <returns>The reader.</returns>
    /// <exception cref="PackFormatException">The bytes are not a valid pack.</exception>
    public static PackReader FromBytes(byte[] bytes)
    {
        ArgumentNullException.ThrowIfNull(bytes);
        var total = (ulong)bytes.LongLength;
        var cursor = new Cursor(bytes);

        if (!cursor.Take(8).SequenceEqual(PackFormat.Magic))
        {
            throw new PackFormatException(PackErrorKind.BadMagic, "not a pack: magic is not GRIMPACK");
        }

        var version = cursor.U32();
        if (version != PackFormat.Version)
        {
            throw new PackFormatException(PackErrorKind.UnsupportedVersion, $"unsupported pack version {version}");
        }

        var headerLength = cursor.U32();
        if (headerLength != PackFormat.HeaderLength)
        {
            throw new PackFormatException(PackErrorKind.HeaderLength, $"header length {headerLength}, expected 64");
        }

        var fileLength = cursor.U64();
        if (fileLength != total)
        {
            throw new PackFormatException(PackErrorKind.FileLength, $"declared file length {fileLength}, actual {total}");
        }

        var tocOffset = cursor.U64();
        if (tocOffset != PackFormat.HeaderLength)
        {
            throw OutOfBounds("toc_offset", tocOffset, PackFormat.HeaderLength);
        }

        var entryCount = cursor.U32();
        if (entryCount > PackFormat.MaxEntries)
        {
            throw new PackFormatException(PackErrorKind.TooManyEntries, $"{entryCount} entries, at most {PackFormat.MaxEntries} allowed");
        }

        if (cursor.U32() != 0)
        {
            throw new PackFormatException(PackErrorKind.NonZeroReserved, "flags at offset 36 are not zero");
        }

        var manifestOffset = cursor.U64();
        var manifestLength = cursor.U64();
        if (manifestLength > PackFormat.MaxManifestLength)
        {
            throw new PackFormatException(PackErrorKind.Manifest, $"manifest length {manifestLength} exceeds {PackFormat.MaxManifestLength}");
        }

        if (cursor.U64() != 0)
        {
            throw new PackFormatException(PackErrorKind.NonZeroReserved, "reserved field at offset 56 is not zero");
        }

        var tocLength = (ulong)entryCount * PackFormat.TocEntryLength;
        var tocEnd = tocOffset + tocLength;
        if (tocEnd > total)
        {
            throw new PackFormatException(PackErrorKind.UnexpectedEnd, $"table of contents needs {tocLength} bytes at offset {tocOffset}, {total - Math.Min(total, tocOffset)} available");
        }

        if (manifestOffset < tocEnd)
        {
            throw OutOfBounds("manifest", manifestOffset, manifestLength);
        }

        if (manifestOffset > ulong.MaxValue - manifestLength || manifestOffset + manifestLength != total)
        {
            throw OutOfBounds("manifest", manifestOffset, manifestLength);
        }

        var entries = new PackEntry[entryCount];
        AssetId? previousId = null;
        var previousEnd = tocEnd;
        for (var index = 0; index < entryCount; index++)
        {
            cursor.Seek(tocOffset + ((ulong)index * PackFormat.TocEntryLength));
            var id = new AssetId(cursor.U64());
            var kindRaw = cursor.U16();
            var reservedOffset = cursor.Position;
            if (cursor.U16() != 0)
            {
                throw new PackFormatException(PackErrorKind.NonZeroReserved, $"reserved field at offset {reservedOffset} is not zero", index);
            }

            var kindVersion = cursor.U32();
            var offset = cursor.U64();
            var length = cursor.U64();
            var sha256 = cursor.Take(32).ToArray();

            if (previousId is { } previous && id.Value <= previous.Value)
            {
                throw new PackFormatException(PackErrorKind.UnsortedIds, $"entry {index} is not in ascending id order", index);
            }

            previousId = id;
            var kind = PackFormat.ClassifyKind(kindRaw, index);
            if (length > PackFormat.MaxEntryLength)
            {
                throw new PackFormatException(PackErrorKind.EntryTooLarge, $"entry {index} has {length} bytes", index);
            }

            if (offset % PackFormat.Alignment != 0)
            {
                throw new PackFormatException(PackErrorKind.Misaligned, $"entry {index} offset {offset} is not a multiple of 16", index);
            }

            if (offset < previousEnd)
            {
                throw new PackFormatException(PackErrorKind.Overlap, $"entry {index} overlaps the previous structure", index);
            }

            if (offset > ulong.MaxValue - length || offset + length > manifestOffset)
            {
                throw OutOfBounds("payload", offset, length);
            }

            previousEnd = offset + length;
            entries[index] = new PackEntry(id, kind, kindVersion, length, sha256, offset);
        }

        var manifestBytes = bytes.AsSpan((int)manifestOffset);
        CheckManifestPrefix(manifestBytes, entryCount);
        PackManifestBody body;
        try
        {
            body = PackManifestBody.Decode(manifestBytes);
        }
        catch (WireFormatException error)
        {
            throw new PackFormatException(PackErrorKind.Manifest, $"manifest does not decode: {error.Message}");
        }

        var paths = new AssetPath[entryCount];
        for (var index = 0; index < entryCount; index++)
        {
            var text = body.Paths[index];
            if (!AssetPath.TryCreate(text, out var path, out _))
            {
                throw new PackFormatException(PackErrorKind.Manifest, $"manifest path `{text}` at index {index} is invalid", index);
            }

            if (AssetId.FromPath(path!) != entries[index].Id)
            {
                throw new PackFormatException(PackErrorKind.ManifestMismatch, $"manifest path at index {index} does not match its entry id", index);
            }

            paths[index] = path!;
        }

        return new PackReader(bytes, entries, paths, body.Compiler, body.CompilerVersion, body.Application);
    }

    /// <summary>
    /// The path of entry <paramref name="id"/>, or <c>null</c>.
    /// </summary>
    /// <param name="id">Asset id.</param>
    /// <returns>The path.</returns>
    public AssetPath? PathOf(AssetId id)
    {
        var index = IndexOf(id);
        return index < 0 ? null : _paths[index];
    }

    /// <summary>
    /// The payload of entry <paramref name="id"/>, after verifying its SHA-256.
    /// </summary>
    /// <param name="id">Asset id.</param>
    /// <returns>The payload, a view into the pack bytes.</returns>
    /// <exception cref="PackFormatException"><see cref="PackErrorKind.NotFound"/> or <see cref="PackErrorKind.HashMismatch"/>.</exception>
    public ReadOnlyMemory<byte> Read(AssetId id)
    {
        var index = IndexOf(id);
        if (index < 0)
        {
            throw new PackFormatException(PackErrorKind.NotFound, $"no entry with id {id}");
        }

        var entry = _entries[index];
        var payload = new ReadOnlyMemory<byte>(_bytes, (int)entry.Offset, (int)entry.Length);
        Span<byte> digest = stackalloc byte[32];
        SHA256.HashData(payload.Span, digest);
        if (!digest.SequenceEqual(entry.Sha256))
        {
            throw new PackFormatException(PackErrorKind.HashMismatch, $"entry {id} does not match its SHA-256", index);
        }

        return payload;
    }

    /// <summary>
    /// The content hash of the pack's entries (contract §12): SHA-256 over <c>grimoire.content.v1\0</c>,
    /// the entry count and, per entry in id order, id, kind, kind version, length and SHA-256. Paths,
    /// compiler and application block do not enter it.
    /// </summary>
    /// <returns>The 32-byte hash.</returns>
    public byte[] ContentHash()
    {
        using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        hash.AppendData("grimoire.content.v1\0"u8);
        Span<byte> scratch = stackalloc byte[8];
        BinaryPrimitives.WriteUInt32LittleEndian(scratch, (uint)_entries.Length);
        hash.AppendData(scratch[..4]);
        foreach (var entry in _entries)
        {
            BinaryPrimitives.WriteUInt64LittleEndian(scratch, entry.Id.Value);
            hash.AppendData(scratch);
            BinaryPrimitives.WriteUInt16LittleEndian(scratch, entry.Kind.Value);
            hash.AppendData(scratch[..2]);
            BinaryPrimitives.WriteUInt32LittleEndian(scratch, entry.KindVersion);
            hash.AppendData(scratch[..4]);
            BinaryPrimitives.WriteUInt64LittleEndian(scratch, entry.Length);
            hash.AppendData(scratch);
            hash.AppendData(entry.Sha256);
        }

        return hash.GetHashAndReset();
    }

    private int IndexOf(AssetId id)
    {
        int low = 0, high = _entries.Length - 1;
        while (low <= high)
        {
            var mid = low + ((high - low) / 2);
            var value = _entries[mid].Id.Value;
            if (value == id.Value)
            {
                return mid;
            }

            if (value < id.Value)
            {
                low = mid + 1;
            }
            else
            {
                high = mid - 1;
            }
        }

        return -1;
    }

    private static PackFormatException OutOfBounds(string what, ulong offset, ulong length) =>
        new(PackErrorKind.OutOfBounds, $"{what} at offset {offset} with length {length} is out of bounds");

    /// <summary>
    /// Rejects a wrong manifest version or entry count with the specific error before the generic
    /// manifest decode, as <c>grimoire_assets</c> does; a prefix too short to read is left to the decode.
    /// </summary>
    private static void CheckManifestPrefix(ReadOnlySpan<byte> manifest, uint tocEntryCount)
    {
        if (manifest.Length < 4)
        {
            return;
        }

        var version = BinaryPrimitives.ReadUInt32LittleEndian(manifest);
        if (version != PackFormat.ManifestVersion)
        {
            throw new PackFormatException(PackErrorKind.Manifest, $"unsupported manifest_version {version}");
        }

        var position = 4;
        for (var i = 0; i < 2; i++)
        {
            if (manifest.Length - position < 2)
            {
                return;
            }

            var length = BinaryPrimitives.ReadUInt16LittleEndian(manifest.Slice(position));
            position += 2;
            if (manifest.Length - position < length)
            {
                return;
            }

            position += length;
        }

        if (manifest.Length - position < 4)
        {
            return;
        }

        var count = BinaryPrimitives.ReadUInt32LittleEndian(manifest.Slice(position));
        if (count != tocEntryCount)
        {
            throw new PackFormatException(PackErrorKind.ManifestMismatch, $"manifest declares {count} entries, the table of contents {tocEntryCount}", count);
        }
    }

    private ref struct Cursor
    {
        private readonly ReadOnlySpan<byte> _bytes;

        public Cursor(ReadOnlySpan<byte> bytes)
        {
            _bytes = bytes;
            Position = 0;
        }

        public ulong Position { get; private set; }

        public void Seek(ulong position) => Position = position;

        public ReadOnlySpan<byte> Take(int length)
        {
            var available = (ulong)_bytes.Length - Math.Min((ulong)_bytes.Length, Position);
            if ((ulong)length > available)
            {
                throw new PackFormatException(PackErrorKind.UnexpectedEnd, $"input ends early: {length} bytes needed at offset {Position}, {available} available");
            }

            var slice = _bytes.Slice((int)Position, length);
            Position += (ulong)length;
            return slice;
        }

        public ushort U16() => BinaryPrimitives.ReadUInt16LittleEndian(Take(2));

        public uint U32() => BinaryPrimitives.ReadUInt32LittleEndian(Take(4));

        public ulong U64() => BinaryPrimitives.ReadUInt64LittleEndian(Take(8));
    }
}
