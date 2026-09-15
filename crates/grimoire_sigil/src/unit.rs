//! Binary format `SigilUnit` v1 (contract §11.1): header, section table, and decode/encode of the
//! sections needed by this work package.
//!
//! The *interior* byte layout of a bullet-pattern program, an emitter definition, a transform or a
//! curve is not part of this crate: it is deferred to `docs/formats/sigil.md` (WP4.1/WP4.2). What
//! this module implements for real is the header, the section table, and — because
//! `SigilLibrary`/`BulletPool` need *something* to build on — a minimal, explicitly provisional
//! encoding of bullet-type records and of the emitter/program/behavior-ref *counts*. Every place
//! that invents such a provisional layout is marked with a `PROVISIONAL` doc comment.

use std::fmt;

use grimoire_core::{StableHash, StableHasher};

use crate::behavior::BehaviorId;
use crate::content::{BulletFlags, BulletType, BulletVisual};

/// Stable identifier of a compiled Sigil unit.
///
/// `0` is reserved and never a valid id: [`SigilUnit::from_bytes`] rejects it. `sigilc` (a
/// separate, not-yet-existing tool crate) derives real ids from a unit's canonical content path;
/// this crate only defines the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitId(pub u64);

impl UnitId {
    /// The reserved, always-invalid id.
    pub const INVALID: Self = Self(0);
}

impl fmt::Display for UnitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl StableHash for UnitId {
    fn stable_hash(&self, hasher: &mut StableHasher) {
        hasher.write_u64(self.0);
    }
}

/// Errors reported by [`SigilUnit::from_bytes`].
///
/// Decoding never panics on any input (contract §2 rule 9); every error is a value of this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UnitError {
    /// The input ended before a fixed-size field could be read in full.
    #[error("unit data ends early: {needed} bytes needed at offset {offset}, {available} left")]
    UnexpectedEnd {
        /// Byte offset where the read was attempted.
        offset: usize,
        /// Size of the field that could not be read, in bytes.
        needed: usize,
        /// Bytes actually available from `offset` to the end of the input.
        available: usize,
    },
    /// The input does not start with `b"GRIMSIGL"`.
    #[error("unit data does not start with the magic bytes GRIMSIGL")]
    BadMagic,
    /// The header declares a format version this build does not support.
    #[error("unsupported unit format version {0}")]
    UnsupportedVersion(u32),
    /// A field documented as reserved (must be `0`) was non-zero.
    #[error("reserved field is not zero: {0:#010x}")]
    ReservedFlags(u32),
    /// `payload_len` in the header did not equal the actual remaining byte count.
    #[error("unit declares payload length {declared} but {actual} bytes actually follow")]
    PayloadLength {
        /// Length declared at header offset 32.
        declared: u64,
        /// Bytes actually following the 40-byte header.
        actual: u64,
    },
    /// The stored `content_hash` does not match the hash computed from the bytes.
    #[error("unit content hash {declared:#018x} does not match the computed hash {computed:#018x}")]
    ContentHash {
        /// Hash stored at header offset 24.
        declared: u64,
        /// Hash computed from the unit's own bytes.
        computed: u64,
    },
    /// A section table entry names a `kind` this build does not recognise.
    #[error("unit references unknown section kind {kind}")]
    UnknownSection {
        /// The unrecognised section kind.
        kind: u32,
    },
    /// A section table entry violates the layout rules (ordering, overlap, bounds, packing).
    #[error("section of kind {kind} has an invalid layout")]
    SectionLayout {
        /// Kind of the offending section (or the kind expected but missing, for a required
        /// section that was never encountered).
        kind: u32,
    },
    /// A count or length exceeded an allowed limit.
    #[error("{what} is {value}, which exceeds the limit {max}")]
    Limit {
        /// Name of the limited quantity.
        what: &'static str,
        /// The value that was found.
        value: u64,
        /// The maximum allowed value.
        max: u64,
    },
    /// An index referenced a position outside a valid range.
    #[error("{what} index {index} is out of range (len {len})")]
    IndexOutOfRange {
        /// Name of the indexed quantity.
        what: &'static str,
        /// The out-of-range index.
        index: u64,
        /// Length of the valid range.
        len: u64,
    },
    /// A floating-point field was NaN or infinite.
    #[error("non-finite float at byte offset {offset}")]
    NonFinite {
        /// Byte offset of the offending field.
        offset: usize,
    },
    /// A cascade depth exceeded [`SigilUnit::MAX_CASCADE_DEPTH`].
    ///
    /// Not produced by this crate's decoder (cascade depth is a spawn-time concern of
    /// `BulletPool::spawn`, not of section decoding), but kept as a valid variant of this
    /// `#[non_exhaustive]` enum for validators added by later work packages.
    #[error("cascade depth {depth} exceeds the maximum")]
    CascadeTooDeep {
        /// The offending cascade depth.
        depth: u8,
    },
    /// The bytes at `offset` are a well-formed but non-canonical encoding.
    #[error("non-canonical encoding at byte offset {offset}")]
    NonCanonical {
        /// Byte offset where the non-canonical encoding starts.
        offset: usize,
    },
}

