//! Pack v1: a little-endian binary format holding a header, a table of contents, payload bytes
//! and a trailing manifest (contract §12).
//!
//! Layout (all multi-byte fields little-endian):
//!
//! 1. Header, exactly 64 bytes: magic (8), format version `u32`, header length `u32` (= 64),
//!    file length `u64`, TOC offset `u64` (= 64), entry count `u32`, flags `u32` (= 0), manifest
//!    offset `u64`, manifest length `u64`, reserved `u64` (= 0).
//! 2. TOC: one 64-byte record per entry, in strictly ascending [`AssetId`] order: id `u64`, kind
//!    `u16`, reserved `u16` (= 0), kind version `u32`, payload offset `u64` (a multiple of
//!    [`PACK_ALIGN`]), payload length `u64`, SHA-256 (32 bytes).
//! 3. Payload: one region per entry, in TOC order, packed with zero padding so every offset stays
//!    aligned; the first entry starts no earlier than right after the TOC and the last entry ends
//!    no later than the manifest offset.
//! 4. Manifest: ends exactly at the end of the file. `manifest_version` `u32` (= 1), compiler name
//!    and version as `Str16` (`u16` length + UTF-8, each at most 64 bytes), entry count `u32`
//!    (= TOC count), one path per entry (`Str16`, TOC order), then an opaque application block
//!    (`u32` length + bytes, at most 64 KiB). No timestamp anywhere: identical inputs to
//!    [`PackWriter`] always produce byte-identical output.
//!
//! [`PackReader::from_bytes`] never panics on any input (contract §2 rule 9): every length and
//! offset is checked against the remaining input and a documented upper bound before it is used
//! to slice, index or allocate.

use std::borrow::Cow;
use std::io;
use std::path::Path;
use std::sync::Arc;

use grimoire_platform::FileSystem;

use crate::error::{AssetError, PackError};
use crate::ids::{AssetEntry, AssetId, AssetKind, AssetPath, Sha256, sha256_of};
use crate::source::AssetSource;
use crate::{
    MAX_ENTRIES, MAX_ENTRY_LEN, MAX_MANIFEST_LEN, MAX_PACK_LEN, MAX_PATH_LEN, PACK_ALIGN,
    PACK_FORMAT_VERSION, PACK_MAGIC,
};

/// Fixed size of the pack v1 header in bytes.
const HEADER_LEN: u64 = 64;
/// Fixed size of one TOC record in bytes.
const TOC_ENTRY_LEN: u64 = 64;
/// Upper bound (bytes) for the compiler name and compiler version manifest strings.
const MAX_COMPILER_STR_LEN: usize = 64;
/// Upper bound (bytes) for the opaque application block.
const MAX_APPLICATION_LEN: usize = 64 * 1024;
/// Fixed manifest format version written and required by pack v1.
const MANIFEST_VERSION: u32 = 1;

/// Classifies a raw on-disk kind value (contract §12): `0` and `6..=0x7FFF` are invalid, `2..=5`
/// are reserved (a v1 reader rejects them, a v1 writer never emits them), `1` is `SIGIL`, and
/// `0x8000..=0xFFFF` are opaque application-defined kinds.
fn classify_kind(raw: u16, index: u32) -> Result<AssetKind, PackError> {
    match raw {
        0 => Err(PackError::InvalidKind { index, kind: raw }),
        1 => Ok(AssetKind::SIGIL),
        2..=5 => Err(PackError::ReservedKind { index, kind: raw }),
        6..=0x7FFF => Err(PackError::InvalidKind { index, kind: raw }),
        _ => Ok(AssetKind(raw)),
    }
}

/// Rounds `value` up to the next multiple of `align` (`align` must be a power of two greater than
/// zero; here always [`PACK_ALIGN`]).
fn align_up(value: u64, align: u64) -> u64 {
    let remainder = value % align;
    if remainder == 0 {
        value
    } else {
        value + (align - remainder)
    }
}

