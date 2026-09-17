using System;
using System.Buffers.Binary;
using System.Linq;
using System.Text;
using Grimoire.Formats.Pack;
using Grimoire.Tools.Tests;
using Xunit;

namespace Grimoire.Formats.Tests;

/// <summary>
/// Pack v1 against the engine's golden fixtures <c>pack_v1_minimal.grimpack</c> and
/// <c>pack_v1_sigil.grimpack</c> (contract §12, <c>grimoire_assets/tests/pack_format.rs</c>).
/// </summary>
public sealed class PackTests
{
    private static byte[] SigilFixture() => RustFixtures.Bytes("pack", "pack_v1_sigil.grimpack");

    private static byte[] MinimalFixture() => RustFixtures.Bytes("pack", "pack_v1_minimal.grimpack");

    [Theory]
    [InlineData("sigils/basic_bolt.sigil", 0xd95f_25b6_0afc_d01eUL)]
    [InlineData("a", 0x7a9e_604f_93d3_09f6UL)]
    [InlineData("a/b/c.txt", 0xe593_762b_ce3e_ef99UL)]
    [InlineData("fixtures/bullet_showcase.sigil", 0x348b_84e8_aa76_98c7UL)]
    [InlineData("notes/readme.txt", 0x9f5e_3495_b87b_170eUL)]
    public void AssetIdsMatchTheEnginesFrozenHashes(string path, ulong expected)
    {
        Assert.Equal(new AssetId(expected), AssetId.FromPath(AssetPath.Create(path)));
        Assert.Equal(expected.ToString("x16", System.Globalization.CultureInfo.InvariantCulture), AssetId.FromPath(AssetPath.Create(path)).ToString());
    }

    [Theory]
    [InlineData("")]
    [InlineData("/a")]
    [InlineData("a/")]
    [InlineData("a//b")]
    [InlineData("a/./b")]
    [InlineData("../a")]
    [InlineData("A/b")]
    [InlineData("a\\b")]
    [InlineData("ä")]
    [InlineData("a b")]
    public void InvalidAssetPathsAreRejected(string path)
    {
        Assert.False(AssetPath.TryCreate(path, out _, out var reason));
        Assert.NotNull(reason);
    }

    [Fact]
    public void LongestAssetPathIsAccepted()
    {
        Assert.True(AssetPath.TryCreate(new string('a', 255), out _, out _));
        Assert.False(AssetPath.TryCreate(new string('a', 256), out _, out _));
    }

    [Fact]
    public void SigilFixtureEqualsItsHandDerivedListing()
    {
        Assert.Equal(RustFixtures.HexListing("pack", "pack_v1_sigil.hex"), SigilFixture());
    }

    [Fact]
    public void SigilFixtureReadsExactlyTheDocumentedFields()
    {
        var reader = PackReader.FromBytes(SigilFixture());
        var expected = new (string Path, ulong Id, ushort Kind, uint KindVersion, ulong Length, string Sha)[]
        {
            ("fixtures/bullet_showcase.sigil", 0x348b_84e8_aa76_98c7, 1, 1, 406, "4ddfdb0f4d84a60351a027ba1f75a3f510b60bc9f17689f72784d093f9f1c9de"),
            ("notes/readme.txt", 0x9f5e_3495_b87b_170e, 0x8000, 7, 32, "adeba84acf777ce0511b66ebbed1bf042b8f1b87a1680c219cb8a1e96830999a"),
        };
        Assert.Equal(expected.Length, reader.Entries.Count);
        for (var i = 0; i < expected.Length; i++)
        {
            var entry = reader.Entries[i];
            Assert.Equal(new AssetId(expected[i].Id), entry.Id);
            Assert.Equal(expected[i].Kind, entry.Kind.Value);
            Assert.Equal(expected[i].KindVersion, entry.KindVersion);
            Assert.Equal(expected[i].Length, entry.Length);
            Assert.Equal(expected[i].Sha, Convert.ToHexStringLower(entry.Sha256));
            Assert.Equal(AssetPath.Create(expected[i].Path), reader.PathOf(entry.Id));
            Assert.Equal((int)expected[i].Length, reader.Read(entry.Id).Length);
        }

        var unit = reader.Read(new AssetId(0x348b_84e8_aa76_98c7)).Span;
        Assert.True(unit[..8].SequenceEqual("GRIMSIGL"u8));
        Assert.Equal(0x348b_84e8_aa76_98c7UL, BinaryPrimitives.ReadUInt64LittleEndian(unit[16..24]));
        Assert.Equal("grimoire pack v1 golden fixture\n", Encoding.ASCII.GetString(reader.Read(new AssetId(0x9f5e_3495_b87b_170e)).Span));
        Assert.Equal(("hand-derived", "wp8.3"), (reader.Compiler, reader.CompilerVersion));
        Assert.Equal("fixture:pack_v1_sigil", Encoding.ASCII.GetString(reader.Application.Span));
    }