/// A decoded, immutable Sigil unit: header identity plus the section content this crate needs.
///
/// Decoding validates the header, the section table, and the four provisional section kinds
/// listed on [`SigilUnit::from_bytes`]; sections `Transforms`, `Curves` and `Names` are only
/// checked to lie fully in-bounds and are otherwise treated as opaque bytes (their interior format
/// is not yet decided, see the module docs).
#[derive(Debug, Clone)]
pub struct SigilUnit {
    id: UnitId,
    content_hash: u64,
    bullet_types: Vec<BulletType>,
    emitter_count: u16,
    program_count: Option<u16>,
    behavior_refs: Option<Vec<BehaviorId>>,
    transforms: Option<Vec<u8>>,
    curves: Option<Vec<u8>>,
    names: Option<Vec<u8>>,
}

/// Little-endian 8-byte magic that every unit must start with.
const MAGIC: [u8; 8] = *b"GRIMSIGL";

/// Section kind: fixed-size bullet-type records (required).
const SECTION_BULLET_TYPES: u32 = 1;
/// Section kind: bullet-pattern programs (count only, provisional; §11.1).
const SECTION_PROGRAMS: u32 = 2;
/// Section kind: emitter definitions (count only, provisional; required).
const SECTION_EMITTERS: u32 = 3;
/// Section kind: opaque, undecided transform data.
const SECTION_TRANSFORMS: u32 = 4;
/// Section kind: opaque, undecided curve data.
const SECTION_CURVES: u32 = 5;
/// Section kind: behavior id references.
const SECTION_BEHAVIOR_REFS: u32 = 6;
/// Section kind: diagnostic names, hashed but never interpreted at runtime.
const SECTION_NAMES: u32 = 7;
/// Byte size of one section table entry (`kind: u32, reserved: u32, offset: u64, len: u64`).
const SECTION_ENTRY_LEN: u64 = 24;

/// Bit mask of the [`BulletFlags`] bits defined in v1; higher bits must be zero.
const BULLET_FLAGS_MASK: u8 = 0b1111;

impl SigilUnit {
    /// Magic bytes every encoded unit starts with.
    pub const MAGIC: [u8; 8] = MAGIC;
    /// Binary format version implemented by this build.
    pub const FORMAT_VERSION: u32 = 1;
    /// Fixed size of the header in bytes.
    pub const HEADER_LEN: usize = 40;
    /// Largest allowed size of a whole unit, header included.
    pub const MAX_UNIT_BYTES: usize = 8 * 1024 * 1024;
    /// Largest allowed sub-spawn cascade depth (contract §11.1/§11.3).
    pub const MAX_CASCADE_DEPTH: u8 = 3;

    /// Decodes a unit from its binary encoding, validating the header, the section table and the
    /// section kinds this crate understands.
    ///
    /// Never panics: every malformed input yields a [`UnitError`] (contract §2 rule 9).
    ///
    /// # Errors
    ///
    /// Returns a [`UnitError`] describing the first validation failure encountered.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, UnitError> {
        if bytes.len() > Self::MAX_UNIT_BYTES {
            return Err(UnitError::Limit {
                what: "unit_bytes",
                value: bytes.len() as u64,
                max: Self::MAX_UNIT_BYTES as u64,
            });
        }

        let mut header = Cursor::new(bytes, 0);
        let magic = header.read_bytes(8)?;
        if magic != Self::MAGIC {
            return Err(UnitError::BadMagic);
        }
        let format_version = header.read_u32()?;
        if format_version != Self::FORMAT_VERSION {
            return Err(UnitError::UnsupportedVersion(format_version));
        }
        let header_flags = header.read_u32()?;
        if header_flags != 0 {
            return Err(UnitError::ReservedFlags(header_flags));
        }
        let raw_id = header.read_u64()?;
        if raw_id == 0 {
            // No dedicated "invalid id" variant exists; `IndexOutOfRange` is the closest fit
            // (the id space excludes 0, so 0 is "out of range" of the one-element-wide set of
            // forbidden values). Documented in the WP1.3 report as a judgment call.
            return Err(UnitError::IndexOutOfRange {
                what: "unit_id",
                index: 0,
                len: 0,
            });
        }
        let declared_content_hash = header.read_u64()?;
        let payload_len = header.read_u64()?;
        debug_assert_eq!(
            header.pos,
            Self::HEADER_LEN,
            "header cursor must land on HEADER_LEN"
        );

