# Pack Format — Pack v1 (`GRIMPACK`)

Hand-written format document of pack v1, the offline-compiled binary asset pack of Grimoire
(contract §12, PRD-0002 FR-02, project ADR-0007). Implemented in `grimoire_assets`
(`crates/grimoire_assets/src/pack.rs`: `PackReader`, `PackWriter`). This page describes what that
code does; a divergence between this page and the crate is a bug in one of the two. Contract §12 is
the binding short form.

The manifest's field table is generated from `schema/pack_manifest_v1.gschema` by
`grimoire_schemagen` (project ADR-0011) into [`pack-manifest.md`](pack-manifest.md), and so is the
manifest codec `PackManifestBody` that `PackReader` and `PackWriter` use. Everything else on this
page (header, table of contents, payload placement, kinds, checks) is hand-written, because it is
fixed-size records and cross-structure validation rather than wire vocabulary.

## 1. Overview

A pack is one file of four regions, all little-endian:

```text
offset 0            64             64 + 64·N           manifest_offset          file end
       ┌────────────┬──────────────┬───────────────────┬────────────────────────┐
       │ header     │ table of     │ payloads          │ manifest               │
       │ 64 bytes   │ contents     │ one per entry,    │ compiler, paths,       │
       │            │ 64 bytes × N │ offsets aligned   │ application block      │
       │            │              │ to 16             │ (ends at the file end) │
       └────────────┴──────────────┴───────────────────┴────────────────────────┘
```