/// Bounds-checked little-endian cursor over a whole pack byte slice.
///
/// Every read returns [`PackError::UnexpectedEnd`] instead of panicking when fewer bytes remain
/// than requested, and every multi-byte value is decoded without any `unwrap`/`expect`/panic path
/// (contract §2 rule 9).
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// Absolute byte offset of the read head, for error reporting.
    fn offset(&self) -> u64 {
        self.pos as u64
    }

    /// Moves the read head to an absolute offset already known to be within bounds (callers only
    /// seek to offsets they have already validated, e.g. a TOC record start or the manifest
    /// start).
    fn seek(&mut self, pos: u64) {
        self.pos = (pos as usize).min(self.bytes.len());
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], PackError> {
        match self.pos.checked_add(len) {
            Some(end) if end <= self.bytes.len() => {
                let slice = &self.bytes[self.pos..end];
                self.pos = end;
                Ok(slice)
            }
            _ => Err(PackError::UnexpectedEnd {
                offset: self.offset(),
                needed: len as u64,
                available: (self.bytes.len() - self.pos) as u64,
            }),
        }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], PackError> {
        let slice = self.take(N)?;
        let mut out = [0u8; N];
        // `take(N)` guarantees `slice.len() == N`.
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn u16(&mut self) -> Result<u16, PackError> {
        let low = u16::from(self.take(1)?[0]);
        let high = u16::from(self.take(1)?[0]);
        Ok(low | (high << 8))
    }

    fn u32(&mut self) -> Result<u32, PackError> {
        let low = u32::from(self.u16()?);
        let high = u32::from(self.u16()?);
        Ok(low | (high << 16))
    }

    fn u64(&mut self) -> Result<u64, PackError> {
        let low = u64::from(self.u32()?);
        let high = u64::from(self.u32()?);
        Ok(low | (high << 32))
    }

    /// Reads a `Str16`: a `u16` length prefix followed by that many UTF-8 bytes, rejecting a
    /// declared length over `max_len` before allocating anything for the string.
    fn str16(&mut self, max_len: usize) -> Result<String, PackError> {
        let offset = self.offset();
        let len = usize::from(self.u16()?);
        if len > max_len {
            return Err(PackError::Manifest(format!(
                "string at offset {offset} has length {len}, exceeding the {max_len}-byte limit"
            )));
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| {
            PackError::Manifest(format!("string at offset {offset} is not valid UTF-8"))
        })
    }
}

/// The manifest section of a pack: compiler provenance, the paths behind each id, and an opaque
/// application-defined block.
///
/// Paths are stored in the same order as [`AssetSource::entries`] (ascending by [`AssetId`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PackManifest {
    compiler: String,
    compiler_version: String,
    paths: Vec<AssetPath>,
    application: Vec<u8>,
}

impl PackManifest {
    /// Name of the tool that produced the pack (e.g. `"sigilc"`).
    #[must_use]
    pub fn compiler(&self) -> &str {
        &self.compiler
    }

    /// Version string of the tool that produced the pack.
    #[must_use]
    pub fn compiler_version(&self) -> &str {
        &self.compiler_version
    }

    /// Looks up the source path for `id`, if the manifest lists one.
    #[must_use]
    pub fn path_of(&self, id: AssetId) -> Option<&AssetPath> {
        self.paths
            .iter()
            .find(|path| AssetId::from_path(path) == id)
    }

    /// The opaque application-defined block (e.g. a game version string), at most 64 KiB.
    #[must_use]
    pub fn application(&self) -> &[u8] {
        &self.application
    }
}

/// Reads pack v1 byte streams (contract §12).
///
/// Cheap to clone: the backing bytes are shared through an [`Arc`].
#[derive(Clone, Debug)]
pub struct PackReader {
    bytes: Arc<[u8]>,
    entries: Vec<AssetEntry>,
    /// Payload offset of `entries[i]`, index-aligned with `entries`. Not part of [`AssetEntry`]
    /// itself: offsets are meaningless for other `AssetSource` implementations.
    offsets: Vec<u64>,
    manifest: PackManifest,
}