        let actual_payload_len = (bytes.len() - Self::HEADER_LEN) as u64;
        if payload_len != actual_payload_len {
            return Err(UnitError::PayloadLength {
                declared: payload_len,
                actual: actual_payload_len,
            });
        }

        let mut hasher = StableHasher::new();
        hasher.write_bytes(&bytes[0..24]);
        hasher.write_bytes(&bytes[32..bytes.len()]);
        let computed_content_hash = hasher.finish();
        if computed_content_hash != declared_content_hash {
            return Err(UnitError::ContentHash {
                declared: declared_content_hash,
                computed: computed_content_hash,
            });
        }

        let payload = &bytes[Self::HEADER_LEN..];
        let sections = decode_section_table(payload)?;

        let mut bullet_types: Option<Vec<BulletType>> = None;
        let mut emitter_count: Option<u16> = None;
        let mut program_count: Option<u16> = None;
        let mut behavior_refs: Option<Vec<BehaviorId>> = None;
        let mut transforms: Option<Vec<u8>> = None;
        let mut curves: Option<Vec<u8>> = None;
        let mut names: Option<Vec<u8>> = None;

        for section in &sections {
            let start = section.offset as usize;
            let end = start + section.len as usize;
            let bytes = &payload[start..end];
            let base = Self::HEADER_LEN + start;
            match section.kind {
                SECTION_BULLET_TYPES => {
                    if bullet_types.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    bullet_types = Some(decode_bullet_types(bytes, base)?);
                }
                SECTION_PROGRAMS => {
                    if program_count.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    program_count = Some(decode_count_only(bytes, base, section.kind)?);
                }
                SECTION_EMITTERS => {
                    if emitter_count.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    emitter_count = Some(decode_count_only(bytes, base, section.kind)?);
                }
                SECTION_TRANSFORMS => {
                    if transforms.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    transforms = Some(bytes.to_vec());
                }
                SECTION_CURVES => {
                    if curves.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    curves = Some(bytes.to_vec());
                }
                SECTION_BEHAVIOR_REFS => {
                    if behavior_refs.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    behavior_refs = Some(decode_behavior_refs(bytes, base, section.kind)?);
                }
                SECTION_NAMES => {
                    if names.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    names = Some(bytes.to_vec());
                }
                other => return Err(UnitError::UnknownSection { kind: other }),
            }
        }

        let bullet_types = bullet_types.ok_or(UnitError::SectionLayout {
            kind: SECTION_BULLET_TYPES,
        })?;
        let emitter_count = emitter_count.ok_or(UnitError::SectionLayout {
            kind: SECTION_EMITTERS,
        })?;

        Ok(Self {
            id: UnitId(raw_id),
            content_hash: computed_content_hash,
            bullet_types,
            emitter_count,
            program_count,
            behavior_refs,
            transforms,
            curves,
            names,
        })
    }

    /// Encodes this unit back into its canonical binary form.
    ///
    /// This is the reference encoder: for every value produced by [`SigilUnit::from_bytes`],
    /// `to_bytes(from_bytes(b)?) == b` holds, because the decoder rejects every non-canonical
    /// encoding up front (contract §11.1).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut contents: Vec<(u32, Vec<u8>)> = Vec::new();
        contents.push((
            SECTION_BULLET_TYPES,
            encode_bullet_types(&self.bullet_types),
        ));
        if let Some(count) = self.program_count {
            contents.push((SECTION_PROGRAMS, encode_count_only(count)));
        }
        contents.push((SECTION_EMITTERS, encode_count_only(self.emitter_count)));
        if let Some(transforms) = &self.transforms {
            contents.push((SECTION_TRANSFORMS, transforms.clone()));
        }
        if let Some(curves) = &self.curves {
            contents.push((SECTION_CURVES, curves.clone()));
        }
        if let Some(refs) = &self.behavior_refs {
            contents.push((SECTION_BEHAVIOR_REFS, encode_behavior_refs(refs)));
        }
        if let Some(names) = &self.names {
            contents.push((SECTION_NAMES, names.clone()));
        }

        let section_count = contents.len() as u32;
        let table_len = 4u64 + u64::from(section_count) * SECTION_ENTRY_LEN;

        let mut table = Vec::new();
        table.extend_from_slice(&section_count.to_le_bytes());
        let mut body = Vec::new();
        let mut offset = table_len;
        for (kind, bytes) in &contents {
            table.extend_from_slice(&kind.to_le_bytes());
            table.extend_from_slice(&0u32.to_le_bytes());
            table.extend_from_slice(&offset.to_le_bytes());
            table.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            offset += bytes.len() as u64;
            body.extend_from_slice(bytes);
        }
        let mut payload = table;
        payload.extend_from_slice(&body);

        let mut out = Vec::with_capacity(Self::HEADER_LEN + payload.len());
        out.extend_from_slice(&Self::MAGIC);
        out.extend_from_slice(&Self::FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&self.id.0.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes()); // content_hash placeholder
        out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        out.extend_from_slice(&payload);

        let mut hasher = StableHasher::new();
        hasher.write_bytes(&out[0..24]);
        hasher.write_bytes(&out[32..out.len()]);
        let hash = hasher.finish();
        out[24..32].copy_from_slice(&hash.to_le_bytes());
        out
    }

    /// Identity of this unit.
    #[must_use]
    pub const fn id(&self) -> UnitId {
        self.id
    }

    /// Content hash from header offset 24, validated during decoding.
    #[must_use]
    pub const fn content_hash(&self) -> u64 {
        self.content_hash
    }

    /// Bullet-type records of this unit.
    #[must_use]
    pub fn bullet_types(&self) -> &[BulletType] {
        &self.bullet_types
    }

    /// Number of emitters defined by this unit.
    #[must_use]
    pub const fn emitter_count(&self) -> u16 {
        self.emitter_count
    }

    /// Number of bullet-pattern programs defined by this unit (`0` if the unit has no `Programs`
    /// section).
    #[must_use]
    pub fn program_count(&self) -> u16 {
        self.program_count.unwrap_or(0)
    }

    /// Behavior ids referenced by this unit, in encoding order.
    #[must_use]
    pub fn behavior_refs(&self) -> &[BehaviorId] {
        self.behavior_refs.as_deref().unwrap_or(&[])
    }
}