A pack is content-addressed: every entry is named by an `AssetId`, a stable hash of its path
([§6](#6-asset-paths-and-ids)), carries the SHA-256 of its payload, and has a kind with a kind
version ([§5](#5-kinds)). Nothing in a pack depends on time or on the order in which a writer was
given its entries: the same content always produces the same bytes.

## 2. Header (64 bytes)

| Offset | Field | Type | Rule | Error otherwise |
|---|---|---|---|---|
| 0 | `magic` | `[u8; 8]` | `GRIMPACK` | `BadMagic` |
| 8 | `format_version` | `u32` | `1` | `UnsupportedVersion` |
| 12 | `header_len` | `u32` | `64` | `HeaderLength` |
| 16 | `file_len` | `u64` | equals the input length | `FileLength` |
| 24 | `toc_offset` | `u64` | `64` | `OutOfBounds { what: "toc_offset" }` |
| 32 | `entry_count` | `u32` | at most `MAX_ENTRIES` (65 536) | `TooManyEntries` |
| 36 | `flags` | `u32` | `0` | `NonZeroReserved { offset: 36 }` |
| 40 | `manifest_offset` | `u64` | at or after the end of the table of contents | `OutOfBounds { what: "manifest" }` |
| 48 | `manifest_len` | `u64` | at most `MAX_MANIFEST_LEN` (16 MiB); `manifest_offset + manifest_len` equals the file length | `Manifest` / `OutOfBounds { what: "manifest" }` |
| 56 | `reserved` | `u64` | `0` | `NonZeroReserved { offset: 56 }` |

A header shorter than 64 bytes, or a table of contents that does not fit into the file, is
`UnexpectedEnd`.

## 3. Table of contents (64 bytes per entry)

`entry_count` records directly after the header, in **strictly ascending `AssetId` order**:

| Offset | Field | Type | Rule | Error otherwise |
|---|---|---|---|---|
| 0 | `id` | `u64` | greater than the previous record's id | `UnsortedIds` |
| 8 | `kind` | `u16` | see [§5](#5-kinds) | `InvalidKind` / `ReservedKind` |
| 10 | `reserved` | `u16` | `0` | `NonZeroReserved` |
| 12 | `kind_version` | `u32` | any; its meaning belongs to the kind | — |
| 16 | `offset` | `u64` | a multiple of `PACK_ALIGN` (16) | `Misaligned` |
| 24 | `len` | `u64` | at most `MAX_ENTRY_LEN` (256 MiB) | `EntryTooLarge` |
| 32 | `sha256` | `[u8; 32]` | SHA-256 of the payload, checked on every read | `HashMismatch` (on read) |

## 4. Payloads

- Payloads lie in table-of-contents order. The first starts no earlier than the end of the table of
  contents, each starts no earlier than the end of the one before it (`Overlap` otherwise), and the
  last ends no later than `manifest_offset` (`OutOfBounds { what: "payload" }` otherwise).
- Every offset is a multiple of 16. The writer places the first payload directly after the table of
  contents (which always ends on a multiple of 16), pads between payloads with zero bytes to the
  next multiple of 16, and puts the manifest directly after the last payload, without padding.
  The reader accepts any gap and does not check padding bytes; only the writer's layout is
  canonical.
- `PackReader::from_bytes` checks the structure in O(entries) and hashes no payload. Every
  `AssetSource::read` hashes the payload it returns and reports `HashMismatch` if the SHA-256 in
  the table of contents does not match. Payloads are returned without copying, borrowed from the
  shared buffer.

## 5. Kinds

| Kind | Name | v1 reader | v1 writer | `kind_version` |
|---|---|---|---|---|
| `0` | — | `InvalidKind` | rejects | — |
| `1` | `SIGIL` | accepts | writes | the binary format version of the `SigilUnit` (contract §11.1), currently `1` |
| `2` | `MESH` | `ReservedKind` | rejects | reserved |
| `3` | `MATERIAL` | `ReservedKind` | rejects | reserved |
| `4` | `AUDIO` | `ReservedKind` | rejects | reserved |
| `5` | `TEMPLATE` | `ReservedKind` | rejects | reserved |
| `6`–`0x7FFF` | — | `InvalidKind` | rejects | — |
| `0x8000`–`0xFFFF` | application-defined | accepts, passes through opaquely | writes | defined by the application |

**Sigil entries.** A `SIGIL` entry's path is the canonical content path the unit was compiled
under, so its `AssetId` equals the unit's `UnitId` (contract §11.1: same derivation). The facade
adapter `grimoire::adapters::assets` ([§10](#10-loading-sigil-units)) checks both.

**The figure pack.** The game's figure pack (`figures.pack`, written by its `figure_pack_builder`
with `PackWriter`) is an ordinary pack v1 file. Its payload formats (`FNP_MESH` and the other
`figure_format` payloads of `grimoire_render`) use the application range, not the reserved engine
kinds: `FNP_MESH` `0x8000`, `FNP_TEXTURE_RAW` `0x8001`, `FNP_SKELETON` `0x8002`, `FNP_FIGURE`
`0x8003`, `FNP_MATERIAL` `0x8004` (`grimoire::adapters::figure_assets`). Kinds `2`–`5` stay
reserved for engine formats that do not exist yet. When the engine defines one (for example a mesh
format owned by the renderer), a contract change unreserves the kind, fixes the meaning of its
`kind_version`, and the game's pack builder moves its entries from the application range to the
engine kind; until then nothing about the figure pack changes.

## 6. Asset paths and ids

- An `AssetPath` is 1 to 255 bytes of ASCII `[a-z0-9_.-]` with `/` as separator: no leading or
  trailing `/`, no empty segment, no `.` or `..` segment. Backslashes, uppercase letters and
  non-ASCII bytes are rejected (`AssetError::InvalidPath`); the asset compiler normalises paths,
  the engine only rejects them.
- `AssetId::from_path(p)` is a fresh `StableHasher` v1 (contract §4) fed with
  `write_str("grimoire.asset-id.v1")` and then `write_str(p)`, finished with `finish()`. `sigilc`
  derives a `UnitId` by exactly the same rule without depending on `grimoire_assets`.
- Two paths with the same id in one pack are a writer error; a reader sees them as `UnsortedIds`.

## 7. Manifest

The manifest ends exactly at the file end. Field table: [`pack-manifest.md`](pack-manifest.md).

| Field | Wire type | Rule |
|---|---|---|
| `manifest_version` | `u32` | `1` |
| `compiler` | `Str16`, at most 64 bytes | name of the producing tool, e.g. `sigilc` or `grimoire-ac` |
| `compiler_version` | `Str16`, at most 64 bytes | its version; the compiler version lives here, never in a unit (contract §11.1) |
| `entry_count` | `u32` | equals the table of contents' `entry_count` |
| `paths` | `entry_count` × `Str16`, each at most 255 bytes, no separate count | one path per entry, in table-of-contents order; each must be a valid `AssetPath` whose `AssetId` is the id of the entry at the same index |
| `application` | `u32` length + bytes, at most 64 KiB | opaque to the engine, e.g. a game version string |

`Str16` is a `u16` byte length followed by UTF-8. There is no timestamp.

**Reader order.** `PackReader` first walks the fixed prefix of the manifest without allocating:
a `manifest_version` other than `1` is `Manifest`, an `entry_count` other than the table of
contents' is `ManifestMismatch { index: <the manifest's count> }`. Only then does the generated
codec decode the manifest, so the path vector is bounded by an entry count that has already been
checked against the input length (contract §2 rule 9). Every codec error (a string over its limit,
invalid UTF-8, bytes missing or left over) is `Manifest`; an invalid path is `Manifest`; a path
whose id does not match its table-of-contents entry is `ManifestMismatch { index }`.

## 8. Limits

| Constant | Value | Checked |
|---|---|---|
| `MAX_ENTRIES` | 65 536 | header `entry_count`, manifest path count |
| `MAX_PATH_LEN` | 255 bytes | `AssetPath`, manifest paths |
| `MAX_ENTRY_LEN` | 256 MiB | table-of-contents `len` |
| `MAX_MANIFEST_LEN` | 16 MiB | header `manifest_len`, writer |
| `MAX_PACK_LEN` | 1 GiB | `PackReader::open` reads through `FileSystem::read_limited` and never loads a larger file (`AssetError::TooLarge`) |
| compiler strings | 64 bytes each | manifest |
| application block | 64 KiB | manifest |

Every offset and length computation uses checked arithmetic, and every count and length is checked
against the remaining input and its limit before anything is allocated or sliced. No input makes the
reader panic (contract §2 rule 9, property tests in `tests/pack_fuzz.rs`).

## 9. Content hash

`AssetSource::content_hash` is SHA-256 over `b"grimoire.content.v1\0"`, the entry count as `u32`,
and for every entry in `entries()` order its `id` (`u64`), `kind` (`u16`), `kind_version` (`u32`),
`len` (`u64`) and `sha256`. Paths, compiler and application block do not enter it, so a
`PackReader` and a `MemorySource` with the same content have the same hash. It identifies a source's
content (diagnostics, pack comparison, C# conformance); it is not the session's content manifest
hash, which `grimoire_sigil` forms from the loaded units and the behaviour registry (contract §11.8).

## 10. Loading Sigil units

`grimoire::adapters::assets` (contract §9.11) turns the `SIGIL` entries of any `AssetSource` into the
`SigilLibrary` that `grimoire_sigil::install` takes: `sigil_units` reads each entry (SHA-256
checked), requires `kind_version == SigilUnit::FORMAT_VERSION`, decodes it with
`SigilUnit::from_bytes` and requires the unit's `UnitId` to equal the entry's `AssetId`;
`sigil_library` builds the library against a behaviour registry. Other kinds are left alone.

## 11. Golden fixtures and tools

| File | Content | Origin |
|---|---|---|
| `crates/grimoire_assets/tests/fixtures/pack_v1_minimal.grimpack` | three entries (two `SIGIL` with placeholder payloads, one application kind), 476 bytes | written by `PackWriter` in WP1.3 and hand-checked; reproduced byte for byte by `PackWriter`; the C# conformance tests (Plan 0002 WP9.1) use it |
| `crates/grimoire_assets/tests/fixtures/pack_v1_sigil.grimpack` | a real `SigilUnit` (`fixtures/bullet_showcase.sigil`) and an application entry, 744 bytes | **hand-derived** (WP8.3): `pack_v1_sigil.hex` next to it is the derivation, every field written out from this page with comments, SHA-256 from an independent tool and ids from an independent re-implementation of `StableHasher` v1; the test checks file = listing, the fields read back, and `PackWriter` reproducing it |

`cargo run -p grimoire_assets --example pack_inspect -- <file>` prints a pack's manifest and one
line per entry (id, kind, kind version, size, SHA-256 prefix, read check, path) and its content hash;
it exits with `1` for a structural error or an entry that does not read back.