    [Fact]
    public void TheWriterReproducesBothFixturesByteForByte()
    {
        var sigil = PackReader.FromBytes(SigilFixture());
        var writer = new PackWriter("hand-derived", "wp8.3");
        foreach (var entry in sigil.Entries.Reverse())
        {
            writer.Add(sigil.PathOf(entry.Id)!, entry.Kind, entry.KindVersion, sigil.Read(entry.Id).Span);
        }

        writer.Application(sigil.Application.ToArray());
        Assert.Equal(SigilFixture(), writer.Finish());

        var minimal = new PackWriter("grimoire_assets-tests", "1.0.0");
        minimal.Add(AssetPath.Create("sigils/basic_bolt.sigil"), AssetKind.Sigil, 3, "bolt-payload-bytes"u8);
        minimal.Add(AssetPath.Create("textures/enemy/core.bin"), new AssetKind(0x8001), 1, Enumerable.Repeat((byte)0xAA, 40).ToArray());
        minimal.Add(AssetPath.Create("audio/hit.wav"), AssetKind.Sigil, 1, new byte[] { 0, 1, 2, 3 });
        minimal.Application("fixture-application-block"u8.ToArray());
        Assert.Equal(MinimalFixture(), minimal.Finish());
    }

    [Fact]
    public void TheContentHashDependsOnEntriesOnly()
    {
        var reader = PackReader.FromBytes(SigilFixture());
        var writer = new PackWriter("another-compiler", "9");
        foreach (var entry in reader.Entries)
        {
            writer.Add(reader.PathOf(entry.Id)!, entry.Kind, entry.KindVersion, reader.Read(entry.Id).Span);
        }

        writer.Application(new byte[] { 1, 2, 3 });
        Assert.Equal(reader.ContentHash(), PackReader.FromBytes(writer.Finish()).ContentHash());
        Assert.NotEqual(reader.ContentHash(), PackReader.FromBytes(MinimalFixture()).ContentHash());
    }

    [Fact]
    public void ACorruptedPayloadIsCaughtOnRead()
    {
        var bytes = SigilFixture();
        bytes[192 + 20] ^= 0xFF;
        var reader = PackReader.FromBytes(bytes);
        var error = Assert.Throws<PackFormatException>(() => reader.Read(new AssetId(0x348b_84e8_aa76_98c7)));
        Assert.Equal(PackErrorKind.HashMismatch, error.Kind);
        Assert.Equal(PackErrorKind.NotFound, Assert.Throws<PackFormatException>(() => reader.Read(new AssetId(1))).Kind);
    }

    [Theory]
    [InlineData(0, PackErrorKind.BadMagic)]
    [InlineData(8, PackErrorKind.UnsupportedVersion)]
    [InlineData(12, PackErrorKind.HeaderLength)]
    [InlineData(16, PackErrorKind.FileLength)]
    [InlineData(24, PackErrorKind.OutOfBounds)]
    [InlineData(36, PackErrorKind.NonZeroReserved)]
    [InlineData(56, PackErrorKind.NonZeroReserved)]
    [InlineData(64 + 10, PackErrorKind.NonZeroReserved)]
    [InlineData(64 + 16, PackErrorKind.Misaligned)]
    public void HeaderAndTableErrorsAreReportedByKind(int offset, PackErrorKind expected)
    {
        var bytes = SigilFixture();
        bytes[offset] ^= 0x01;
        Assert.Equal(expected, Assert.Throws<PackFormatException>(() => PackReader.FromBytes(bytes)).Kind);
    }