impl PartialEq for SigilUnit {
    /// Two units are equal iff their canonical byte encodings are equal.
    ///
    /// A field-by-field comparison is not possible with `#[derive(PartialEq)]` because
    /// `BulletType` carries `f32` fields, and the contract still asks for `Eq`. Every decoded
    /// unit is guaranteed NaN-free (the decoder rejects non-finite floats), so byte equality is a
    /// sound, simple stand-in for structural equality.
    fn eq(&self, other: &Self) -> bool {
        self.to_bytes() == other.to_bytes()
    }
}

impl Eq for SigilUnit {}

/// One validated entry of a section table, offsets relative to the start of the payload.
struct SectionEntry {
    kind: u32,
    offset: u64,
    len: u64,
}

/// Decodes and fully validates the section table of `payload` (contract §11.1): ascending
/// `(kind, offset)` order, no overlap, fully in-bounds, no gap before, between or after sections
/// (see the `NonCanonical` note below).
fn decode_section_table(payload: &[u8]) -> Result<Vec<SectionEntry>, UnitError> {
    let mut cursor = Cursor::new(payload, SigilUnit::HEADER_LEN);
    let section_count = cursor.read_u32()?;
    let table_end = 4u64
        .checked_add(
            u64::from(section_count)
                .checked_mul(SECTION_ENTRY_LEN)
                .ok_or(UnitError::Limit {
                    what: "section_count",
                    value: u64::from(section_count),
                    max: u64::MAX,
                })?,
        )
        .ok_or(UnitError::Limit {
            what: "section_count",
            value: u64::from(section_count),
            max: u64::MAX,
        })?;

    // No `Vec::with_capacity(section_count as usize)`: `section_count` is attacker-controlled and
    // must not size an allocation before every entry has been read against the actual remaining
    // bytes (contract §11.1 / §2 rule 9). `read_u32`/`read_u64` below bound each read against
    // `payload.len()`, so a bogus huge `section_count` fails with `UnexpectedEnd` long before any
    // large allocation would happen.
    let mut sections = Vec::new();
    for _ in 0..section_count {
        let kind = cursor.read_u32()?;
        let reserved = cursor.read_u32()?;
        if reserved != 0 {
            return Err(UnitError::ReservedFlags(reserved));
        }
        let offset = cursor.read_u64()?;
        let len = cursor.read_u64()?;

        if !(1..=7).contains(&kind) {
            return Err(UnitError::UnknownSection { kind });
        }
        if offset < table_end {
            return Err(UnitError::SectionLayout { kind });
        }
        let end = offset
            .checked_add(len)
            .ok_or(UnitError::SectionLayout { kind })?;
        if end > payload.len() as u64 {
            return Err(UnitError::SectionLayout { kind });
        }
        sections.push(SectionEntry { kind, offset, len });
    }

    for pair in sections.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if (b.kind, b.offset) <= (a.kind, a.offset) {
            return Err(UnitError::SectionLayout { kind: b.kind });
        }
    }

    // Global overlap check independent of table order: sort a copy by byte offset and verify
    // every section starts no earlier than the previous one ends.
    let mut by_offset: Vec<&SectionEntry> = sections.iter().collect();
    by_offset.sort_by_key(|section| section.offset);
    for pair in by_offset.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b.offset < a.offset + a.len {
            return Err(UnitError::SectionLayout { kind: b.kind });
        }
    }

    // PROVISIONAL (WP1.3, pending docs/formats/sigil.md from WP4.1): the contract requires
    // sections to be ascending, non-overlapping and in-bounds, but says nothing about gaps
    // between them. To keep `to_bytes` a pure function of the decoded fields (rather than having
    // to retain raw gap bytes just to reproduce them), this decoder additionally requires the
    // payload to be tightly packed: the first section starts exactly at the end of the section
    // table, each next section starts exactly where the previous one ends, and the last section
    // ends exactly at the end of the payload. A gap is rejected as `NonCanonical`. This is a
    // contract-change candidate: a future format document could instead mandate byte-for-byte
    // gap preservation, which would need `SigilUnit` to retain the whole raw payload.
    let mut expected = table_end;
    for section in &sections {
        if section.offset != expected {
            return Err(UnitError::NonCanonical {
                offset: SigilUnit::HEADER_LEN + expected as usize,
            });
        }
        expected += section.len;
    }
    if expected != payload.len() as u64 {
        return Err(UnitError::NonCanonical {
            offset: SigilUnit::HEADER_LEN + expected as usize,
        });
    }

    Ok(sections)
}