impl PackReader {
    /// Parses a pack v1 byte stream.
    ///
    /// Runs in O(entry count) and never reads or hashes payload bytes; [`AssetSource::read`]
    /// verifies each payload's SHA-256 the first time it is actually read.
    ///
    /// # Errors
    /// Returns a [`PackError`] describing the first structural problem found; never panics, for
    /// any input (contract §2 rule 9).
    // This function is long because it is one careful, sequentially ordered validation pass over
    // the header, TOC and manifest (contract §2 rule 9); splitting it up would scatter that order
    // rather than clarify it.
    pub fn from_bytes(bytes: Arc<[u8]>) -> Result<Self, PackError> {
        let total_len = bytes.len() as u64;
        let mut cursor = Cursor::new(&bytes);

        let magic: [u8; 8] = cursor.array()?;
        if magic != PACK_MAGIC {
            return Err(PackError::BadMagic);
        }
        let version = cursor.u32()?;
        if version != PACK_FORMAT_VERSION {
            return Err(PackError::UnsupportedVersion(version));
        }
        let header_len = cursor.u32()?;
        if u64::from(header_len) != HEADER_LEN {
            return Err(PackError::HeaderLength(header_len));
        }
        let file_len = cursor.u64()?;
        if file_len != total_len {
            return Err(PackError::FileLength {
                declared: file_len,
                actual: total_len,
            });
        }
        let toc_offset = cursor.u64()?;
        if toc_offset != HEADER_LEN {
            return Err(PackError::OutOfBounds {
                what: "toc_offset",
                offset: toc_offset,
                len: HEADER_LEN,
            });
        }
        let entry_count = cursor.u32()?;
        if entry_count > MAX_ENTRIES {
            return Err(PackError::TooManyEntries(entry_count));
        }
        let flags = cursor.u32()?;
        if flags != 0 {
            return Err(PackError::NonZeroReserved { offset: 36 });
        }
        let manifest_offset = cursor.u64()?;
        let manifest_len = cursor.u64()?;
        if manifest_len > MAX_MANIFEST_LEN {
            return Err(PackError::Manifest(format!(
                "manifest length {manifest_len} exceeds MAX_MANIFEST_LEN ({MAX_MANIFEST_LEN})"
            )));
        }
        let reserved = cursor.u64()?;
        if reserved != 0 {
            return Err(PackError::NonZeroReserved { offset: 56 });
        }

        let toc_start = toc_offset;
        let toc_len = u64::from(entry_count)
            .checked_mul(TOC_ENTRY_LEN)
            .ok_or(PackError::TooManyEntries(entry_count))?;
        let toc_end = toc_start
            .checked_add(toc_len)
            .ok_or(PackError::OutOfBounds {
                what: "toc",
                offset: toc_start,
                len: toc_len,
            })?;
        if toc_end > total_len {
            return Err(PackError::UnexpectedEnd {
                offset: toc_start,
                needed: toc_len,
                available: total_len.saturating_sub(toc_start),
            });
        }
        if manifest_offset < toc_end {
            return Err(PackError::OutOfBounds {
                what: "manifest",
                offset: manifest_offset,
                len: manifest_len,
            });
        }
        let manifest_end =
            manifest_offset
                .checked_add(manifest_len)
                .ok_or(PackError::OutOfBounds {
                    what: "manifest",
                    offset: manifest_offset,
                    len: manifest_len,
                })?;
        if manifest_end != total_len {
            return Err(PackError::OutOfBounds {
                what: "manifest",
                offset: manifest_offset,
                len: manifest_len,
            });
        }

        let mut entries = Vec::with_capacity(entry_count as usize);
        let mut offsets = Vec::with_capacity(entry_count as usize);
        let mut previous_id: Option<AssetId> = None;
        let mut previous_end = toc_end;
        for index in 0..entry_count {
            // `index < entry_count <= MAX_ENTRIES` and `toc_start + index * TOC_ENTRY_LEN <
            // toc_end <= total_len`, already checked above, so this multiplication cannot
            // overflow and the seek target is in bounds.
            let entry_start = toc_start + u64::from(index) * TOC_ENTRY_LEN;
            cursor.seek(entry_start);

            let id = AssetId(cursor.u64()?);
            let kind_raw = cursor.u16()?;
            let toc_reserved_offset = cursor.offset();
            let toc_reserved = cursor.u16()?;
            if toc_reserved != 0 {
                return Err(PackError::NonZeroReserved {
                    offset: toc_reserved_offset,
                });
            }
            let kind_version = cursor.u32()?;
            let offset = cursor.u64()?;
            let length = cursor.u64()?;
            let sha256: [u8; 32] = cursor.array()?;

            if let Some(previous_id) = previous_id
                && id <= previous_id
            {
                return Err(PackError::UnsortedIds { index });
            }
            previous_id = Some(id);

            let kind = classify_kind(kind_raw, index)?;

            if length > MAX_ENTRY_LEN {
                return Err(PackError::EntryTooLarge { index, len: length });
            }
            if offset % PACK_ALIGN != 0 {
                return Err(PackError::Misaligned { index });
            }
            if offset < previous_end {
                return Err(PackError::Overlap { index });
            }
            let end = offset.checked_add(length).ok_or(PackError::OutOfBounds {
                what: "payload",
                offset,
                len: length,
            })?;
            if end > manifest_offset {
                return Err(PackError::OutOfBounds {
                    what: "payload",
                    offset,
                    len: length,
                });
            }
            previous_end = end;

            entries.push(AssetEntry::new(
                id,
                kind,
                kind_version,
                length,
                Sha256(sha256),
            ));
            offsets.push(offset);
        }

        cursor.seek(manifest_offset);
        let manifest_version = cursor.u32()?;
        if manifest_version != MANIFEST_VERSION {
            return Err(PackError::Manifest(format!(
                "unsupported manifest_version {manifest_version}"
            )));
        }
        let compiler = cursor.str16(MAX_COMPILER_STR_LEN)?;
        let compiler_version = cursor.str16(MAX_COMPILER_STR_LEN)?;
        let manifest_entry_count = cursor.u32()?;
        if manifest_entry_count != entry_count {
            // No single TOC index applies to a whole-count mismatch; report the manifest's own
            // (wrong) count as the offending value.
            return Err(PackError::ManifestMismatch {
                index: manifest_entry_count,
            });
        }
        let mut paths = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            // `index` fits `u32`: bounded by `entry_count <= MAX_ENTRIES`.
            let index = index as u32;
            let path_str = cursor.str16(MAX_PATH_LEN)?;
            let path = AssetPath::new(&path_str).map_err(|_| {
                PackError::Manifest(format!(
                    "manifest path {path_str:?} at index {index} is invalid"
                ))
            })?;
            if AssetId::from_path(&path) != entry.id {
                return Err(PackError::ManifestMismatch { index });
            }
            paths.push(path);
        }
        let application_len = cursor.u32()? as usize;
        if application_len > MAX_APPLICATION_LEN {
            return Err(PackError::Manifest(format!(
                "application block length {application_len} exceeds {MAX_APPLICATION_LEN} bytes"
            )));
        }
        let application = cursor.take(application_len)?.to_vec();