    [Theory]
    [InlineData((ushort)0, PackErrorKind.InvalidKind)]
    [InlineData((ushort)2, PackErrorKind.ReservedKind)]
    [InlineData((ushort)5, PackErrorKind.ReservedKind)]
    [InlineData((ushort)6, PackErrorKind.InvalidKind)]
    [InlineData((ushort)0x7FFF, PackErrorKind.InvalidKind)]
    public void KindsOutsideSigilAndTheApplicationRangeAreRejected(ushort kind, PackErrorKind expected)
    {
        var bytes = SigilFixture();
        BinaryPrimitives.WriteUInt16LittleEndian(bytes.AsSpan(64 + 8), kind);
        Assert.Equal(expected, Assert.Throws<PackFormatException>(() => PackReader.FromBytes(bytes)).Kind);
        Assert.Equal(expected, Assert.Throws<PackFormatException>(() => new PackWriter("c", "v").Add(AssetPath.Create("a"), new AssetKind(kind), 1, default)).Kind);
    }

    [Fact]
    public void UnsortedIdsAndAManifestPathForAnotherIdAreRejected()
    {
        var swapped = SigilFixture();
        var first = swapped.AsSpan(64, 8).ToArray();
        swapped.AsSpan(128, 8).CopyTo(swapped.AsSpan(64, 8));
        first.CopyTo(swapped.AsSpan(128, 8));
        Assert.Equal(PackErrorKind.UnsortedIds, Assert.Throws<PackFormatException>(() => PackReader.FromBytes(swapped)).Kind);

        var renamed = SigilFixture();
        var path = Encoding.ASCII.GetBytes("notes/readme.txt");
        var at = renamed.AsSpan().LastIndexOf(path);
        renamed[at] = (byte)'m';
        Assert.Equal(PackErrorKind.ManifestMismatch, Assert.Throws<PackFormatException>(() => PackReader.FromBytes(renamed)).Kind);
    }

    [Fact]
    public void EveryTruncationFailsAndArbitraryMutationsOnlyThrowPackErrors()
    {
        var bytes = SigilFixture();
        for (var length = 0; length < bytes.Length; length++)
        {
            Assert.Throws<PackFormatException>(() => PackReader.FromBytes(bytes.AsSpan(0, length).ToArray()));
        }

        var random = new Random(0x9AC);
        for (var iteration = 0; iteration < 4000; iteration++)
        {
            var mutated = (byte[])bytes.Clone();
            for (var flips = random.Next(1, 4); flips > 0; flips--)
            {
                mutated[random.Next(mutated.Length)] = (byte)random.Next(256);
            }

            try
            {
                var reader = PackReader.FromBytes(mutated);
                foreach (var entry in reader.Entries)
                {
                    try
                    {
                        reader.Read(entry.Id);
                    }
                    catch (PackFormatException)
                    {
                    }
                }
            }
            catch (PackFormatException)
            {
            }
        }
    }

    [Fact]
    public void TheWriterRejectsDuplicatesAndOversizedManifests()
    {
        var writer = new PackWriter("c", "v");
        writer.Add(AssetPath.Create("a"), AssetKind.Sigil, 1, new byte[] { 1 });
        Assert.Equal(PackErrorKind.Manifest, Assert.Throws<PackFormatException>(() => writer.Add(AssetPath.Create("a"), AssetKind.Sigil, 1, new byte[] { 2 })).Kind);

        var application = new PackWriter("c", "v");
        application.Application(new byte[65_537]);
        Assert.Equal(PackErrorKind.Manifest, Assert.Throws<PackFormatException>(() => application.Finish()).Kind);
    }
}