/// PROVISIONAL (WP1.3, pending docs/formats/sigil.md from WP4.1): minimal fixed-size bullet-type
/// record layout — `count: u16` then `count` × 20-byte records (`radius: f32`,
/// `collision_radius: f32`, `lifetime_ticks: u32`, `flags: u8`, `reserved: u8`, `silhouette: u16`,
/// `palette: u16`, `palette_space: u8`, `glow: u8`).
const BULLET_TYPE_RECORD_LEN: usize = 20;

/// Decodes the provisional `BulletTypes` section content (see [`BULLET_TYPE_RECORD_LEN`]).
fn decode_bullet_types(bytes: &[u8], base: usize) -> Result<Vec<BulletType>, UnitError> {
    let mut cursor = Cursor::new(bytes, base);
    let count = cursor.read_u16()?;
    let mut result = Vec::new();
    for _ in 0..count {
        let radius = cursor.read_finite_f32()?;
        let collision_radius = cursor.read_finite_f32()?;
        let lifetime_ticks = cursor.read_u32()?;
        let flags_byte = cursor.read_u8()?;
        let reserved = cursor.read_u8()?;
        if reserved != 0 {
            return Err(UnitError::ReservedFlags(u32::from(reserved)));
        }
        if flags_byte & !BULLET_FLAGS_MASK != 0 {
            return Err(UnitError::Limit {
                what: "bullet_type.flags",
                value: u64::from(flags_byte),
                max: u64::from(BULLET_FLAGS_MASK),
            });
        }
        let silhouette = cursor.read_u16()?;
        let palette = cursor.read_u16()?;
        let palette_space = cursor.read_u8()?;
        let glow = cursor.read_u8()?;
        result.push(BulletType {
            visual: BulletVisual {
                silhouette,
                palette,
                palette_space,
                glow,
            },
            radius,
            collision_radius,
            lifetime_ticks,
            flags: BulletFlags(flags_byte),
        });
    }
    if cursor.pos != bytes.len() {
        return Err(UnitError::SectionLayout {
            kind: SECTION_BULLET_TYPES,
        });
    }
    Ok(result)
}

/// Encodes bullet-type records back into the provisional layout (inverse of
/// [`decode_bullet_types`]).
fn encode_bullet_types(bullet_types: &[BulletType]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + bullet_types.len() * BULLET_TYPE_RECORD_LEN);
    out.extend_from_slice(&(bullet_types.len() as u16).to_le_bytes());
    for bullet_type in bullet_types {
        out.extend_from_slice(&bullet_type.radius.to_le_bytes());
        out.extend_from_slice(&bullet_type.collision_radius.to_le_bytes());
        out.extend_from_slice(&bullet_type.lifetime_ticks.to_le_bytes());
        out.push(bullet_type.flags.0);
        out.push(0); // reserved
        out.extend_from_slice(&bullet_type.visual.silhouette.to_le_bytes());
        out.extend_from_slice(&bullet_type.visual.palette.to_le_bytes());
        out.push(bullet_type.visual.palette_space);
        out.push(bullet_type.visual.glow);
    }
    out
}

/// PROVISIONAL (WP1.3, pending docs/formats/sigil.md from WP4.1): decodes a section whose entire
/// content is a `count: u16` (used for `Programs` and `Emitters`, kinds 2 and 3, since their real
/// per-item content is not yet specified).
fn decode_count_only(bytes: &[u8], base: usize, kind: u32) -> Result<u16, UnitError> {
    let mut cursor = Cursor::new(bytes, base);
    let count = cursor.read_u16()?;
    if cursor.pos != bytes.len() {
        return Err(UnitError::SectionLayout { kind });
    }
    Ok(count)
}