        if cursor.offset() != total_len {
            return Err(PackError::Manifest(format!(
                "{} trailing byte(s) after the manifest",
                total_len - cursor.offset()
            )));
        }

        Ok(Self {
            bytes,
            entries,
            offsets,
            manifest: PackManifest {
                compiler,
                compiler_version,
                paths,
                application,
            },
        })
    }

    /// Reads a pack file through `fs`, rejecting it if it is longer than [`MAX_PACK_LEN`] bytes.
    ///
    /// # Errors
    /// Returns [`AssetError::TooLarge`] if the file is longer than [`MAX_PACK_LEN`] bytes,
    /// [`AssetError::Io`] for any other I/O error, or [`AssetError::Pack`] if the bytes do not
    /// parse as a valid pack.
    pub fn open(fs: &dyn FileSystem, path: &Path) -> Result<Self, AssetError> {
        let bytes = fs
            .read_limited(path, MAX_PACK_LEN)
            .map_err(|error| match error.kind() {
                io::ErrorKind::FileTooLarge => AssetError::TooLarge {
                    path: path.display().to_string(),
                    max: MAX_PACK_LEN,
                },
                kind => AssetError::Io {
                    path: path.display().to_string(),
                    kind,
                },
            })?;
        Ok(Self::from_bytes(Arc::from(bytes))?)
    }

    /// The pack's manifest (compiler provenance, paths, application block).
    #[must_use]
    pub fn manifest(&self) -> &PackManifest {
        &self.manifest
    }
}

impl AssetSource for PackReader {
    fn name(&self) -> &str {
        "pack"
    }

