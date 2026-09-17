//! Binary format `SigilUnit` v1 (contract §11.1): header, section table, and decode/encode of
//! every section kind this crate understands.
//!
//! Finalised in Plan 0002 WP4.2 (`grimoire_sigilc` is the compiler that produces these bytes;
//! see `docs/formats/sigil.md`'s binary section for the authoritative layout tables):
//!
//! - `BulletTypes` (kind 1) and `BehaviorRefs` (kind 6) were already final as of WP1.3 and are
//!   unchanged here.
//! - `Programs` (kind 2, block + modifier stack), `Emitters` (kind 3) and `Curves` (kind 5,
//!   keyframes for the `speed_curve` modifier) get their real v1 record layouts in this work
//!   package, replacing the WP1.3 placeholder that stored only a bare count.
//! - `Transforms` (kind 4) gets its interior layout in Plan 0002 WP5.2, once the interpreter knew
//!   what it has to read: one record per bullet type that has a runtime script, carrying the
//!   type's optional `BulletBehavior` binding (id plus constant parameters) and its ordered
//!   transform list (`reverse`, `change_type`, `burst`, `become_emitter`, each with a `time`,
//!   `distance` or `event` trigger). The behavior binding lives here because the `BulletTypes`
//!   record is frozen at 20 bytes and `BehaviorRefs` (kind 6) is only the set of referenced ids;
//!   this closes the WP5.1 gap that a behavior could not be called from a tick phase. The decoder
//!   now also re-checks the cascade depth and the absence of sub-emitter recursion (contract
//!   §11.1) on this section's graph. `Names` (kind 7, diagnostics only) stays opaque bytes.

use std::fmt;

use grimoire_core::{StableHash, StableHasher};

use crate::behavior::BehaviorId;
use crate::content::{BulletFlags, BulletType, BulletVisual};

/// Stable identifier of a compiled Sigil unit.
///
/// `0` is reserved and never a valid id: [`SigilUnit::from_bytes`] rejects it. `grimoire_sigilc`
/// derives real ids from a unit's canonical content path; this crate only defines the type.
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
    /// Produced since Plan 0002 WP5.2 when a primary emitter's bullet type reaches, through the
    /// `Transforms` section's `burst`/`become_emitter` edges, a depth beyond the maximum; a
    /// recursion through those edges always does. `depth` is the first depth that is too deep.
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
    /// An enum tag this build does not recognise (a block kind or a modifier kind).
    ///
    /// Additive (contract §2 rule 13, `#[non_exhaustive]`): introduced in Plan 0002 WP4.2 for the
    /// `Programs` section's block- and modifier-kind tags, reused rather than adding one
    /// dedicated variant per tag kind (the same "reuse an existing shape" clarification already
    /// applied to `IndexOutOfRange`/`SectionLayout` in WP1.3, contract §11.1).
    #[error("unknown {what} tag {tag}")]
    UnknownTag {
        /// What kind of tag this is (e.g. `"block.kind"`, `"modifier.kind"`).
        what: &'static str,
        /// The unrecognised tag value.
        tag: u32,
    },
}

/// A decoded, immutable Sigil unit: header identity plus every section's content.
#[derive(Debug, Clone)]
pub struct SigilUnit {
    id: UnitId,
    content_hash: u64,
    bullet_types: Vec<BulletType>,
    programs: Vec<ProgramRecord>,
    emitters: Vec<EmitterRecord>,
    behavior_refs: Option<Vec<BehaviorId>>,
    scripts: Vec<BulletScript>,
    curves: Vec<CurveRecord>,
    names: Option<Vec<u8>>,
}

/// Little-endian 8-byte magic that every unit must start with.
const MAGIC: [u8; 8] = *b"GRIMSIGL";

/// Section kind: fixed-size bullet-type records (required).
const SECTION_BULLET_TYPES: u32 = 1;
/// Section kind: bullet-pattern programs (block + modifier stack).
const SECTION_PROGRAMS: u32 = 2;
/// Section kind: emitter definitions (required).
const SECTION_EMITTERS: u32 = 3;
/// Section kind: per-bullet-type runtime scripts, i.e. behavior binding and transform list
/// (module docs, `docs/formats/sigil.md` §10.9).
const SECTION_TRANSFORMS: u32 = 4;
/// Section kind: keyframe curves referenced by the `speed_curve` modifier.
const SECTION_CURVES: u32 = 5;
/// Section kind: behavior id references.
const SECTION_BEHAVIOR_REFS: u32 = 6;
/// Section kind: diagnostic names, hashed but never interpreted at runtime.
const SECTION_NAMES: u32 = 7;
/// Byte size of one section table entry (`kind: u32, reserved: u32, offset: u64, len: u64`).
const SECTION_ENTRY_LEN: u64 = 24;

/// Bit mask of the [`BulletFlags`] bits defined in v1; higher bits must be zero.
const BULLET_FLAGS_MASK: u8 = 0b1111;

/// A shot-placement block (`docs/formats/sigil.md`'s `Programs` section table): a kind tag plus a
/// fixed, generic parameter slate. The decoder validates only the generic shape (a known `kind`,
/// every float finite and canonical); which slots a given `kind` actually means is a
/// `grimoire_sigilc` concern (contract §11.1: "was der Compiler zusichert"), kept out of this
/// crate on purpose so a new block kind's parameter *meaning* never needs a decoder change, only
/// a new `kind` tag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BlockDef {
    pub(crate) kind: u8,
    pub(crate) count: u16,
    pub(crate) params: [f32; 6],
    pub(crate) seed_hash: u32,
}

/// One entry of a [`ProgramRecord`]'s modifier stack, in written order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ModifierDef {
    pub(crate) kind: u8,
    pub(crate) flag: u8,
    pub(crate) extra: u16,
    pub(crate) params: [f32; 3],
}

/// One `Programs`-section record: a placement block plus its ordered modifier stack.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProgramRecord {
    pub(crate) block: BlockDef,
    pub(crate) modifiers: Vec<ModifierDef>,
}

/// One `Emitters`-section record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EmitterRecord {
    pub(crate) bullet_type: u16,
    /// Index into [`SigilUnit::programs`]; [`EmitterRecord::NO_PROGRAM`] means "spawn with the
    /// bare bullet type, no block/modifier stack".
    pub(crate) program: u16,
    pub(crate) role: u8,
    pub(crate) delay_ticks: u32,
    /// [`EmitterRecord::FOREVER`] means "repeat without an upper bound".
    pub(crate) repeat: u32,
    pub(crate) interval_ticks: u32,
    pub(crate) speed: f32,
    pub(crate) offset_x: f32,
    pub(crate) offset_y: f32,
}

impl EmitterRecord {
    /// Sentinel `program` value meaning "no program".
    pub(crate) const NO_PROGRAM: u16 = u16::MAX;
    /// Sentinel `repeat` value meaning "repeat without an upper bound", interpreted by the
    /// `sigil.emit` phase (WP5.1).
    pub(crate) const FOREVER: u32 = u32::MAX;
}

/// One `Curves`-section record: a keyframe list for the `speed_curve` modifier.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CurveRecord {
    pub(crate) keys: Vec<(u32, f32)>,
}

/// When a [`TransformDef`] fires (`docs/formats/sigil.md` §10.9, contract §11.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Trigger {
    /// In the update in which the bullet's age since it took its current type reaches this many
    /// ticks (at least the first update).
    Time(u32),
    /// In the update in which the distance travelled since the bullet took its current type
    /// reaches this many units (a value `<= 0` fires in the first update).
    Distance(f32),
    /// In every update of a tick in which this event id was raised (`crate::EventId`).
    Event(u32),
}