/// Encodes a count-only section (inverse of [`decode_count_only`]).
fn encode_count_only(count: u16) -> Vec<u8> {
    count.to_le_bytes().to_vec()
}

/// Decodes the `BehaviorRefs` section: `count: u16` then `count` × `u32` behavior ids.
fn decode_behavior_refs(
    bytes: &[u8],
    base: usize,
    kind: u32,
) -> Result<Vec<BehaviorId>, UnitError> {
    let mut cursor = Cursor::new(bytes, base);
    let count = cursor.read_u16()?;
    let mut refs = Vec::new();
    for _ in 0..count {
        refs.push(BehaviorId(cursor.read_u32()?));
    }
    if cursor.pos != bytes.len() {
        return Err(UnitError::SectionLayout { kind });
    }
    Ok(refs)
}

/// Encodes behavior references (inverse of [`decode_behavior_refs`]).
fn encode_behavior_refs(refs: &[BehaviorId]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + refs.len() * 4);
    out.extend_from_slice(&(refs.len() as u16).to_le_bytes());
    for id in refs {
        out.extend_from_slice(&id.0.to_le_bytes());
    }
    out
}

/// Negative-zero bit pattern, used by [`Cursor::read_finite_f32`] to reject the non-canonical
/// `-0.0` encoding (contract §11.1: "`-0.0` instead of `+0.0`" is one of the listed non-canonical
/// cases).
const NEG_ZERO_BITS: u32 = 0x8000_0000;

/// Bounds-checked little-endian byte reader that tracks its absolute offset within the whole unit
/// buffer, so every error it produces carries a meaningful, unit-wide byte offset.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    base: usize,
}

impl<'a> Cursor<'a> {
    /// Creates a cursor over `bytes`, which starts at absolute offset `base` within the unit.
    const fn new(bytes: &'a [u8], base: usize) -> Self {
        Self {
            bytes,
            pos: 0,
            base,
        }
    }