    fn entries(&self) -> &[AssetEntry] {
        &self.entries
    }

    fn read(&self, id: AssetId) -> Result<Cow<'_, [u8]>, AssetError> {
        let index = self
            .entries
            .binary_search_by_key(&id, |entry| entry.id)
            .map_err(|_| AssetError::NotFound(id))?;
        let entry = &self.entries[index];
        let start = self.offsets[index] as usize;
        let end = start + entry.len as usize;
        // `from_bytes` already proved `offset + len <= manifest_offset <= self.bytes.len()` for
        // every entry, and neither `self.bytes`, `self.entries` nor `self.offsets` change after
        // construction, so this slice is always in bounds.
        let slice = &self.bytes[start..end];
        if sha256_of(slice) != entry.sha256 {
            return Err(AssetError::HashMismatch(id));
        }
        Ok(Cow::Borrowed(slice))
    }
}

struct WriterEntry {
    id: AssetId,
    path: AssetPath,
    kind: AssetKind,
    kind_version: u32,
    bytes: Vec<u8>,
    sha256: Sha256,
}

/// Reference writer for pack v1 fixtures and tests.
///
/// Deterministic: given the same set of `add`ed assets (regardless of call order) and the same
/// compiler metadata and application block, [`PackWriter::finish`] always produces byte-identical
/// output (no timestamp is ever written).
pub struct PackWriter {
    compiler: String,
    compiler_version: String,
    entries: Vec<WriterEntry>,
    application: Vec<u8>,
}

impl PackWriter {
    /// Starts a new, empty pack attributed to `compiler`/`compiler_version` (recorded in the
    /// manifest, e.g. `("sigilc", "0.3.1")`).
    #[must_use]
    pub fn new(compiler: &str, compiler_version: &str) -> Self {
        Self {
            compiler: compiler.to_owned(),
            compiler_version: compiler_version.to_owned(),
            entries: Vec::new(),
            application: Vec::new(),
        }
    }

    /// Adds one asset.
    ///
    /// # Errors
    /// Returns a [`PackError`] if: the pack already has [`MAX_ENTRIES`] entries; `path`'s id
    /// collides with an id already added (contract §12: an id collision is a write error);
    /// `kind` is `0`, reserved (`2..=5`) or otherwise unrecognised; or `bytes` is longer than
    /// [`MAX_ENTRY_LEN`].
    pub fn add(
        &mut self,
        path: &AssetPath,
        kind: AssetKind,
        kind_version: u32,
        bytes: &[u8],
    ) -> Result<AssetId, PackError> {
        if self.entries.len() >= MAX_ENTRIES as usize {
            return Err(PackError::TooManyEntries(self.entries.len() as u32 + 1));
        }
        let id = AssetId::from_path(path);
        if self.entries.iter().any(|entry| entry.id == id) {
            return Err(PackError::Manifest(format!(
                "duplicate asset id {id} (path {:?}) in the same pack",
                path.as_str()
            )));
        }
        // The index this entry would occupy if the TOC were built right now, in insertion order;
        // used only to name the offending entry in an error (the final TOC index, after sorting
        // by id in `finish`, need not be the same, but no error here depends on that).
        let index = self.entries.len() as u32;
        classify_kind(kind.0, index)?;
        if bytes.len() as u64 > MAX_ENTRY_LEN {
            return Err(PackError::EntryTooLarge {
                index,
                len: bytes.len() as u64,
            });
        }
        self.entries.push(WriterEntry {
            id,
            path: path.clone(),
            kind,
            kind_version,
            bytes: bytes.to_vec(),
            sha256: sha256_of(bytes),
        });
        Ok(id)
    }

    /// Sets the opaque application-defined block (e.g. a game version string). At most 64 KiB;
    /// checked by [`PackWriter::finish`].
    pub fn application(&mut self, bytes: Vec<u8>) {
        self.application = bytes;
    }