/// One transform of a bullet type's ordered transform list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TransformDef {
    /// One of the [`TransformKind`] tags.
    pub(crate) kind: u8,
    pub(crate) trigger: Trigger,
    /// `change_type`/`burst`: bullet-type index; `become_emitter`: emitter index; `reverse`: `0`.
    pub(crate) target: u16,
    /// `burst`: index into [`SigilUnit::programs`] (zero-based); every other kind:
    /// [`EmitterRecord::NO_PROGRAM`].
    pub(crate) program: u16,
    /// `burst`: base speed of the sub-bullets; every other kind: `+0.0`.
    pub(crate) speed: f32,
}

/// A bullet type's `BulletBehavior` binding: the registered id plus the constant parameters handed
/// to it as `BehaviorInput::params` (contract §11.5).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BehaviorBinding {
    pub(crate) id: BehaviorId,
    pub(crate) params: Vec<f32>,
}

/// One `Transforms`-section record: the runtime script of one bullet type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BulletScript {
    pub(crate) bullet_type: u16,
    pub(crate) behavior: Option<BehaviorBinding>,
    pub(crate) transforms: Vec<TransformDef>,
}

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

    /// Decodes a unit from its binary encoding, validating the header, the section table and
    /// every section kind.
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
            // Contract §11.1 (clarified on review of PR #3): reuses `IndexOutOfRange` rather than
            // a dedicated "invalid id" variant (the id space excludes 0, so 0 is "out of range" of
            // the one-element-wide set of forbidden values).
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
        let mut programs: Option<Vec<ProgramRecord>> = None;
        let mut emitters: Option<Vec<EmitterRecord>> = None;
        let mut behavior_refs: Option<Vec<BehaviorId>> = None;
        let mut scripts: Option<Vec<BulletScript>> = None;
        let mut curves: Option<Vec<CurveRecord>> = None;
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
                    if programs.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    programs = Some(decode_programs(bytes, base)?);
                }
                SECTION_EMITTERS => {
                    if emitters.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    emitters = Some(decode_emitters(bytes, base)?);
                }
                SECTION_TRANSFORMS => {
                    if scripts.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    scripts = Some(decode_scripts(bytes, base)?);
                }
                SECTION_CURVES => {
                    if curves.is_some() {
                        return Err(UnitError::SectionLayout { kind: section.kind });
                    }
                    curves = Some(decode_curves(bytes, base)?);
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
        let emitters = emitters.ok_or(UnitError::SectionLayout {
            kind: SECTION_EMITTERS,
        })?;
        let programs = programs.unwrap_or_default();
        let curves = curves.unwrap_or_default();
        let scripts = scripts.unwrap_or_default();

        // Structural re-validation the compiler already guaranteed (contract §11.1): every
        // `EmitterRecord::program`/`bullet_type` index lies in range.
        for emitter in &emitters {
            if emitter.bullet_type as usize >= bullet_types.len() {
                return Err(UnitError::IndexOutOfRange {
                    what: "emitter.bullet_type",
                    index: u64::from(emitter.bullet_type),
                    len: bullet_types.len() as u64,
                });
            }
            if emitter.program != EmitterRecord::NO_PROGRAM
                && emitter.program as usize >= programs.len()
            {
                return Err(UnitError::IndexOutOfRange {
                    what: "emitter.program",
                    index: u64::from(emitter.program),
                    len: programs.len() as u64,
                });
            }
        }
        for program in &programs {
            for modifier in &program.modifiers {
                if modifier.kind == ModifierKind::SPEED_CURVE
                    && modifier.extra as usize >= curves.len()
                {
                    return Err(UnitError::IndexOutOfRange {
                        what: "modifier.speed_curve.curve",
                        index: u64::from(modifier.extra),
                        len: curves.len() as u64,
                    });
                }
            }
        }
        validate_scripts(
            &scripts,
            &bullet_types,
            &emitters,
            &programs,
            behavior_refs.as_deref().unwrap_or(&[]),
        )?;

        Ok(Self {
            id: UnitId(raw_id),
            content_hash: computed_content_hash,
            bullet_types,
            programs,
            emitters,
            behavior_refs,
            scripts,
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
        if !self.programs.is_empty() {
            contents.push((SECTION_PROGRAMS, encode_programs(&self.programs)));
        }
        contents.push((SECTION_EMITTERS, encode_emitters(&self.emitters)));
        if !self.scripts.is_empty() {
            contents.push((SECTION_TRANSFORMS, encode_scripts(&self.scripts)));
        }
        if !self.curves.is_empty() {
            contents.push((SECTION_CURVES, encode_curves(&self.curves)));
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
    pub fn emitter_count(&self) -> u16 {
        self.emitters.len() as u16
    }

    /// Number of bullet-pattern programs defined by this unit (`0` if the unit has no `Programs`
    /// section).
    #[must_use]
    pub fn program_count(&self) -> u16 {
        self.programs.len() as u16
    }

    /// Behavior ids referenced by this unit, in encoding order.
    #[must_use]
    pub fn behavior_refs(&self) -> &[BehaviorId] {
        self.behavior_refs.as_deref().unwrap_or(&[])
    }

    /// Bullet-pattern programs of this unit (block + modifier stack), in encoding order.
    ///
    /// `pub(crate)`: the interpreter (WP5.1, `crate::blocks`/`crate::runtime`) is this crate's
    /// only other module and reads a program's block/modifier shape directly rather than through
    /// a re-exported wrapper type, matching how `bullet_types()` already exposes `BulletType`
    /// directly. Not public: `BlockDef`/`ModifierDef` are decoder-internal shapes whose only
    /// documented meaning lives in `docs/formats/sigil.md` §10.4, not in this crate's public API.
    pub(crate) fn programs(&self) -> &[ProgramRecord] {
        &self.programs
    }

    /// Emitter definitions of this unit, in encoding order (contract §11.1's `role`, `delay_ticks`,
    /// `repeat`, `interval_ticks`, `speed`, `offset_x`/`offset_y`; `docs/formats/sigil.md` §10.5).
    pub(crate) fn emitters(&self) -> &[EmitterRecord] {
        &self.emitters
    }

    /// `speed_curve` keyframe curves of this unit, in encoding order (`docs/formats/sigil.md`
    /// §10.6); indexed by a `speed_curve` modifier's `extra` field.
    pub(crate) fn curves(&self) -> &[CurveRecord] {
        &self.curves
    }

    /// Runtime scripts (behavior binding and transform list) of this unit's bullet types,
    /// ascending by bullet type; a type without an entry has neither (`docs/formats/sigil.md`
    /// §10.9).
    pub(crate) fn scripts(&self) -> &[BulletScript] {
        &self.scripts
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

    // Contract §11.1 (clarified on review of PR #3): sections are ascending, non-overlapping,
    // in-bounds, AND tightly packed — the first section starts exactly at the end of the section
    // table, each next section starts exactly where the previous one ends, and the last section
    // ends exactly at the end of the payload. A gap is rejected as `NonCanonical`, keeping
    // `to_bytes` a pure function of the decoded fields without the decoder having to retain raw
    // gap bytes just to reproduce them. `docs/formats/sigil.md` (WP4.1) restates this in detail.
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

/// Fixed-size bullet-type record layout (contract §11.2, final since WP1.3): `count: u16` then
/// `count` × 20-byte records (`radius: f32`, `collision_radius: f32`, `lifetime_ticks: u32`,
/// `flags: u8`, `reserved: u8`, `silhouette: u16`, `palette: u16`, `palette_space: u8`, `glow: u8`).
const BULLET_TYPE_RECORD_LEN: usize = 20;

/// Decodes the `BulletTypes` section content (see [`BULLET_TYPE_RECORD_LEN`]).
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

/// Encodes bullet-type records back into the v1 layout (inverse of [`decode_bullet_types`]).
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

/// Known [`BlockDef::kind`] tags (`docs/formats/sigil.md`'s `Programs` section table).
///
/// This module's own decoder only ever compares a wire `kind` byte against `1..=MAX`; it never
/// needs to distinguish one tag from another. `crate::blocks` (WP5.1) is what actually interprets
/// a `BlockDef`'s generic `params` per kind and reads these constants directly, so they are the
/// one authoritative copy of the tag numbers documented in `docs/formats/sigil.md`.
pub(crate) struct BlockKind;
impl BlockKind {
    pub(crate) const RING: u8 = 1;
    pub(crate) const SPIRAL: u8 = 2;
    pub(crate) const FAN: u8 = 3;
    pub(crate) const AIMED: u8 = 4;
    pub(crate) const WAVE: u8 = 5;
    pub(crate) const LINE: u8 = 6;
    pub(crate) const SCATTER: u8 = 7;
    const MAX: u8 = 7;
}

/// Known [`ModifierDef::kind`] tags (`docs/formats/sigil.md`'s `Programs` section table). Read by
/// the decoder (this module, generic shape only) and by the interpreter (`crate::runtime`,
/// WP5.1), which is what actually gives each tag its per-tick meaning.
pub(crate) struct ModifierKind;
impl ModifierKind {
    pub(crate) const ACCELERATE: u8 = 1;
    pub(crate) const SINE_OFFSET: u8 = 2;
    pub(crate) const ROTATE: u8 = 3;
    pub(crate) const MIRROR: u8 = 4;
    pub(crate) const SPEED_CURVE: u8 = 5;
    pub(crate) const CURVE: u8 = 6;
    const MAX: u8 = 6;
}

fn decode_block_def(cursor: &mut Cursor<'_>) -> Result<BlockDef, UnitError> {
    let kind = cursor.read_u8()?;
    if kind == 0 || kind > BlockKind::MAX {
        return Err(UnitError::UnknownTag {
            what: "block.kind",
            tag: u32::from(kind),
        });
    }
    let reserved = cursor.read_u8()?;
    if reserved != 0 {
        return Err(UnitError::ReservedFlags(u32::from(reserved)));
    }
    let count = cursor.read_u16()?;
    let mut params = [0.0f32; 6];
    for slot in &mut params {
        *slot = cursor.read_finite_f32()?;
    }
    let seed_hash = cursor.read_u32()?;
    Ok(BlockDef {
        kind,
        count,
        params,
        seed_hash,
    })
}

fn encode_block_def(out: &mut Vec<u8>, block: &BlockDef) {
    out.push(block.kind);
    out.push(0); // reserved
    out.extend_from_slice(&block.count.to_le_bytes());
    for slot in &block.params {
        out.extend_from_slice(&slot.to_le_bytes());
    }
    out.extend_from_slice(&block.seed_hash.to_le_bytes());
}

fn decode_modifier_def(cursor: &mut Cursor<'_>) -> Result<ModifierDef, UnitError> {
    let kind = cursor.read_u8()?;
    if kind == 0 || kind > ModifierKind::MAX {
        return Err(UnitError::UnknownTag {
            what: "modifier.kind",
            tag: u32::from(kind),
        });
    }
    let flag = cursor.read_u8()?;
    let extra = cursor.read_u16()?;
    let mut params = [0.0f32; 3];
    for slot in &mut params {
        *slot = cursor.read_finite_f32()?;
    }
    Ok(ModifierDef {
        kind,
        flag,
        extra,
        params,
    })
}

fn encode_modifier_def(out: &mut Vec<u8>, modifier: &ModifierDef) {
    out.push(modifier.kind);
    out.push(modifier.flag);
    out.extend_from_slice(&modifier.extra.to_le_bytes());
    for slot in &modifier.params {
        out.extend_from_slice(&slot.to_le_bytes());
    }
}

/// Decodes the `Programs` section: `count: u16`, then `count` records of a [`BlockDef`] followed
/// by `modifier_count: u16` [`ModifierDef`]s.
fn decode_programs(bytes: &[u8], base: usize) -> Result<Vec<ProgramRecord>, UnitError> {
    let mut cursor = Cursor::new(bytes, base);
    let count = read_section_count(&mut cursor)?;
    let mut programs = Vec::new();
    for _ in 0..count {
        let block = decode_block_def(&mut cursor)?;
        let modifier_count = cursor.read_u16()?;
        let mut modifiers = Vec::new();
        for _ in 0..modifier_count {
            modifiers.push(decode_modifier_def(&mut cursor)?);
        }
        programs.push(ProgramRecord { block, modifiers });
    }
    if cursor.pos != bytes.len() {
        return Err(UnitError::SectionLayout {
            kind: SECTION_PROGRAMS,
        });
    }
    Ok(programs)
}

fn encode_programs(programs: &[ProgramRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(programs.len() as u16).to_le_bytes());
    for program in programs {
        encode_block_def(&mut out, &program.block);
        out.extend_from_slice(&(program.modifiers.len() as u16).to_le_bytes());
        for modifier in &program.modifiers {
            encode_modifier_def(&mut out, modifier);
        }
    }
    out
}

/// Reads the `count: u16` of an optional section whose records the encoder only ever writes when
/// there is at least one (`Programs`, `Curves`, `Transforms`). A present section with count `0`
/// decodes to the same value as an absent one, so it is rejected as `NonCanonical`; otherwise
/// `to_bytes(from_bytes(b)) == b` would not hold (contract §11.1).
fn read_section_count(cursor: &mut Cursor<'_>) -> Result<u16, UnitError> {
    let offset = cursor.abs();
    let count = cursor.read_u16()?;
    if count == 0 {
        return Err(UnitError::NonCanonical { offset });
    }
    Ok(count)
}

/// Byte size of one [`EmitterRecord`].
const EMITTER_RECORD_LEN: usize = 30;

/// Decodes the `Emitters` section: `count: u16` then `count` × [`EmitterRecord`].
fn decode_emitters(bytes: &[u8], base: usize) -> Result<Vec<EmitterRecord>, UnitError> {
    let mut cursor = Cursor::new(bytes, base);
    let count = cursor.read_u16()?;
    let mut emitters = Vec::new();
    for _ in 0..count {
        let bullet_type = cursor.read_u16()?;
        let program = cursor.read_u16()?;
        let role = cursor.read_u8()?;
        if role > 1 {
            return Err(UnitError::Limit {
                what: "emitter.role",
                value: u64::from(role),
                max: 1,
            });
        }
        let reserved = cursor.read_u8()?;
        if reserved != 0 {
            return Err(UnitError::ReservedFlags(u32::from(reserved)));
        }
        let delay_ticks = cursor.read_u32()?;
        let repeat = cursor.read_u32()?;
        let interval_ticks = cursor.read_u32()?;
        let speed = cursor.read_finite_f32()?;
        let offset_x = cursor.read_finite_f32()?;
        let offset_y = cursor.read_finite_f32()?;
        emitters.push(EmitterRecord {
            bullet_type,
            program,
            role,
            delay_ticks,
            repeat,
            interval_ticks,
            speed,
            offset_x,
            offset_y,
        });
    }
    if cursor.pos != bytes.len() {
        return Err(UnitError::SectionLayout {
            kind: SECTION_EMITTERS,
        });
    }
    Ok(emitters)
}

fn encode_emitters(emitters: &[EmitterRecord]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + emitters.len() * EMITTER_RECORD_LEN);
    out.extend_from_slice(&(emitters.len() as u16).to_le_bytes());
    for emitter in emitters {
        out.extend_from_slice(&emitter.bullet_type.to_le_bytes());
        out.extend_from_slice(&emitter.program.to_le_bytes());
        out.push(emitter.role);
        out.push(0); // reserved
        out.extend_from_slice(&emitter.delay_ticks.to_le_bytes());
        out.extend_from_slice(&emitter.repeat.to_le_bytes());
        out.extend_from_slice(&emitter.interval_ticks.to_le_bytes());
        out.extend_from_slice(&emitter.speed.to_le_bytes());
        out.extend_from_slice(&emitter.offset_x.to_le_bytes());
        out.extend_from_slice(&emitter.offset_y.to_le_bytes());
    }
    out
}

/// Largest number of keyframes one curve may hold. Generous for any real tempo curve (the corpus'
/// richest example uses 4) and small enough that even `MAX_UNIT_BYTES` worth of one-key curves
/// stays a bounded, quickly-rejected allocation (contract §2 rule 9).
const MAX_CURVE_KEYS: u16 = 256;

/// Decodes the `Curves` section: `count: u16`, then `count` records of `key_count: u16` followed
/// by `key_count` × (`at_ticks: u32`, `mul: f32`).
fn decode_curves(bytes: &[u8], base: usize) -> Result<Vec<CurveRecord>, UnitError> {
    let mut cursor = Cursor::new(bytes, base);
    let count = read_section_count(&mut cursor)?;
    let mut curves = Vec::new();
    for _ in 0..count {
        let key_count = cursor.read_u16()?;
        if key_count > MAX_CURVE_KEYS {
            return Err(UnitError::Limit {
                what: "curve.key_count",
                value: u64::from(key_count),
                max: u64::from(MAX_CURVE_KEYS),
            });
        }
        let mut keys = Vec::new();
        for _ in 0..key_count {
            let at_ticks = cursor.read_u32()?;
            let mul = cursor.read_finite_f32()?;
            keys.push((at_ticks, mul));
        }
        curves.push(CurveRecord { keys });
    }
    if cursor.pos != bytes.len() {
        return Err(UnitError::SectionLayout {
            kind: SECTION_CURVES,
        });
    }
    Ok(curves)
}

fn encode_curves(curves: &[CurveRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(curves.len() as u16).to_le_bytes());
    for curve in curves {
        out.extend_from_slice(&(curve.keys.len() as u16).to_le_bytes());
        for &(at_ticks, mul) in &curve.keys {
            out.extend_from_slice(&at_ticks.to_le_bytes());
            out.extend_from_slice(&mul.to_le_bytes());
        }
    }
    out
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

/// Known [`TransformDef::kind`] tags (`docs/formats/sigil.md` §10.9).
pub(crate) struct TransformKind;
impl TransformKind {
    pub(crate) const REVERSE: u8 = 1;
    pub(crate) const CHANGE_TYPE: u8 = 2;
    pub(crate) const BURST: u8 = 3;
    pub(crate) const BECOME_EMITTER: u8 = 4;
    const MAX: u8 = 4;
}

/// Known trigger tags of a [`TransformDef`] (`docs/formats/sigil.md` §10.9).
struct TriggerKind;
impl TriggerKind {
    const TIME: u8 = 1;
    const DISTANCE: u8 = 2;
    const EVENT: u8 = 3;
}

/// Largest number of constant parameters one behavior binding may carry.
pub(crate) const MAX_BEHAVIOR_PARAMS: u16 = 8;
/// Largest number of transforms one bullet type may carry.
pub(crate) const MAX_TRANSFORMS_PER_TYPE: u16 = 8;
/// Byte size of one [`TransformDef`] record.
const TRANSFORM_RECORD_LEN: usize = 24;

/// Decodes the `Transforms` section: `count: u16` (at least `1`, an empty section is not
/// canonical), then `count` script records, strictly ascending by `bullet_type`. Checks every
/// in-section rule; cross-section references are [`validate_scripts`]'s job.
fn decode_scripts(bytes: &[u8], base: usize) -> Result<Vec<BulletScript>, UnitError> {
    let mut cursor = Cursor::new(bytes, base);
    let count = read_section_count(&mut cursor)?;
    let mut scripts: Vec<BulletScript> = Vec::new();
    for _ in 0..count {
        let entry_offset = cursor.abs();
        let bullet_type = cursor.read_u16()?;
        if let Some(previous) = scripts.last()
            && bullet_type <= previous.bullet_type
        {
            return Err(UnitError::NonCanonical {
                offset: entry_offset,
            });
        }
        let has_behavior = cursor.read_u8()?;
        if has_behavior > 1 {
            return Err(UnitError::Limit {
                what: "transforms.has_behavior",
                value: u64::from(has_behavior),
                max: 1,
            });
        }
        let reserved = cursor.read_u8()?;
        if reserved != 0 {
            return Err(UnitError::ReservedFlags(u32::from(reserved)));
        }
        let behavior_offset = cursor.abs();
        let behavior = cursor.read_u32()?;
        let param_count = cursor.read_u16()?;
        if param_count > MAX_BEHAVIOR_PARAMS {
            return Err(UnitError::Limit {
                what: "transforms.param_count",
                value: u64::from(param_count),
                max: u64::from(MAX_BEHAVIOR_PARAMS),
            });
        }
        let transform_count = cursor.read_u16()?;
        if transform_count > MAX_TRANSFORMS_PER_TYPE {
            return Err(UnitError::Limit {
                what: "transforms.transform_count",
                value: u64::from(transform_count),
                max: u64::from(MAX_TRANSFORMS_PER_TYPE),
            });
        }
        if has_behavior == 0 && (behavior != 0 || param_count != 0) {
            return Err(UnitError::NonCanonical {
                offset: behavior_offset,
            });
        }
        if has_behavior == 0 && transform_count == 0 {
            return Err(UnitError::NonCanonical {
                offset: entry_offset,
            });
        }
        let mut params = Vec::new();
        for _ in 0..param_count {
            params.push(cursor.read_finite_f32()?);
        }
        let mut transforms = Vec::new();
        for _ in 0..transform_count {
            transforms.push(decode_transform(&mut cursor)?);
        }
        scripts.push(BulletScript {
            bullet_type,
            behavior: (has_behavior == 1).then_some(BehaviorBinding {
                id: BehaviorId(behavior),
                params,
            }),
            transforms,
        });
    }
    if cursor.pos != bytes.len() {
        return Err(UnitError::SectionLayout {
            kind: SECTION_TRANSFORMS,
        });
    }
    Ok(scripts)
}

/// Decodes one 24-byte [`TransformDef`], rejecting unknown tags and every non-zero field the
/// record's kind and trigger do not use (canonical encoding, contract §11.1).
fn decode_transform(cursor: &mut Cursor<'_>) -> Result<TransformDef, UnitError> {
    let record_offset = cursor.abs();
    let kind = cursor.read_u8()?;
    if kind == 0 || kind > TransformKind::MAX {
        return Err(UnitError::UnknownTag {
            what: "transform.kind",
            tag: u32::from(kind),
        });
    }
    let trigger_kind = cursor.read_u8()?;
    let reserved = cursor.read_u16()?;
    if reserved != 0 {
        return Err(UnitError::ReservedFlags(u32::from(reserved)));
    }
    let at_ticks = cursor.read_u32()?;
    let distance = cursor.read_finite_f32()?;
    let event = cursor.read_u32()?;
    let target = cursor.read_u16()?;
    let program = cursor.read_u16()?;
    let speed = cursor.read_finite_f32()?;
    debug_assert_eq!(cursor.abs() - record_offset, TRANSFORM_RECORD_LEN);

    let non_canonical = UnitError::NonCanonical {
        offset: record_offset,
    };
    let trigger = match trigger_kind {
        TriggerKind::TIME if distance == 0.0 && event == 0 => Trigger::Time(at_ticks),
        TriggerKind::DISTANCE if at_ticks == 0 && event == 0 => Trigger::Distance(distance),
        TriggerKind::EVENT if at_ticks == 0 && distance == 0.0 => Trigger::Event(event),
        TriggerKind::TIME | TriggerKind::DISTANCE | TriggerKind::EVENT => {
            return Err(non_canonical);
        }
        other => {
            return Err(UnitError::UnknownTag {
                what: "transform.trigger",
                tag: u32::from(other),
            });
        }
    };
    let is_burst = kind == TransformKind::BURST;
    if kind == TransformKind::REVERSE && target != 0 {
        return Err(non_canonical);
    }
    if !is_burst && (program != EmitterRecord::NO_PROGRAM || speed != 0.0) {
        return Err(non_canonical);
    }
    Ok(TransformDef {
        kind,
        trigger,
        target,
        program,
        speed,
    })
}

/// Encodes script records back into the `Transforms` layout (inverse of [`decode_scripts`]).
fn encode_scripts(scripts: &[BulletScript]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(scripts.len() as u16).to_le_bytes());
    for script in scripts {
        out.extend_from_slice(&script.bullet_type.to_le_bytes());
        match &script.behavior {
            Some(binding) => {
                out.push(1);
                out.push(0); // reserved
                out.extend_from_slice(&binding.id.0.to_le_bytes());
                out.extend_from_slice(&(binding.params.len() as u16).to_le_bytes());
            }
            None => {
                out.push(0);
                out.push(0); // reserved
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
            }
        }
        out.extend_from_slice(&(script.transforms.len() as u16).to_le_bytes());
        if let Some(binding) = &script.behavior {
            for param in &binding.params {
                out.extend_from_slice(&param.to_le_bytes());
            }
        }
        for transform in &script.transforms {
            let (trigger_kind, at_ticks, distance, event) = match transform.trigger {
                Trigger::Time(ticks) => (TriggerKind::TIME, ticks, 0.0f32, 0u32),
                Trigger::Distance(units) => (TriggerKind::DISTANCE, 0, units, 0),
                Trigger::Event(id) => (TriggerKind::EVENT, 0, 0.0, id),
            };
            out.push(transform.kind);
            out.push(trigger_kind);
            out.extend_from_slice(&0u16.to_le_bytes()); // reserved
            out.extend_from_slice(&at_ticks.to_le_bytes());
            out.extend_from_slice(&distance.to_le_bytes());
            out.extend_from_slice(&event.to_le_bytes());
            out.extend_from_slice(&transform.target.to_le_bytes());
            out.extend_from_slice(&transform.program.to_le_bytes());
            out.extend_from_slice(&transform.speed.to_le_bytes());
        }
    }
    out
}

/// Cross-section checks of the `Transforms` section (contract §11.1): every bullet-type, emitter
/// and program reference in range, every bound behavior id listed in `BehaviorRefs`, and — on
/// the graph whose `burst`/`become_emitter` edges add one cascade level while `change_type` keeps
/// it — no bullet type reachable from a primary emitter beyond [`SigilUnit::MAX_CASCADE_DEPTH`].
/// A recursion through a level-adding edge therefore always fails with `CascadeTooDeep`. This
/// mirrors the compiler's own walk (`grimoire_sigilc`, `SIG0018`), which also starts only at
/// primary (non-`sub`) emitters.
fn validate_scripts(
    scripts: &[BulletScript],
    bullet_types: &[BulletType],
    emitters: &[EmitterRecord],
    programs: &[ProgramRecord],
    behavior_refs: &[BehaviorId],
) -> Result<(), UnitError> {
    let type_count = bullet_types.len() as u64;
    for script in scripts {
        if u64::from(script.bullet_type) >= type_count {
            return Err(UnitError::IndexOutOfRange {
                what: "transforms.bullet_type",
                index: u64::from(script.bullet_type),
                len: type_count,
            });
        }
        if let Some(binding) = &script.behavior
            && !behavior_refs.contains(&binding.id)
        {
            return Err(UnitError::UnknownTag {
                what: "transforms.behavior",
                tag: binding.id.0,
            });
        }
        for transform in &script.transforms {
            match transform.kind {
                TransformKind::CHANGE_TYPE | TransformKind::BURST
                    if u64::from(transform.target) >= type_count =>
                {
                    return Err(UnitError::IndexOutOfRange {
                        what: "transform.bullet_type",
                        index: u64::from(transform.target),
                        len: type_count,
                    });
                }
                TransformKind::BECOME_EMITTER
                    if usize::from(transform.target) >= emitters.len() =>
                {
                    return Err(UnitError::IndexOutOfRange {
                        what: "transform.emitter",
                        index: u64::from(transform.target),
                        len: emitters.len() as u64,
                    });
                }
                _ => {}
            }
            if transform.kind == TransformKind::BURST
                && usize::from(transform.program) >= programs.len()
            {
                return Err(UnitError::IndexOutOfRange {
                    what: "transform.program",
                    index: u64::from(transform.program),
                    len: programs.len() as u64,
                });
            }
        }
    }
    if scripts.is_empty() {
        return Ok(());
    }

    // Depth walk over (bullet type, level) states. `seen[type]` is a bit set of the levels
    // already visited, so every state is expanded at most once and a `change_type` cycle (which
    // never adds a level) terminates.
    let max = SigilUnit::MAX_CASCADE_DEPTH;
    let mut seen = vec![0u8; bullet_types.len()];
    let mut stack: Vec<(u16, u8)> = emitters
        .iter()
        .filter(|emitter| emitter.role == 0)
        .map(|emitter| (emitter.bullet_type, 0))
        .collect();
    while let Some((bullet_type, level)) = stack.pop() {
        let bit = 1u8 << level;
        let slot = &mut seen[usize::from(bullet_type)];
        if *slot & bit != 0 {
            continue;
        }
        *slot |= bit;
        let Ok(position) = scripts.binary_search_by_key(&bullet_type, |s| s.bullet_type) else {
            continue;
        };
        for transform in &scripts[position].transforms {
            let next = match transform.kind {
                TransformKind::CHANGE_TYPE => (transform.target, level),
                TransformKind::BURST | TransformKind::BECOME_EMITTER => {
                    let next_level = level + 1;
                    if next_level > max {
                        return Err(UnitError::CascadeTooDeep { depth: next_level });
                    }
                    let next_type = if transform.kind == TransformKind::BURST {
                        transform.target
                    } else {
                        emitters[usize::from(transform.target)].bullet_type
                    };
                    (next_type, next_level)
                }
                _ => continue,
            };
            stack.push(next);
        }
    }
    Ok(())
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
        let mut emitters_content = Vec::new();
        emitters_content.extend_from_slice(&0u16.to_le_bytes()); // 0 emitters
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_le_bytes()); // section_count
        payload.extend_from_slice(&SECTION_EMITTERS.to_le_bytes()); // kind
        payload.extend_from_slice(&0u32.to_le_bytes()); // reserved
        payload.extend_from_slice(&table_end.to_le_bytes()); // offset
        payload.extend_from_slice(&(emitters_content.len() as u64).to_le_bytes()); // len
        payload.extend_from_slice(&emitters_content);

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

    #[test]
    fn decodes_a_program_with_a_block_and_modifier_stack() {
        let block = BlockDef {
            kind: BlockKind::RING,
            count: 24,
            params: [0.13, 0.0, 0.0, 0.0, 0.0, 0.0],
            seed_hash: 0,
        };
        let modifier = ModifierDef {
            kind: ModifierKind::ACCELERATE,
            flag: 0,
            extra: 0,
            params: [0.002, 0.2, 0.0],
        };
        let program = ProgramRecord {
            block,
            modifiers: vec![modifier],
        };
        let bytes = build_unit_bytes_with_programs(
            1,
            &one_bullet_type(),
            &[],
            std::slice::from_ref(&program),
        );
        let unit = SigilUnit::from_bytes(&bytes).expect("must decode");
        assert_eq!(unit.program_count(), 1);
        assert_eq!(unit.programs[0], program);
    }

    #[test]
    fn rejects_unknown_block_kind() {
        let block = BlockDef {
            kind: 99,
            count: 1,
            params: [0.0; 6],
            seed_hash: 0,
        };
        let program = ProgramRecord {
            block,
            modifiers: vec![],
        };
        let bytes = build_unit_bytes_with_programs(1, &one_bullet_type(), &[], &[program]);
        assert_eq!(
            SigilUnit::from_bytes(&bytes),
            Err(UnitError::UnknownTag {
                what: "block.kind",
                tag: 99,
            })
        );
    }

    #[test]
    fn rejects_emitter_bullet_type_out_of_range() {
        let emitter = EmitterRecord {
            bullet_type: 5,
            program: EmitterRecord::NO_PROGRAM,
            role: 0,
            delay_ticks: 0,
            repeat: 1,
            interval_ticks: 1,
            speed: 0.1,
            offset_x: 0.0,
            offset_y: 0.0,
        };
        let bytes = build_unit_bytes_with_emitters(1, &one_bullet_type(), &[emitter], &[]);
        assert_eq!(
            SigilUnit::from_bytes(&bytes),
            Err(UnitError::IndexOutOfRange {
                what: "emitter.bullet_type",
                index: 5,
                len: 1,
            })
        );
    }

    #[test]
    fn rejects_speed_curve_referencing_unknown_curve() {
        let block = BlockDef {
            kind: BlockKind::RING,
            count: 1,
            params: [0.0; 6],
            seed_hash: 0,
        };
        let modifier = ModifierDef {
            kind: ModifierKind::SPEED_CURVE,
            flag: 0,
            extra: 3, // no curve at index 3 -- none defined at all
            params: [0.0; 3],
        };
        let program = ProgramRecord {
            block,
            modifiers: vec![modifier],
        };
        let bytes = build_unit_bytes_with_programs(1, &one_bullet_type(), &[], &[program]);
        assert_eq!(
            SigilUnit::from_bytes(&bytes),
            Err(UnitError::IndexOutOfRange {
                what: "modifier.speed_curve.curve",
                index: 3,
                len: 0,
            })
        );
    }

    /// Hand-builds a unit with a real `Programs` section, bypassing `test_support`'s
    /// count-only helper (kept for the other, still count-only-shaped, tests above).
    fn build_unit_bytes_with_programs(
        id: u64,
        bullet_types: &[BulletType],
        curves: &[CurveRecord],
        programs: &[ProgramRecord],
    ) -> Vec<u8> {
        build_unit_bytes_full(id, bullet_types, &[], programs, curves)
    }

    fn build_unit_bytes_with_emitters(
        id: u64,
        bullet_types: &[BulletType],
        emitters: &[EmitterRecord],
        programs: &[ProgramRecord],
    ) -> Vec<u8> {
        build_unit_bytes_full(id, bullet_types, emitters, programs, &[])
    }

    fn build_unit_bytes_full(
        id: u64,
        bullet_types: &[BulletType],
        emitters: &[EmitterRecord],
        programs: &[ProgramRecord],
        curves: &[CurveRecord],
    ) -> Vec<u8> {
        let mut contents: Vec<(u32, Vec<u8>)> = Vec::new();
        contents.push((SECTION_BULLET_TYPES, encode_bullet_types(bullet_types)));
        if !programs.is_empty() {
            contents.push((SECTION_PROGRAMS, encode_programs(programs)));
        }
        contents.push((SECTION_EMITTERS, encode_emitters(emitters)));
        if !curves.is_empty() {
            contents.push((SECTION_CURVES, encode_curves(curves)));
        }
        contents.sort_by_key(|&(kind, _)| kind);

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

        let mut out = Vec::with_capacity(SigilUnit::HEADER_LEN + payload.len());
        out.extend_from_slice(&SigilUnit::MAGIC);
        out.extend_from_slice(&SigilUnit::FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        out.extend_from_slice(&payload);
        recompute_hash(&mut out);
        out
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

    // ---- `Transforms` section (kind 4, Plan 0002 WP5.2) ------------------------------------

    /// One raw 24-byte transform record, every field given explicitly so tests can break any one.
    #[allow(clippy::too_many_arguments)]
    fn raw_transform(
        kind: u8,
        trigger: u8,
        at_ticks: u32,
        distance: f32,
        event: u32,
        target: u16,
        program: u16,
        speed: f32,
    ) -> Vec<u8> {
        let mut out = vec![kind, trigger, 0, 0];
        out.extend_from_slice(&at_ticks.to_le_bytes());
        out.extend_from_slice(&distance.to_le_bytes());
        out.extend_from_slice(&event.to_le_bytes());
        out.extend_from_slice(&target.to_le_bytes());
        out.extend_from_slice(&program.to_le_bytes());
        out.extend_from_slice(&speed.to_le_bytes());
        out
    }

    fn raw_time_transform(
        kind: u8,
        at_ticks: u32,
        target: u16,
        program: u16,
        speed: f32,
    ) -> Vec<u8> {
        raw_transform(kind, 1, at_ticks, 0.0, 0, target, program, speed)
    }

    /// One raw script record.
    fn raw_script(
        bullet_type: u16,
        behavior: Option<(u32, &[f32])>,
        transforms: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut out = bullet_type.to_le_bytes().to_vec();
        let (flag, id, params): (u8, u32, &[f32]) =
            behavior.map_or((0, 0, &[]), |(id, p)| (1, id, p));
        out.push(flag);
        out.push(0);
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(params.len() as u16).to_le_bytes());
        out.extend_from_slice(&(transforms.len() as u16).to_le_bytes());
        for param in params {
            out.extend_from_slice(&param.to_le_bytes());
        }
        for transform in transforms {
            out.extend_from_slice(transform);
        }
        out
    }

    fn counted(records: &[Vec<u8>]) -> Vec<u8> {
        let mut out = (records.len() as u16).to_le_bytes().to_vec();
        for record in records {
            out.extend_from_slice(record);
        }
        out
    }

    /// A unit with four plain bullet types, one ring program, a primary emitter of type `0` and a
    /// sub emitter (index `1`) of type `1`, `BehaviorRefs` `[5]`, and `transforms` (the raw section
    /// body) as its `Transforms` section.
    fn unit_with_transforms(transforms: Vec<u8>) -> Vec<u8> {
        let bullet_types = vec![plain_bullet_type(); 4];
        let program = ProgramRecord {
            block: BlockDef {
                kind: BlockKind::RING,
                count: 2,
                params: [0.0; 6],
                seed_hash: 0,
            },
            modifiers: vec![],
        };
        let primary = EmitterRecord {
            bullet_type: 0,
            program: EmitterRecord::NO_PROGRAM,
            role: 0,
            delay_ticks: 0,
            repeat: 1,
            interval_ticks: 1,
            speed: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
        };
        let sub = EmitterRecord {
            bullet_type: 1,
            program: 0,
            role: 1,
            ..primary
        };
        let mut contents = vec![
            (SECTION_BULLET_TYPES, encode_bullet_types(&bullet_types)),
            (SECTION_PROGRAMS, encode_programs(&[program])),
            (SECTION_EMITTERS, encode_emitters(&[primary, sub])),
            (
                SECTION_BEHAVIOR_REFS,
                encode_behavior_refs(&[BehaviorId(5)]),
            ),
        ];
        contents.push((SECTION_TRANSFORMS, transforms));
        assemble_for_test(1, contents)
    }

    fn assemble_for_test(id: u64, mut contents: Vec<(u32, Vec<u8>)>) -> Vec<u8> {
        contents.sort_by_key(|&(kind, _)| kind);
        let table_len = 4u64 + contents.len() as u64 * SECTION_ENTRY_LEN;
        let mut payload = (contents.len() as u32).to_le_bytes().to_vec();
        let mut body = Vec::new();
        let mut offset = table_len;
        for (kind, bytes) in &contents {
            payload.extend_from_slice(&kind.to_le_bytes());
            payload.extend_from_slice(&0u32.to_le_bytes());
            payload.extend_from_slice(&offset.to_le_bytes());
            payload.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            offset += bytes.len() as u64;
            body.extend_from_slice(bytes);
        }
        payload.extend_from_slice(&body);
        let mut out = SigilUnit::MAGIC.to_vec();
        out.extend_from_slice(&SigilUnit::FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        out.extend_from_slice(&payload);
        recompute_hash(&mut out);
        out
    }

    fn decode_transforms(records: &[Vec<u8>]) -> Result<SigilUnit, UnitError> {
        SigilUnit::from_bytes(&unit_with_transforms(counted(records)))
    }

    #[test]
    fn decodes_and_round_trips_every_transform_and_trigger_kind() {
        let records = [
            raw_script(
                0,
                Some((5, &[0.5, -2.0])),
                &[
                    raw_transform(TransformKind::REVERSE, 3, 0, 0.0, 77, 0, 0xFFFF, 0.0),
                    raw_transform(TransformKind::CHANGE_TYPE, 2, 0, 4.5, 0, 2, 0xFFFF, 0.0),
                ],
            ),
            raw_script(
                2,
                None,
                &[
                    raw_time_transform(TransformKind::BURST, 40, 3, 0, 0.06),
                    raw_time_transform(TransformKind::BECOME_EMITTER, 50, 1, 0xFFFF, 0.0),
                ],
            ),
            raw_script(3, Some((5, &[])), &[]),
        ];
        let bytes = unit_with_transforms(counted(&records));
        let unit = SigilUnit::from_bytes(&bytes).expect("must decode");
        assert_eq!(unit.to_bytes(), bytes);
        let scripts = unit.scripts();
        assert_eq!(scripts.len(), 3);
        assert_eq!(
            scripts[0].behavior,
            Some(BehaviorBinding {
                id: BehaviorId(5),
                params: vec![0.5, -2.0],
            })
        );
        assert_eq!(scripts[0].transforms[0].trigger, Trigger::Event(77));
        assert_eq!(scripts[0].transforms[1].trigger, Trigger::Distance(4.5));
        assert_eq!(scripts[0].transforms[1].target, 2);
        assert_eq!(scripts[1].transforms[0].trigger, Trigger::Time(40));
        assert_eq!(scripts[1].transforms[0].speed, 0.06);
        assert_eq!(scripts[1].transforms[1].kind, TransformKind::BECOME_EMITTER);
        assert!(scripts[2].transforms.is_empty());
    }

    #[test]
    fn rejects_unknown_transform_and_trigger_tags() {
        let bad_kind = raw_time_transform(9, 1, 0, 0xFFFF, 0.0);
        assert_eq!(
            decode_transforms(&[raw_script(0, None, &[bad_kind])]).unwrap_err(),
            UnitError::UnknownTag {
                what: "transform.kind",
                tag: 9,
            }
        );
        let bad_trigger = raw_transform(TransformKind::REVERSE, 4, 0, 0.0, 0, 0, 0xFFFF, 0.0);
        assert_eq!(
            decode_transforms(&[raw_script(0, None, &[bad_trigger])]).unwrap_err(),
            UnitError::UnknownTag {
                what: "transform.trigger",
                tag: 4,
            }
        );
    }

    #[test]
    fn rejects_non_canonical_transform_records() {
        let cases = [
            // A time trigger with a distance, an event trigger with ticks.
            raw_transform(TransformKind::REVERSE, 1, 5, 1.0, 0, 0, 0xFFFF, 0.0),
            raw_transform(TransformKind::REVERSE, 3, 5, 0.0, 1, 0, 0xFFFF, 0.0),
            // `reverse` with a target, `change_type` with a program or a speed.
            raw_time_transform(TransformKind::REVERSE, 5, 1, 0xFFFF, 0.0),
            raw_time_transform(TransformKind::CHANGE_TYPE, 5, 1, 0, 0.0),
            raw_time_transform(TransformKind::BECOME_EMITTER, 5, 1, 0xFFFF, 0.5),
        ];
        for case in cases {
            match decode_transforms(&[raw_script(0, None, &[case])]) {
                Err(UnitError::NonCanonical { .. }) => {}
                other => panic!("expected NonCanonical, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_non_canonical_script_records() {
        let reverse = raw_time_transform(TransformKind::REVERSE, 5, 0, 0xFFFF, 0.0);
        // Out of order, duplicated, empty section, entry with nothing in it.
        let out_of_order = counted(&[
            raw_script(1, None, std::slice::from_ref(&reverse)),
            raw_script(0, None, std::slice::from_ref(&reverse)),
        ]);
        let duplicated = counted(&[
            raw_script(0, None, std::slice::from_ref(&reverse)),
            raw_script(0, None, std::slice::from_ref(&reverse)),
        ]);
        let empty = counted(&[]);
        let nothing = counted(&[raw_script(0, None, &[])]);
        let mut hidden_behavior = raw_script(0, None, std::slice::from_ref(&reverse));
        hidden_behavior[4..8].copy_from_slice(&5u32.to_le_bytes());
        for body in [
            out_of_order,
            duplicated,
            empty,
            nothing,
            counted(&[hidden_behavior]),
        ] {
            match SigilUnit::from_bytes(&unit_with_transforms(body)) {
                Err(UnitError::NonCanonical { .. }) => {}
                other => panic!("expected NonCanonical, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_script_limits() {
        let reverse = raw_time_transform(TransformKind::REVERSE, 5, 0, 0xFFFF, 0.0);
        let too_many = vec![reverse; usize::from(MAX_TRANSFORMS_PER_TYPE) + 1];
        match decode_transforms(&[raw_script(0, None, &too_many)]) {
            Err(UnitError::Limit {
                what: "transforms.transform_count",
                ..
            }) => {}
            other => panic!("expected a transform_count Limit, got {other:?}"),
        }
        let params = vec![0.0f32; usize::from(MAX_BEHAVIOR_PARAMS) + 1];
        match decode_transforms(&[raw_script(0, Some((5, &params)), &[])]) {
            Err(UnitError::Limit {
                what: "transforms.param_count",
                ..
            }) => {}
            other => panic!("expected a param_count Limit, got {other:?}"),
        }
    }

    #[test]
    fn rejects_script_references_out_of_range() {
        let reverse = raw_time_transform(TransformKind::REVERSE, 5, 0, 0xFFFF, 0.0);
        let cases: [(Vec<u8>, &str, u64, u64); 4] = [
            (
                raw_script(4, None, &[reverse]),
                "transforms.bullet_type",
                4,
                4,
            ),
            (
                raw_script(
                    0,
                    None,
                    &[raw_time_transform(
                        TransformKind::CHANGE_TYPE,
                        5,
                        4,
                        0xFFFF,
                        0.0,
                    )],
                ),
                "transform.bullet_type",
                4,
                4,
            ),
            (
                raw_script(
                    0,
                    None,
                    &[raw_time_transform(
                        TransformKind::BECOME_EMITTER,
                        5,
                        2,
                        0xFFFF,
                        0.0,
                    )],
                ),
                "transform.emitter",
                2,
                2,
            ),
            (
                raw_script(
                    0,
                    None,
                    &[raw_time_transform(TransformKind::BURST, 5, 1, 1, 0.1)],
                ),
                "transform.program",
                1,
                1,
            ),
        ];
        for (record, what, index, len) in cases {
            assert_eq!(
                decode_transforms(&[record]).unwrap_err(),
                UnitError::IndexOutOfRange { what, index, len }
            );
        }
        assert_eq!(
            decode_transforms(&[raw_script(0, Some((6, &[])), &[])]).unwrap_err(),
            UnitError::UnknownTag {
                what: "transforms.behavior",
                tag: 6,
            }
        );
    }

    #[test]
    fn rejects_cascades_deeper_than_the_maximum_from_a_primary_emitter() {
        // 0 -burst-> 1 -become_emitter(sub emitter 1, type 1)... build a straight chain instead:
        // 0 -burst-> 1 -burst-> 2 -burst-> 3 is depth 3 and legal; 3 -burst-> 0 is depth 4.
        let burst_to = |to: u16| raw_time_transform(TransformKind::BURST, 5, to, 0, 0.1);
        let legal = [
            raw_script(0, None, &[burst_to(1)]),
            raw_script(1, None, &[burst_to(2)]),
            raw_script(2, None, &[burst_to(3)]),
        ];
        assert!(decode_transforms(&legal).is_ok());

        let recursive = [
            raw_script(0, None, &[burst_to(1)]),
            raw_script(1, None, &[burst_to(2)]),
            raw_script(2, None, &[burst_to(3)]),
            raw_script(3, None, &[burst_to(0)]),
        ];
        assert_eq!(
            decode_transforms(&recursive).unwrap_err(),
            UnitError::CascadeTooDeep { depth: 4 }
        );

        // `become_emitter` adds a level too: 0 -> 1 -> 2 -> 3 via emitter 1 (type 1) and bursts.
        let via_emitter = [
            raw_script(
                0,
                None,
                &[raw_time_transform(
                    TransformKind::BECOME_EMITTER,
                    5,
                    1,
                    0xFFFF,
                    0.0,
                )],
            ),
            raw_script(1, None, &[burst_to(2)]),
            raw_script(2, None, &[burst_to(3)]),
            raw_script(3, None, &[burst_to(1)]),
        ];
        assert_eq!(
            decode_transforms(&via_emitter).unwrap_err(),
            UnitError::CascadeTooDeep { depth: 4 }
        );
    }

    #[test]
    fn accepts_type_change_cycles_and_chains_no_primary_emitter_reaches() {
        let change_to =
            |to: u16| raw_time_transform(TransformKind::CHANGE_TYPE, 5, to, 0xFFFF, 0.0);
        assert!(
            decode_transforms(&[
                raw_script(0, None, &[change_to(1)]),
                raw_script(1, None, &[change_to(0)]),
            ])
            .is_ok()
        );
        // Type 2 bursts into itself, but only the primary emitter's type 0 is a starting point.
        assert!(
            decode_transforms(&[raw_script(
                2,
                None,
                &[raw_time_transform(TransformKind::BURST, 5, 2, 0, 0.1)]
            )])
            .is_ok()
        );
    }

    #[test]
    fn rejects_empty_optional_sections_as_non_canonical() {
        let bullet_types = vec![plain_bullet_type()];
        let base = vec![
            (SECTION_BULLET_TYPES, encode_bullet_types(&bullet_types)),
            (SECTION_EMITTERS, encode_emitters(&[])),
        ];
        for kind in [SECTION_PROGRAMS, SECTION_CURVES, SECTION_TRANSFORMS] {
            let mut contents = base.clone();
            contents.push((kind, 0u16.to_le_bytes().to_vec()));
            match SigilUnit::from_bytes(&assemble_for_test(1, contents)) {
                Err(UnitError::NonCanonical { .. }) => {}
                other => panic!("section kind {kind}: expected NonCanonical, got {other:?}"),
            }
        }
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

        /// Same no-panic property, but against a unit that actually has a real `Programs`,
        /// `Emitters` and `Curves` section (the WP4.2 additions), not just the count-only shapes
        /// `build_unit_bytes` still produces for the other, pre-existing tests.
        #[test]
        fn from_bytes_never_panics_on_mutated_unit_with_programs(
            mutate_index in 0usize..512,
            mutate_value in any::<u8>(),
            truncate_to in 0usize..512,
        ) {
            let block = BlockDef { kind: BlockKind::WAVE, count: 9, params: [1.5, 6.0, 0.0, 0.0, 1.0, 0.0], seed_hash: 0 };
            let modifier = ModifierDef { kind: ModifierKind::SPEED_CURVE, flag: 1, extra: 0, params: [0.0; 3] };
            let program = ProgramRecord { block, modifiers: vec![modifier] };
            let curve = CurveRecord { keys: vec![(0, 1.0), (20, 0.5)] };
            let emitter = EmitterRecord {
                bullet_type: 0, program: 0, role: 0, delay_ticks: 0, repeat: EmitterRecord::FOREVER,
                interval_ticks: 6, speed: 0.1, offset_x: 0.0, offset_y: 0.0,
            };
            let mut bytes = build_unit_bytes_full(1, &one_bullet_type(), &[emitter], &[program], &[curve]);
            if mutate_index < bytes.len() {
                bytes[mutate_index] = mutate_value;
            }
            let _ = SigilUnit::from_bytes(&bytes);

            let cut = truncate_to.min(bytes.len());
            bytes.truncate(cut);
            let _ = SigilUnit::from_bytes(&bytes);
        }

        /// Same no-panic property against a unit with a real `Transforms` section (WP5.2),
        /// including its cascade walk.
        #[test]
        fn from_bytes_never_panics_on_mutated_unit_with_transforms(
            mutate_index in 0usize..512,
            mutate_value in any::<u8>(),
            truncate_to in 0usize..512,
        ) {
            let records = [
                raw_script(0, Some((5, &[1.0])), &[
                    raw_time_transform(TransformKind::BECOME_EMITTER, 9, 1, 0xFFFF, 0.0),
                ]),
                raw_script(1, None, &[
                    raw_transform(TransformKind::CHANGE_TYPE, 2, 0, 2.0, 0, 2, 0xFFFF, 0.0),
                ]),
                raw_script(2, None, &[
                    raw_transform(TransformKind::REVERSE, 3, 0, 0.0, 42, 0, 0xFFFF, 0.0),
                    raw_time_transform(TransformKind::BURST, 3, 3, 0, 0.5),
                ]),
            ];
            let mut bytes = unit_with_transforms(counted(&records));
            prop_assert!(SigilUnit::from_bytes(&bytes).is_ok());
            if mutate_index < bytes.len() {
                bytes[mutate_index] = mutate_value;
            }
            let _ = SigilUnit::from_bytes(&bytes);
            recompute_hash(&mut bytes);
            let _ = SigilUnit::from_bytes(&bytes);

            let cut = truncate_to.min(bytes.len());
            bytes.truncate(cut);
            let _ = SigilUnit::from_bytes(&bytes);
        }
    }
}