    /// Absolute byte offset of the cursor's current position within the whole unit.
    const fn abs(&self) -> usize {
        self.base + self.pos
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], UnitError> {
        let available = self.bytes.len() - self.pos;
        if available < len {
            return Err(UnitError::UnexpectedEnd {
                offset: self.abs(),
                needed: len,
                available,
            });
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, UnitError> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, UnitError> {
        // `read_bytes` guarantees a slice of exactly 2 bytes here.
        let b = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, UnitError> {
        // `read_bytes` guarantees a slice of exactly 4 bytes here.
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, UnitError> {
        // `read_bytes` guarantees a slice of exactly 8 bytes here.
        let b = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// Reads an `f32`, rejecting non-finite values (`NonFinite`) and the non-canonical `-0.0`
    /// encoding (`NonCanonical`).
    fn read_finite_f32(&mut self) -> Result<f32, UnitError> {
        let offset = self.abs();
        let bits = self.read_u32()?;
        let value = f32::from_bits(bits);
        if !value.is_finite() {
            return Err(UnitError::NonFinite { offset });
        }
        if bits == NEG_ZERO_BITS {
            return Err(UnitError::NonCanonical { offset });
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::test_support::{build_unit_bytes, plain_bullet_type};

    fn one_bullet_type() -> Vec<BulletType> {
        vec![plain_bullet_type()]
    }

    #[test]
    fn decodes_a_minimal_valid_unit() {
        let bytes = build_unit_bytes(7, &one_bullet_type(), 2, None, None);
        let unit = SigilUnit::from_bytes(&bytes).expect("must decode");
        assert_eq!(unit.id(), UnitId(7));
        assert_eq!(unit.bullet_types().len(), 1);
        assert_eq!(unit.emitter_count(), 2);
        assert_eq!(unit.program_count(), 0);
        assert!(unit.behavior_refs().is_empty());
    }

    #[test]
    fn decodes_programs_and_behavior_refs_when_present() {
        let refs = [BehaviorId(5), BehaviorId(9)];
        let bytes = build_unit_bytes(1, &one_bullet_type(), 1, Some(3), Some(&refs));
        let unit = SigilUnit::from_bytes(&bytes).expect("must decode");
        assert_eq!(unit.program_count(), 3);
        assert_eq!(unit.behavior_refs(), &refs);
    }

    #[test]
    fn to_bytes_from_bytes_round_trips() {
        let refs = [BehaviorId(1)];
        let bytes = build_unit_bytes(42, &one_bullet_type(), 4, Some(1), Some(&refs));
        let unit = SigilUnit::from_bytes(&bytes).expect("must decode");
        assert_eq!(unit.to_bytes(), bytes);
        let decoded_again = SigilUnit::from_bytes(&unit.to_bytes()).expect("re-decode");
        assert_eq!(decoded_again, unit);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = build_unit_bytes(1, &one_bullet_type(), 0, None, None);
        bytes[0] = b'X';
        assert_eq!(SigilUnit::from_bytes(&bytes), Err(UnitError::BadMagic));
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut bytes = build_unit_bytes(1, &one_bullet_type(), 0, None, None);
        bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
        // format_version feeds into content_hash, so the stored hash is now stale too; the
        // version check must fire before the hash check to produce `UnsupportedVersion`.
        assert_eq!(
            SigilUnit::from_bytes(&bytes),
            Err(UnitError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn rejects_reserved_header_flags() {
        let mut bytes = build_unit_bytes(1, &one_bullet_type(), 0, None, None);
        bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            SigilUnit::from_bytes(&bytes),
            Err(UnitError::ReservedFlags(1))
        );
    }

    #[test]
    fn rejects_zero_unit_id() {
        let bytes = build_unit_bytes(0, &one_bullet_type(), 0, None, None);
        assert_eq!(
            SigilUnit::from_bytes(&bytes),
            Err(UnitError::IndexOutOfRange {
                what: "unit_id",
                index: 0,
                len: 0,
            })
        );
    }

    #[test]
    fn rejects_wrong_payload_length() {
        let mut bytes = build_unit_bytes(1, &one_bullet_type(), 0, None, None);
        bytes.push(0); // trailing byte not accounted for by payload_len
        assert_eq!(
            SigilUnit::from_bytes(&bytes),
            Err(UnitError::PayloadLength {
                declared: (bytes.len() - 1 - SigilUnit::HEADER_LEN) as u64,
                actual: (bytes.len() - SigilUnit::HEADER_LEN) as u64,
            })
        );
    }

    #[test]
    fn rejects_wrong_content_hash() {
        let mut bytes = build_unit_bytes(1, &one_bullet_type(), 0, None, None);
        bytes[24..32].copy_from_slice(&0xdead_beef_u64.to_le_bytes());
        match SigilUnit::from_bytes(&bytes) {
            Err(UnitError::ContentHash { declared, .. }) => assert_eq!(declared, 0xdead_beef),
            other => panic!("expected ContentHash error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_section_kind() {
        let mut bytes = build_unit_bytes(1, &one_bullet_type(), 0, None, None);
        // Section table starts right after the 40-byte header + 4-byte section_count; the first
        // entry's `kind` field is the next 4 bytes.
        let kind_offset = SigilUnit::HEADER_LEN + 4;
        bytes[kind_offset..kind_offset + 4].copy_from_slice(&99u32.to_le_bytes());
        recompute_hash(&mut bytes);
        assert_eq!(
            SigilUnit::from_bytes(&bytes),
            Err(UnitError::UnknownSection { kind: 99 })
        );
    }

    #[test]
    fn rejects_missing_required_bullet_types_section() {
        // Build a unit with only the Emitters section by hand.
        let table_end = 4u64 + SECTION_ENTRY_LEN;
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_le_bytes()); // section_count
        payload.extend_from_slice(&SECTION_EMITTERS.to_le_bytes()); // kind
        payload.extend_from_slice(&0u32.to_le_bytes()); // reserved
        payload.extend_from_slice(&table_end.to_le_bytes()); // offset
        payload.extend_from_slice(&2u64.to_le_bytes()); // len
        payload.extend_from_slice(&5u16.to_le_bytes()); // emitter count content

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&SigilUnit::MAGIC);
        bytes.extend_from_slice(&SigilUnit::FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&payload);
        recompute_hash(&mut bytes);

        assert_eq!(
            SigilUnit::from_bytes(&bytes),
            Err(UnitError::SectionLayout {
                kind: SECTION_BULLET_TYPES
            })
        );
    }

    #[test]
    fn rejects_non_finite_bullet_radius() {
        let mut bytes = build_unit_bytes(1, &one_bullet_type(), 0, None, None);
        let radius_offset = bullet_type_record_offset(&bytes);
        bytes[radius_offset..radius_offset + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        recompute_hash(&mut bytes);
        match SigilUnit::from_bytes(&bytes) {
            Err(UnitError::NonFinite { .. }) => {}
            other => panic!("expected NonFinite, got {other:?}"),
        }
    }

    #[test]
    fn rejects_negative_zero_radius_as_non_canonical() {
        let mut bytes = build_unit_bytes(1, &one_bullet_type(), 0, None, None);
        let radius_offset = bullet_type_record_offset(&bytes);
        bytes[radius_offset..radius_offset + 4].copy_from_slice(&(-0.0f32).to_le_bytes());
        recompute_hash(&mut bytes);
        match SigilUnit::from_bytes(&bytes) {
            Err(UnitError::NonCanonical { .. }) => {}
            other => panic!("expected NonCanonical, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_bullet_flags_bits() {
        let mut bytes = build_unit_bytes(1, &one_bullet_type(), 0, None, None);
        // flags byte sits right after the 12-byte radius/collision_radius/lifetime_ticks prefix.
        let flags_offset = bullet_type_record_offset(&bytes) + 12;
        bytes[flags_offset] = 0b1111_0000;
        recompute_hash(&mut bytes);
        match SigilUnit::from_bytes(&bytes) {
            Err(UnitError::Limit {
                what: "bullet_type.flags",
                ..
            }) => {}
            other => panic!("expected a bullet_type.flags Limit error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_gap_between_sections_as_non_canonical() {
        let mut bytes = build_unit_bytes(1, &one_bullet_type(), 1, None, None);

        let section_count = u32::from_le_bytes(
            bytes[SigilUnit::HEADER_LEN..SigilUnit::HEADER_LEN + 4]
                .try_into()
                .unwrap(),
        );
        let table_end =
            SigilUnit::HEADER_LEN + 4 + section_count as usize * SECTION_ENTRY_LEN as usize;

        // Insert one padding byte right after the section table (shifting only the section
        // *content*, not the table entries, since the insertion point is exactly at the
        // boundary between them), then bump every entry's recorded `offset` by one to match.
        // The first section's offset is now `table_end + 1`, a one-byte leading gap.
        bytes.insert(table_end, 0xAA);
        for i in 0..section_count as usize {
            let offset_field = SigilUnit::HEADER_LEN + 4 + i * SECTION_ENTRY_LEN as usize + 8;
            let old = u64::from_le_bytes(bytes[offset_field..offset_field + 8].try_into().unwrap());
            bytes[offset_field..offset_field + 8].copy_from_slice(&(old + 1).to_le_bytes());
        }
        let old_payload_len = u64::from_le_bytes(bytes[32..40].try_into().unwrap());
        bytes[32..40].copy_from_slice(&(old_payload_len + 1).to_le_bytes());
        recompute_hash(&mut bytes);

        match SigilUnit::from_bytes(&bytes) {
            Err(UnitError::NonCanonical { .. }) => {}
            other => panic!("expected NonCanonical (gap), got {other:?}"),
        }
    }

    /// Offset of the first bullet type record's `radius` field within `bytes` (right after the
    /// bullet-types section's own `count: u16`).
    fn bullet_type_record_offset(bytes: &[u8]) -> usize {
        let section_count = u32::from_le_bytes(
            bytes[SigilUnit::HEADER_LEN..SigilUnit::HEADER_LEN + 4]
                .try_into()
                .unwrap(),
        );
        assert!(section_count >= 1);
        // The bullet-types section is always the first table entry (kind 1 is the smallest kind).
        let entry_start = SigilUnit::HEADER_LEN + 4;
        let offset_field = entry_start + 8;
        let section_offset =
            u64::from_le_bytes(bytes[offset_field..offset_field + 8].try_into().unwrap()) as usize;
        SigilUnit::HEADER_LEN + section_offset + 2
    }

    /// Recomputes and overwrites the `content_hash` field after a test has mutated other bytes.
    fn recompute_hash(bytes: &mut [u8]) {
        let mut hasher = StableHasher::new();
        hasher.write_bytes(&bytes[0..24]);
        let len = bytes.len();
        hasher.write_bytes(&bytes[32..len]);
        let hash = hasher.finish();
        bytes[24..32].copy_from_slice(&hash.to_le_bytes());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// The decoder never panics on arbitrary bytes (contract §2 rule 9).
        #[test]
        fn from_bytes_never_panics_on_arbitrary_input(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
            let _ = SigilUnit::from_bytes(&bytes);
        }

        /// Truncating or mutating a single byte of a valid unit's bytes never panics: the result
        /// is always `Ok` or `Err`.
        #[test]
        fn from_bytes_never_panics_on_mutated_valid_unit(
            mutate_index in 0usize..256,
            mutate_value in any::<u8>(),
            truncate_to in 0usize..256,
        ) {
            let mut bytes = build_unit_bytes(1, &one_bullet_type(), 3, Some(2), Some(&[BehaviorId(1)]));
            if mutate_index < bytes.len() {
                bytes[mutate_index] = mutate_value;
            }
            let _ = SigilUnit::from_bytes(&bytes);

            let mut truncated = build_unit_bytes(1, &one_bullet_type(), 3, Some(2), Some(&[BehaviorId(1)]));
            let cut = truncate_to.min(truncated.len());
            truncated.truncate(cut);
            let _ = SigilUnit::from_bytes(&truncated);
        }
    }
}