    /// Serialises the pack.
    ///
    /// # Errors
    /// Returns [`PackError::Manifest`] if the compiler name, compiler version or application
    /// block exceeds its length limit, or if the resulting manifest exceeds [`MAX_MANIFEST_LEN`].
    pub fn finish(self) -> Result<Vec<u8>, PackError> {
        if self.compiler.len() > MAX_COMPILER_STR_LEN {
            return Err(PackError::Manifest(format!(
                "compiler name {:?} exceeds {MAX_COMPILER_STR_LEN} bytes",
                self.compiler
            )));
        }
        if self.compiler_version.len() > MAX_COMPILER_STR_LEN {
            return Err(PackError::Manifest(format!(
                "compiler version {:?} exceeds {MAX_COMPILER_STR_LEN} bytes",
                self.compiler_version
            )));
        }
        if self.application.len() > MAX_APPLICATION_LEN {
            return Err(PackError::Manifest(format!(
                "application block of {} bytes exceeds {MAX_APPLICATION_LEN} bytes",
                self.application.len()
            )));
        }

        let mut entries = self.entries;
        // The only place insertion order matters: sorting here (rather than requiring callers to
        // `add` in id order) is what makes `finish` deterministic regardless of `add` call order.
        entries.sort_by_key(|entry| entry.id);

        let entry_count = u32::try_from(entries.len())
            .expect("`add` rejects a pack once it already holds MAX_ENTRIES (a u32) entries");

        let payload_start = HEADER_LEN + u64::from(entry_count) * TOC_ENTRY_LEN;
        let mut payload = Vec::new();
        let mut offsets = Vec::with_capacity(entries.len());
        let mut offset = payload_start;
        for (index, entry) in entries.iter().enumerate() {
            offsets.push(offset);
            payload.extend_from_slice(&entry.bytes);
            offset += entry.bytes.len() as u64;
            if index + 1 < entries.len() {
                let aligned = align_up(offset, PACK_ALIGN);
                payload.resize(payload.len() + (aligned - offset) as usize, 0);
                offset = aligned;
            }
        }
        let manifest_offset = offset;

        let mut manifest = Vec::new();
        manifest.extend_from_slice(&MANIFEST_VERSION.to_le_bytes());
        write_str16(&mut manifest, &self.compiler)?;
        write_str16(&mut manifest, &self.compiler_version)?;
        manifest.extend_from_slice(&entry_count.to_le_bytes());
        for entry in &entries {
            write_str16(&mut manifest, entry.path.as_str())?;
        }
        manifest.extend_from_slice(&(self.application.len() as u32).to_le_bytes());
        manifest.extend_from_slice(&self.application);

        let manifest_len = manifest.len() as u64;
        if manifest_len > MAX_MANIFEST_LEN {
            return Err(PackError::Manifest(format!(
                "manifest length {manifest_len} exceeds MAX_MANIFEST_LEN ({MAX_MANIFEST_LEN})"
            )));
        }
        let file_len = manifest_offset + manifest_len;

        let mut out = Vec::with_capacity(file_len as usize);
        out.extend_from_slice(&PACK_MAGIC);
        out.extend_from_slice(&PACK_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        out.extend_from_slice(&file_len.to_le_bytes());
        out.extend_from_slice(&HEADER_LEN.to_le_bytes());
        out.extend_from_slice(&entry_count.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // flags
        out.extend_from_slice(&manifest_offset.to_le_bytes());
        out.extend_from_slice(&manifest_len.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes()); // reserved

        for (entry, &entry_offset) in entries.iter().zip(offsets.iter()) {
            out.extend_from_slice(&entry.id.0.to_le_bytes());
            out.extend_from_slice(&entry.kind.0.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // reserved
            out.extend_from_slice(&entry.kind_version.to_le_bytes());
            out.extend_from_slice(&entry_offset.to_le_bytes());
            out.extend_from_slice(&(entry.bytes.len() as u64).to_le_bytes());
            out.extend_from_slice(&entry.sha256.0);
        }

        out.extend_from_slice(&payload);
        out.extend_from_slice(&manifest);

        Ok(out)
    }
}

/// Appends a `Str16` (length-prefixed UTF-8 string) to `buf`.
fn write_str16(buf: &mut Vec<u8>, s: &str) -> Result<(), PackError> {
    let bytes = s.as_bytes();
    let len = u16::try_from(bytes.len())
        .map_err(|_| PackError::Manifest(format!("string {s:?} exceeds the Str16 length limit")))?;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(bytes);
    Ok(())
}
