//! Pure byte decoders for the P1 "Figuren in der Engine" pack payloads (shared cross-strand spec:
//! `figures/<name>/mesh/<i>`, `.../material/<i>`, `.../texture/<n>`, `.../skeleton`,
//! `figures/<name>/figure`; kinds `FNP_MESH`, `FNP_MATERIAL`, `FNP_TEXTURE_RAW`, `FNP_SKELETON`,
//! `FNP_FIGURE`). Strand A (the offline Python/Rust converter) produces these payloads and packs
//! them with `grimoire_assets::PackWriter`; this module is Strand B's side of the same layout.
//!
//! **Deliberately independent of `grimoire_assets`**: the engine crate map (§1) forbids *any* edge
//! (normal, build, or dev) from `grimoire_render` to `grimoire_assets`, so every function here
//! takes a raw payload byte slice — one pack entry's contents, *without* the pack container's own
//! header/TOC/manifest — and returns a plain, GPU-free value or a [`FigureFormatError`]. The
//! caller (the `grimoire` facade, the only crate allowed to depend on both `grimoire_assets` and
//! this crate — see `docs/architektur/crate-vertraege.md` §1) is responsible for extracting that
//! slice from a `PackReader`/`AssetSource` entry, for cross-checking a decoded mesh's joint indices
//! against its skeleton's `joint_count` ([`validate_joint_indices`], a separate step because a mesh
//! payload alone carries no skeleton context), and for calling
//! [`crate::WgpuRenderer::register_mesh`]/[`crate::WgpuRenderer::register_texture`] with the
//! result.
//!
//! **Never panics** (contract §2 rule 9, matching `grimoire_assets::PackReader::from_bytes`):
//! every declared count is checked against the remaining input and a documented upper bound
//! *before* it is used to size an allocation or index a slice. [`decode_mesh`] additionally runs
//! the decoded [`MeshData`] through [`crate::mesh::MeshData::validate`] before returning it, so an
//! index out of range, a non-finite vertex, or a skinning-weight sum outside tolerance is reported
//! exactly once, through the same [`crate::MeshError`] any other mesh source would produce —
//! rather than duplicating that arithmetic here.
//!
//! **Byte layout note on `FNP_SKELETON`** (flagged, not silently resolved — see this crate's PR
//! description): the shared spec's wording — "dann je Knochen: `parent`, `inverse_bind`, `name`...
//! **Dazu** je Knochen die Ruhepose als `translation`, `rotation`, `scale`" — reads as two
//! *separate*, sequential per-joint loops (hierarchy+name first, then the whole rest pose), not one
//! interleaved loop. [`decode_skeleton`] implements that reading. If Strand A instead interleaved
//! the two per joint, every skeleton this decoder reads will fail — loudly, as
//! [`FigureFormatError::UnexpectedEnd`] or a similar structural error, never a silent
//! misinterpretation, because the two layouts have different total lengths for any skeleton with
//! more than one joint.

use crate::mesh::{MeshData, MeshError, MeshVertex};
use crate::stage3d::AlphaMode;
use crate::texture::{TextureColorSpace, TextureData};

/// Format version every decoder in this module accepts (`kind_version`, contract §12: "Jeder
/// Eintrag beginnt mit `u32 version`").
pub const FORMAT_VERSION: u32 = 1;

/// Upper bound on `FNP_MESH`'s `vertex_count` (shared spec).
pub const MAX_MESH_VERTICES: u32 = 1_000_000;
/// Upper bound on `FNP_MESH`'s `index_count` (shared spec).
pub const MAX_MESH_INDICES: u32 = 3_000_000;
/// Upper bound on `FNP_TEXTURE_RAW`'s `width * height` (shared spec: "Produkt ≤ 64 Mio.").
pub const MAX_TEXTURE_PIXELS: u64 = 64_000_000;
/// Upper bound on `FNP_SKELETON`'s `joint_count` (shared spec), matching
/// [`crate::MAX_SKIN_JOINTS`].
pub const MAX_SKELETON_JOINTS: u32 = crate::stage3d::MAX_SKIN_JOINTS;
/// Upper bound on a skeleton joint name's UTF-8 byte length (shared spec).
pub const MAX_JOINT_NAME_LEN: usize = 63;
/// Upper bound on `FNP_FIGURE`'s `part_count`. **Not given a number by the shared spec** — chosen
/// here as a generous, documented limit for a hand-authored figure; flagged in this crate's PR
/// description as a gap the spec left open.
pub const MAX_FIGURE_PARTS: u32 = 1024;
/// Upper bound on `FNP_FIGURE`'s `texture_count`; see [`MAX_FIGURE_PARTS`]'s doc comment (same
/// spec gap).
pub const MAX_FIGURE_TEXTURES: u32 = 1024;

/// Sentinel for "no texture" in [`MaterialPayload`]'s texture slots (shared spec: a `u32` with
/// `0xFFFF_FFFF` meaning none).
const NO_TEXTURE: u32 = 0xFFFF_FFFF;

/// Failure decoding one figure pack payload (P1 "Figuren in der Engine" package). Every variant
/// reports a structural problem in foreign bytes; a malformed payload always yields one of these
/// instead of panicking (contract §2 rule 9). `#[non_exhaustive]`: new variants are additive.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FigureFormatError {
    /// Fewer bytes remain than a field or block needs.
    #[error(
        "unexpected end of payload at offset {offset}: needed {needed} bytes, {available} available"
    )]
    UnexpectedEnd {
        /// Byte offset the read started at.
        offset: usize,
        /// Bytes the read needed.
        needed: usize,
        /// Bytes actually remaining at `offset`.
        available: usize,
    },
    /// The payload's leading `u32 version` did not match [`FORMAT_VERSION`].
    #[error("unsupported format version {found} (expected {expected})")]
    UnsupportedVersion {
        /// The version this decoder understands.
        expected: u32,
        /// The version the payload declared.
        found: u32,
    },
    /// A declared count exceeds its documented upper bound.
    #[error("{what} count {count} exceeds the documented limit of {limit}")]
    CountExceedsLimit {
        /// Name of the offending count field.
        what: &'static str,
        /// The declared count.
        count: u64,
        /// The documented upper bound.
        limit: u64,
    },
    /// An index or id addresses something outside a known range.
    #[error("{what} {index} is out of range ({bound} available)")]
    IndexOutOfRange {
        /// Name of the offending index field.
        what: &'static str,
        /// The offending value.
        index: u64,
        /// The number of valid values.
        bound: u64,
    },
    /// `FNP_MESH`'s `index_count` is not a multiple of 3.
    #[error("index count {0} is not a multiple of 3")]
    IndexCountNotMultipleOfThree(u32),
    /// `FNP_TEXTURE_RAW`'s `width * height` exceeds [`MAX_TEXTURE_PIXELS`], or overflows computing
    /// it.
    #[error("texture is {width}x{height}: pixel count exceeds {MAX_TEXTURE_PIXELS} or overflows")]
    TextureTooLarge {
        /// The declared width.
        width: u32,
        /// The declared height.
        height: u32,
    },
    /// `FNP_TEXTURE_RAW`'s `width` or `height` is `0`.
    #[error("texture has zero width or height")]
    TextureZeroSize,
    /// A joint's `parent` is not `-1` (root) and not strictly less than the joint's own index, so
    /// a single forward pass cannot resolve it before its child (shared spec requirement).
    #[error(
        "joint {joint} has parent {parent}, which is neither -1 nor less than the joint's own index"
    )]
    InvalidJointParent {
        /// Index of the offending joint.
        joint: u32,
        /// The offending raw parent value.
        parent: i64,
    },
    /// A joint name's declared bytes are not valid UTF-8.
    #[error("joint {0}'s name is not valid UTF-8")]
    InvalidJointName(u32),
    /// `FNP_MATERIAL`'s `alpha_mode` byte was not `0`, `1` or `2`.
    #[error("alpha mode byte {0} is not 0 (opaque), 1 (mask) or 2 (blend)")]
    InvalidAlphaMode(u8),
    /// `FNP_TEXTURE_RAW`'s `color_space` byte was not `0` or `1`.
    #[error("colour space byte {0} is not 0 (sRGB) or 1 (linear)")]
    InvalidColorSpace(u8),
    /// Bytes remained after the payload was fully parsed.
    #[error("{0} trailing byte(s) after the payload")]
    TrailingBytes(usize),
    /// [`decode_mesh`]'s resulting [`MeshData`] failed [`crate::mesh::MeshData::validate`] (index
    /// out of range, a non-finite vertex, or a skinning-weight sum outside tolerance).
    #[error("decoded mesh is structurally invalid: {0}")]
    InvalidMesh(#[from] MeshError),
    /// [`validate_joint_indices`]: a mesh vertex's joint index is not `< joint_count` of the
    /// skeleton it was decoded against (shared spec: "jeder Knochenindex < joint_count des
    /// Skeletts").
    #[error(
        "mesh vertex {vertex} references joint {joint}, out of range for a skeleton of {joint_count} joints"
    )]
    JointIndexOutOfRange {
        /// Index of the offending vertex.
        vertex: u32,
        /// The offending joint index.
        joint: u16,
        /// The skeleton's joint count.
        joint_count: u32,
    },
}

/// Bounds-checked little-endian cursor over one payload byte slice. Every read returns
/// [`FigureFormatError::UnexpectedEnd`] instead of panicking when fewer bytes remain than
/// requested (contract §2 rule 9); mirrors the shape of `grimoire_assets::pack::Cursor` (private
/// to that crate, and this crate may not depend on it — see this module's doc comment), rebuilt
/// here for this payload format alone.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], FigureFormatError> {
        match self.pos.checked_add(len) {
            Some(end) if end <= self.bytes.len() => {
                let slice = &self.bytes[self.pos..end];
                self.pos = end;
                Ok(slice)
            }
            _ => Err(FigureFormatError::UnexpectedEnd {
                offset: self.pos,
                needed: len,
                available: self.remaining(),
            }),
        }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], FigureFormatError> {
        let slice = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, FigureFormatError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, FigureFormatError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, FigureFormatError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, FigureFormatError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, FigureFormatError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn f32(&mut self) -> Result<f32, FigureFormatError> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    fn f32_array<const N: usize>(&mut self) -> Result<[f32; N], FigureFormatError> {
        let mut out = [0.0f32; N];
        for slot in &mut out {
            *slot = self.f32()?;
        }
        Ok(out)
    }

    /// Reads a `u16 len` count-prefixed UTF-8 string, at most `max_len` bytes, without allocating
    /// anything before the declared length is checked (contract §2 rule 9).
    fn short_string(&mut self, max_len: usize, joint: u32) -> Result<String, FigureFormatError> {
        let len = usize::from(self.u8()?);
        if len > max_len {
            return Err(FigureFormatError::CountExceedsLimit {
                what: "joint name length",
                count: len as u64,
                limit: max_len as u64,
            });
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| FigureFormatError::InvalidJointName(joint))
    }

    fn expect_version(&mut self) -> Result<(), FigureFormatError> {
        let version = self.u32()?;
        if version != FORMAT_VERSION {
            return Err(FigureFormatError::UnsupportedVersion {
                expected: FORMAT_VERSION,
                found: version,
            });
        }
        Ok(())
    }

    fn expect_exhausted(&self) -> Result<(), FigureFormatError> {
        if self.remaining() != 0 {
            return Err(FigureFormatError::TrailingBytes(self.remaining()));
        }
        Ok(())
    }
}

/// Checks `count` (already read) against `limit`, before it is used to size any allocation
/// (contract §2 rule 9).
fn check_count(what: &'static str, count: u32, limit: u32) -> Result<u32, FigureFormatError> {
    if count > limit {
        return Err(FigureFormatError::CountExceedsLimit {
            what,
            count: u64::from(count),
            limit: u64::from(limit),
        });
    }
    Ok(count)
}

/// Decodes an `FNP_MESH` payload (shared spec, `figures/<name>/mesh/<i>`) into a [`MeshData`].
///
/// Checks `vertex_count` (`<= MAX_MESH_VERTICES`) and `index_count` (`<= MAX_MESH_INDICES`, a
/// multiple of 3) against the shared spec's documented upper bounds *before* allocating either
/// vector, then reads exactly that many vertices and indices. The result is run through
/// [`MeshData::validate`] before being returned, so an out-of-range index, a non-finite vertex, or
/// a skinning-weight sum outside `1e-3` tolerance is reported as [`FigureFormatError::InvalidMesh`]
/// rather than silently accepted or checked twice.
///
/// Does **not** check [`MeshVertex::joints`] against a skeleton's `joint_count` — a mesh payload
/// alone carries no skeleton context; see [`validate_joint_indices`] for that cross-check, run
/// separately once the mesh's skeleton is known.
///
/// # Errors
/// See [`FigureFormatError`]; never panics, for any input.
pub fn decode_mesh(bytes: &[u8]) -> Result<MeshData, FigureFormatError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect_version()?;

    let vertex_count = check_count("vertex_count", cursor.u32()?, MAX_MESH_VERTICES)?;
    let index_count = check_count("index_count", cursor.u32()?, MAX_MESH_INDICES)?;
    if !index_count.is_multiple_of(3) {
        return Err(FigureFormatError::IndexCountNotMultipleOfThree(index_count));
    }

    let mut vertices = Vec::with_capacity(vertex_count as usize);
    for _ in 0..vertex_count {
        let position = cursor.f32_array::<3>()?;
        let normal = cursor.f32_array::<3>()?;
        let uv = cursor.f32_array::<2>()?;
        let joints = [cursor.u16()?, cursor.u16()?, cursor.u16()?, cursor.u16()?];
        let weights = cursor.f32_array::<4>()?;
        vertices.push(MeshVertex {
            position,
            normal,
            uv,
            joints,
            weights,
        });
    }

    let mut indices = Vec::with_capacity(index_count as usize);
    for _ in 0..index_count {
        indices.push(cursor.u32()?);
    }
    cursor.expect_exhausted()?;

    let mesh = MeshData { vertices, indices };
    mesh.validate()?;
    Ok(mesh)
}

/// Checks every vertex of `mesh` against `joint_count` (shared spec: "jeder Knochenindex <
/// joint_count des Skeletts"), a separate step from [`decode_mesh`] because a mesh payload alone
/// carries no skeleton context — the caller (the `grimoire` facade) runs this once it has decoded
/// the mesh's `FNP_FIGURE`-referenced `FNP_SKELETON` and knows its joint count.
///
/// # Errors
/// [`FigureFormatError::JointIndexOutOfRange`] for the first offending vertex found; never panics.
pub fn validate_joint_indices(mesh: &MeshData, joint_count: u32) -> Result<(), FigureFormatError> {
    for (index, vertex) in mesh.vertices.iter().enumerate() {
        for &joint in &vertex.joints {
            if u32::from(joint) >= joint_count {
                return Err(FigureFormatError::JointIndexOutOfRange {
                    // `index < mesh.vertices.len() <= MAX_MESH_VERTICES`, well within u32.
                    vertex: u32::try_from(index).unwrap_or(u32::MAX),
                    joint,
                    joint_count,
                });
            }
        }
    }
    Ok(())
}

/// One decoded `FNP_MATERIAL` payload (shared spec, `figures/<name>/material/<i>`): the same
/// value shape as [`crate::PbrMaterial`], except the texture slots are indices into the owning
/// figure's texture list ([`FigureManifest::texture_ids`]), not yet resolved
/// [`crate::TextureHandle`]s — the caller resolves them after registering the figure's textures in
/// order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialPayload {
    /// Linear RGBA base colour factor.
    pub base_color_factor: [f32; 4],
    /// Metalness factor.
    pub metallic_factor: f32,
    /// Perceptual roughness factor.
    pub roughness_factor: f32,
    /// Linear RGB emissive factor.
    pub emissive_factor: [f32; 3],
    /// Alpha coverage mode.
    pub alpha_mode: AlphaMode,
    /// Index into the figure's texture list for the base colour texture, or `None`.
    pub base_color_texture_index: Option<u32>,
    /// Index into the figure's texture list for the normal texture, or `None`.
    pub normal_texture_index: Option<u32>,
    /// Index into the figure's texture list for the occlusion-roughness-metallic texture, or
    /// `None`.
    pub occlusion_roughness_metallic_texture_index: Option<u32>,
}

fn optional_texture_index(cursor: &mut Cursor<'_>) -> Result<Option<u32>, FigureFormatError> {
    let raw = cursor.u32()?;
    Ok(if raw == NO_TEXTURE { None } else { Some(raw) })
}

/// Decodes an `FNP_MATERIAL` payload into a [`MaterialPayload`].
///
/// # Errors
/// See [`FigureFormatError`]; never panics, for any input.
pub fn decode_material(bytes: &[u8]) -> Result<MaterialPayload, FigureFormatError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect_version()?;

    let base_color_factor = cursor.f32_array::<4>()?;
    let metallic_factor = cursor.f32()?;
    let roughness_factor = cursor.f32()?;
    let emissive_factor = cursor.f32_array::<3>()?;
    let alpha_mode_byte = cursor.u8()?;
    let alpha_cutoff = cursor.f32()?;
    let alpha_mode = match alpha_mode_byte {
        0 => AlphaMode::Opaque,
        1 => AlphaMode::Mask {
            cutoff: alpha_cutoff,
        },
        2 => AlphaMode::Blend,
        other => return Err(FigureFormatError::InvalidAlphaMode(other)),
    };
    let base_color_texture_index = optional_texture_index(&mut cursor)?;
    let normal_texture_index = optional_texture_index(&mut cursor)?;
    let occlusion_roughness_metallic_texture_index = optional_texture_index(&mut cursor)?;
    cursor.expect_exhausted()?;

    Ok(MaterialPayload {
        base_color_factor,
        metallic_factor,
        roughness_factor,
        emissive_factor,
        alpha_mode,
        base_color_texture_index,
        normal_texture_index,
        occlusion_roughness_metallic_texture_index,
    })
}

/// Decodes an `FNP_TEXTURE_RAW` payload (shared spec, `figures/<name>/texture/<n>`) into a
/// [`TextureData`].
///
/// Checks `width * height` (`<= MAX_TEXTURE_PIXELS`, and the multiplication itself for overflow)
/// *before* allocating the pixel buffer.
///
/// # Errors
/// See [`FigureFormatError`]; never panics, for any input.
pub fn decode_texture_raw(bytes: &[u8]) -> Result<TextureData, FigureFormatError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect_version()?;

    let width = cursor.u32()?;
    let height = cursor.u32()?;
    if width == 0 || height == 0 {
        return Err(FigureFormatError::TextureZeroSize);
    }
    let pixel_count = u64::from(width)
        .checked_mul(u64::from(height))
        .filter(|&count| count <= MAX_TEXTURE_PIXELS)
        .ok_or(FigureFormatError::TextureTooLarge { width, height })?;
    let byte_len = pixel_count
        .checked_mul(4)
        .and_then(|len| usize::try_from(len).ok())
        .ok_or(FigureFormatError::TextureTooLarge { width, height })?;

    let color_space_byte = cursor.u8()?;
    let color_space = match color_space_byte {
        0 => TextureColorSpace::Srgb,
        1 => TextureColorSpace::Linear,
        other => return Err(FigureFormatError::InvalidColorSpace(other)),
    };
    let pixels = cursor.take(byte_len)?.to_vec();
    cursor.expect_exhausted()?;

    Ok(TextureData {
        width,
        height,
        pixels,
        color_space,
    })
}

/// One joint of a decoded [`SkeletonData`] (shared spec `FNP_SKELETON`).
#[derive(Debug, Clone, PartialEq)]
pub struct JointData {
    /// Parent joint index, `None` for the root. Always `< ` this joint's own index when present
    /// (shared spec: "damit eine einzige Vorwärtsschleife genügt"), checked by [`decode_skeleton`].
    pub parent: Option<u32>,
    /// Column-major inverse bind matrix.
    pub inverse_bind: [[f32; 4]; 4],
    /// Diagnostic-only joint name (shared spec: "nur zur Diagnose").
    pub name: String,
    /// Rest-pose translation.
    pub translation: [f32; 3],
    /// Rest-pose rotation (quaternion, x, y, z, w).
    pub rotation: [f32; 4],
    /// Rest-pose scale.
    pub scale: [f32; 3],
}

/// A decoded `FNP_SKELETON` payload (shared spec, `figures/<name>/skeleton`): joint hierarchy,
/// inverse bind matrices and the rest pose. Computing per-frame skinning matrices from this plus an
/// animated pose is explicitly out of this package's scope (see `crate::stage3d::SkinBinding`'s
/// doc comment); this type only carries the decoded data.
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonData {
    /// Joints in parent-before-child order (shared spec's single-forward-loop invariant).
    pub joints: Vec<JointData>,
}

impl SkeletonData {
    /// Number of joints, for [`validate_joint_indices`] and [`crate::SkinBinding::joint_count`].
    #[must_use]
    pub fn joint_count(&self) -> u32 {
        // `decode_skeleton` already checked `joints.len() <= MAX_SKELETON_JOINTS` (a u32).
        u32::try_from(self.joints.len()).unwrap_or(u32::MAX)
    }
}

/// Decodes an `FNP_SKELETON` payload into a [`SkeletonData`].
///
/// See this module's doc comment for the two-separate-loops reading of the shared spec's wording
/// (hierarchy + inverse bind + name first, the whole rest pose second) this function implements.
///
/// # Errors
/// See [`FigureFormatError`]; never panics, for any input.
pub fn decode_skeleton(bytes: &[u8]) -> Result<SkeletonData, FigureFormatError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect_version()?;

    let joint_count = check_count("joint_count", cursor.u32()?, MAX_SKELETON_JOINTS)?;

    struct Hierarchy {
        parent: Option<u32>,
        inverse_bind: [[f32; 4]; 4],
        name: String,
    }
    let mut hierarchy = Vec::with_capacity(joint_count as usize);
    for joint in 0..joint_count {
        let parent_raw = cursor.i32()?;
        let parent = if parent_raw == -1 {
            None
        } else if parent_raw >= 0 && (parent_raw as u32) < joint {
            Some(parent_raw as u32)
        } else {
            return Err(FigureFormatError::InvalidJointParent {
                joint,
                parent: i64::from(parent_raw),
            });
        };
        let inverse_bind = [
            cursor.f32_array::<4>()?,
            cursor.f32_array::<4>()?,
            cursor.f32_array::<4>()?,
            cursor.f32_array::<4>()?,
        ];
        let name = cursor.short_string(MAX_JOINT_NAME_LEN, joint)?;
        hierarchy.push(Hierarchy {
            parent,
            inverse_bind,
            name,
        });
    }

    let mut joints = Vec::with_capacity(joint_count as usize);
    for entry in hierarchy {
        let translation = cursor.f32_array::<3>()?;
        let rotation = cursor.f32_array::<4>()?;
        let scale = cursor.f32_array::<3>()?;
        joints.push(JointData {
            parent: entry.parent,
            inverse_bind: entry.inverse_bind,
            name: entry.name,
            translation,
            rotation,
            scale,
        });
    }
    cursor.expect_exhausted()?;

    Ok(SkeletonData { joints })
}

/// One part of a decoded [`FigureManifest`] (shared spec `FNP_FIGURE`): one mesh/material pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FigurePart {
    /// `AssetId` (as a raw `u64`, contract §12) of this part's `FNP_MESH` entry.
    pub mesh_id: u64,
    /// `AssetId` of this part's `FNP_MATERIAL` entry.
    pub material_id: u64,
}

/// A decoded `FNP_FIGURE` payload (shared spec, `figures/<name>/figure`): which parts, textures
/// and skeleton make up one figure, plus its bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct FigureManifest {
    /// Mesh/material pairs making up this figure.
    pub parts: Vec<FigurePart>,
    /// `AssetId`s of this figure's `FNP_TEXTURE_RAW` entries, in the order
    /// [`MaterialPayload`]'s texture indices reference.
    pub texture_ids: Vec<u64>,
    /// `AssetId` of this figure's `FNP_SKELETON` entry.
    pub skeleton_id: u64,
    /// Axis-aligned bounding box minimum, model space.
    pub bounds_min: [f32; 3],
    /// Axis-aligned bounding box maximum, model space.
    pub bounds_max: [f32; 3],
}

/// Decodes an `FNP_FIGURE` payload into a [`FigureManifest`].
///
/// `part_count`/`texture_count` are checked against [`MAX_FIGURE_PARTS`]/[`MAX_FIGURE_TEXTURES`]
/// before either vector is allocated — limits this decoder documents itself (see their doc
/// comments for why: the shared spec gives no explicit number for either).
///
/// # Errors
/// See [`FigureFormatError`]; never panics, for any input.
pub fn decode_figure_manifest(bytes: &[u8]) -> Result<FigureManifest, FigureFormatError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect_version()?;

    let part_count = check_count("part_count", cursor.u32()?, MAX_FIGURE_PARTS)?;
    let mut parts = Vec::with_capacity(part_count as usize);
    for _ in 0..part_count {
        let mesh_id = cursor.u64()?;
        let material_id = cursor.u64()?;
        parts.push(FigurePart {
            mesh_id,
            material_id,
        });
    }

    let texture_count = check_count("texture_count", cursor.u32()?, MAX_FIGURE_TEXTURES)?;
    let mut texture_ids = Vec::with_capacity(texture_count as usize);
    for _ in 0..texture_count {
        texture_ids.push(cursor.u64()?);
    }

    let skeleton_id = cursor.u64()?;
    let bounds_min = cursor.f32_array::<3>()?;
    let bounds_max = cursor.f32_array::<3>()?;
    cursor.expect_exhausted()?;

    Ok(FigureManifest {
        parts,
        texture_ids,
        skeleton_id,
        bounds_min,
        bounds_max,
    })
}

const IDENTITY_MATRIX: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// One joint's local pose: translation, rotation (quaternion, x, y, z, w) and scale — the same
/// shape as [`JointData`]'s rest-pose fields, so a caller can reconstruct the rest pose itself
/// (`skeleton.joints[i]`'s own `translation`/`rotation`/`scale`, see [`compute_skin_matrices`]'s
/// doc comment) or substitute a different value for one or more joints.
///
/// **Not part of an animation system**: there is no time axis, no interpolation and no blending
/// here (this crate's PR description, "Nicht in diesem Paket: Animationskurven und Laufzyklen") —
/// just the plain data one static pose needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointPose {
    /// Local translation relative to the parent joint (or the model origin, for the root).
    pub translation: [f32; 3],
    /// Local rotation, unit quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    /// Local scale.
    pub scale: [f32; 3],
}

/// Column-major local transform `T * R * S` for `pose` (translation column last, matching this
/// crate's matrix convention, e.g. [`crate::MeshInstance::transform`]).
fn trs_matrix(pose: JointPose) -> [[f32; 4]; 4] {
    let [x, y, z, w] = pose.rotation;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    let [sx, sy, sz] = pose.scale;
    [
        [
            (1.0 - 2.0 * (yy + zz)) * sx,
            (2.0 * (xy + wz)) * sx,
            (2.0 * (xz - wy)) * sx,
            0.0,
        ],
        [
            (2.0 * (xy - wz)) * sy,
            (1.0 - 2.0 * (xx + zz)) * sy,
            (2.0 * (yz + wx)) * sy,
            0.0,
        ],
        [
            (2.0 * (xz + wy)) * sz,
            (2.0 * (yz - wx)) * sz,
            (1.0 - 2.0 * (xx + yy)) * sz,
            0.0,
        ],
        [
            pose.translation[0],
            pose.translation[1],
            pose.translation[2],
            1.0,
        ],
    ]
}

/// Column-major 4x4 matrix product `a * b`.
fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut result = [[0.0f32; 4]; 4];
    for (col, result_col) in result.iter_mut().enumerate() {
        for (row, cell) in result_col.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[k][row] * b[col][k]).sum();
        }
    }
    result
}

/// Composes one static pose into skinning matrices ready for
/// [`crate::StageFrame::joint_matrices`] (P1 skinning addendum): walks `skeleton`'s hierarchy in
/// index order (guaranteed parent-before-child by [`decode_skeleton`]) to build each joint's
/// world-space matrix from `poses[i]`'s TRS, then right-multiplies by
/// `skeleton.joints[i].inverse_bind` — the standard linear-blend-skinning composition
/// (`world_pose * inverse_bind`).
///
/// Calling this with each joint's own [`JointData::translation`]/[`JointData::rotation`]/
/// [`JointData::scale`] (i.e. `skeleton`'s own rest pose, exactly as [`decode_skeleton`] returned
/// it) always yields the identity matrix for every joint — the mathematical definition of "rest
/// pose equals bind pose" — which is what a mesh with no animation at all should use.
///
/// **Not an animation system** (see [`JointPose`]'s doc comment): one static pose in, one set of
/// matrices out.
///
/// # Errors
/// [`FigureFormatError::CountExceedsLimit`] if `poses.len() != skeleton.joints.len()`, rather than
/// panicking or silently truncating.
pub fn compute_skin_matrices(
    skeleton: &SkeletonData,
    poses: &[JointPose],
) -> Result<Vec<[[f32; 4]; 4]>, FigureFormatError> {
    if poses.len() != skeleton.joints.len() {
        return Err(FigureFormatError::CountExceedsLimit {
            what: "poses (must match skeleton.joints.len())",
            count: poses.len() as u64,
            limit: skeleton.joints.len() as u64,
        });
    }
    let mut world = vec![IDENTITY_MATRIX; skeleton.joints.len()];
    for (index, joint) in skeleton.joints.iter().enumerate() {
        let local = trs_matrix(poses[index]);
        world[index] = match joint.parent {
            Some(parent) => mat4_mul(&world[parent as usize], &local),
            None => local,
        };
    }
    Ok(skeleton
        .joints
        .iter()
        .zip(world.iter())
        .map(|(joint, world)| mat4_mul(world, &joint.inverse_bind))
        .collect())
}

/// The rest pose of `skeleton`, i.e. [`compute_skin_matrices`] called with each joint's own
/// stored TRS — always the identity matrix for every joint (see that function's doc comment), but
/// spelled out so a caller never has to build `Vec<JointPose>` from [`JointData`] by hand just to
/// draw a figure with no pose applied at all.
#[must_use]
pub fn rest_pose_skin_matrices(skeleton: &SkeletonData) -> Vec<[[f32; 4]; 4]> {
    let poses: Vec<JointPose> = skeleton
        .joints
        .iter()
        .map(|joint| JointPose {
            translation: joint.translation,
            rotation: joint.rotation,
            scale: joint.scale,
        })
        .collect();
    // `poses.len() == skeleton.joints.len()` by construction (built directly above), so this can
    // never hit the length-mismatch error `compute_skin_matrices` reports for a foreign `poses`.
    compute_skin_matrices(skeleton, &poses)
        .expect("poses has exactly one entry per skeleton.joints, built above")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Hand-built valid payloads (no PackWriter: this crate may not depend on
    //     `grimoire_assets`, see this module's doc comment) ------------------------------------

    fn push_f32(buf: &mut Vec<u8>, value: f32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }
    fn push_f32s(buf: &mut Vec<u8>, values: &[f32]) {
        for &value in values {
            push_f32(buf, value);
        }
    }
    fn push_u16(buf: &mut Vec<u8>, value: u16) {
        buf.extend_from_slice(&value.to_le_bytes());
    }
    fn push_u32(buf: &mut Vec<u8>, value: u32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }
    fn push_u64(buf: &mut Vec<u8>, value: u64) {
        buf.extend_from_slice(&value.to_le_bytes());
    }
    fn push_i32(buf: &mut Vec<u8>, value: i32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    /// One raw test vertex's fields, in payload order: position, normal, uv, joints, weights.
    type RawVertex = ([f32; 3], [f32; 3], [f32; 2], [u16; 4], [f32; 4]);

    /// Two triangles sharing an edge (4 vertices, 2 bones): vertices 0/1 bound fully to bone 0,
    /// vertices 2/3 fully to bone 1 — the exact fixture shape the showcase test also builds (this
    /// module's decoder side of it), small enough to hand-encode legibly.
    fn valid_mesh_bytes() -> Vec<u8> {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 4); // vertex_count
        push_u32(&mut buf, 6); // index_count
        let vertices: [RawVertex; 4] = [
            (
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0],
                [0, 0, 0, 0],
                [1.0, 0.0, 0.0, 0.0],
            ),
            (
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0],
                [0, 0, 0, 0],
                [1.0, 0.0, 0.0, 0.0],
            ),
            (
                [1.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0],
                [1, 0, 0, 0],
                [1.0, 0.0, 0.0, 0.0],
            ),
            (
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 1.0],
                [1, 0, 0, 0],
                [1.0, 0.0, 0.0, 0.0],
            ),
        ];
        for (position, normal, uv, joints, weights) in vertices {
            push_f32s(&mut buf, &position);
            push_f32s(&mut buf, &normal);
            push_f32s(&mut buf, &uv);
            for joint in joints {
                push_u16(&mut buf, joint);
            }
            push_f32s(&mut buf, &weights);
        }
        for index in [0u32, 1, 2, 0, 2, 3] {
            push_u32(&mut buf, index);
        }
        buf
    }

    #[test]
    fn decode_mesh_round_trips_a_valid_payload() {
        let mesh = decode_mesh(&valid_mesh_bytes()).expect("valid mesh payload");
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(mesh.vertices[2].joints, [1, 0, 0, 0]);
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn decode_mesh_rejects_wrong_version() {
        let mut bytes = valid_mesh_bytes();
        bytes[0] = 99; // low byte of the little-endian u32 version
        assert_eq!(
            decode_mesh(&bytes),
            Err(FigureFormatError::UnsupportedVersion {
                expected: FORMAT_VERSION,
                found: 99,
            })
        );
    }

    #[test]
    fn decode_mesh_rejects_vertex_count_over_the_documented_limit() {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, MAX_MESH_VERTICES + 1);
        push_u32(&mut buf, 0);
        assert_eq!(
            decode_mesh(&buf),
            Err(FigureFormatError::CountExceedsLimit {
                what: "vertex_count",
                count: u64::from(MAX_MESH_VERTICES) + 1,
                limit: u64::from(MAX_MESH_VERTICES),
            })
        );
    }

    #[test]
    fn decode_mesh_rejects_index_count_not_a_multiple_of_three() {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 0);
        push_u32(&mut buf, 4);
        assert_eq!(
            decode_mesh(&buf),
            Err(FigureFormatError::IndexCountNotMultipleOfThree(4))
        );
    }

    #[test]
    fn decode_mesh_rejects_out_of_range_index_via_validate() {
        let mut bytes = valid_mesh_bytes();
        // Overwrite the first index (right after the 4 vertices) with 99 (out of range for 4
        // vertices), little-endian u32.
        let index_offset = bytes.len() - 6 * 4;
        bytes[index_offset..index_offset + 4].copy_from_slice(&99u32.to_le_bytes());
        assert!(matches!(
            decode_mesh(&bytes),
            Err(FigureFormatError::InvalidMesh(MeshError::IndexOutOfRange {
                index: 99,
                vertex_count: 4,
            }))
        ));
    }

    #[test]
    fn decode_mesh_rejects_weight_sum_out_of_tolerance_via_validate() {
        let mut bytes = valid_mesh_bytes();
        // Vertex 0's `weights` field: header (12 bytes) + vertex 0's position/normal/uv/joints
        // (12 + 12 + 8 + 8 = 40 bytes), so `weights` starts at byte 52.
        let weights_offset = 12 + 40;
        let mut bad_weights = Vec::new();
        push_f32s(&mut bad_weights, &[0.5, 0.0, 0.0, 0.0]);
        bytes[weights_offset..weights_offset + 16].copy_from_slice(&bad_weights);
        assert!(matches!(
            decode_mesh(&bytes),
            Err(FigureFormatError::InvalidMesh(
                MeshError::WeightSumOutOfTolerance { vertex: 0, .. }
            ))
        ));
    }

    #[test]
    fn decode_mesh_rejects_trailing_bytes() {
        let mut bytes = valid_mesh_bytes();
        bytes.push(0xFF);
        assert_eq!(
            decode_mesh(&bytes),
            Err(FigureFormatError::TrailingBytes(1))
        );
    }

    #[test]
    fn decode_mesh_rejects_truncated_payload_without_panic() {
        let bytes = valid_mesh_bytes();
        for len in [0, 1, 4, 8, 12, 20] {
            assert!(matches!(
                decode_mesh(&bytes[..len]),
                Err(FigureFormatError::UnexpectedEnd { .. })
                    | Err(FigureFormatError::UnsupportedVersion { .. })
            ));
        }
    }

    #[test]
    fn validate_joint_indices_accepts_in_range_joints() {
        let mesh = decode_mesh(&valid_mesh_bytes()).unwrap();
        assert_eq!(validate_joint_indices(&mesh, 2), Ok(()));
    }

    #[test]
    fn validate_joint_indices_rejects_out_of_range_joint() {
        let mesh = decode_mesh(&valid_mesh_bytes()).unwrap();
        assert_eq!(
            validate_joint_indices(&mesh, 1),
            Err(FigureFormatError::JointIndexOutOfRange {
                vertex: 2,
                joint: 1,
                joint_count: 1,
            })
        );
    }

    fn valid_material_bytes() -> Vec<u8> {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_f32s(&mut buf, &[1.0, 0.9, 0.8, 1.0]); // base_color_factor
        push_f32(&mut buf, 0.1); // metallic
        push_f32(&mut buf, 0.6); // roughness
        push_f32s(&mut buf, &[0.0, 0.0, 0.0]); // emissive
        buf.push(1); // alpha_mode = Mask
        push_f32(&mut buf, 0.5); // alpha_cutoff
        push_u32(&mut buf, 0); // base_color_texture -> index 0
        push_u32(&mut buf, NO_TEXTURE); // normal_texture -> none
        push_u32(&mut buf, 1); // orm_texture -> index 1
        buf
    }

    #[test]
    fn decode_material_round_trips_a_valid_payload() {
        let material = decode_material(&valid_material_bytes()).expect("valid material payload");
        assert_eq!(material.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
        assert_eq!(material.base_color_texture_index, Some(0));
        assert_eq!(material.normal_texture_index, None);
        assert_eq!(material.occlusion_roughness_metallic_texture_index, Some(1));
    }

    #[test]
    fn decode_material_rejects_invalid_alpha_mode() {
        let mut bytes = valid_material_bytes();
        let alpha_mode_offset = 4 + 16 + 4 + 4 + 12;
        bytes[alpha_mode_offset] = 3;
        assert_eq!(
            decode_material(&bytes),
            Err(FigureFormatError::InvalidAlphaMode(3))
        );
    }

    fn valid_texture_bytes() -> Vec<u8> {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 2); // width
        push_u32(&mut buf, 1); // height
        buf.push(0); // sRGB
        buf.extend_from_slice(&[255, 0, 0, 255, 0, 255, 0, 255]); // 2 RGBA8 pixels
        buf
    }

    #[test]
    fn decode_texture_raw_round_trips_a_valid_payload() {
        let texture = decode_texture_raw(&valid_texture_bytes()).expect("valid texture payload");
        assert_eq!(texture.width, 2);
        assert_eq!(texture.height, 1);
        assert_eq!(texture.color_space, TextureColorSpace::Srgb);
        assert_eq!(texture.pixels.len(), 8);
        assert_eq!(texture.validate(), Ok(()));
    }

    #[test]
    fn decode_texture_raw_rejects_zero_size() {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 0);
        push_u32(&mut buf, 4);
        assert_eq!(
            decode_texture_raw(&buf),
            Err(FigureFormatError::TextureZeroSize)
        );
    }

    #[test]
    fn decode_texture_raw_rejects_product_over_the_documented_limit() {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 100_000);
        push_u32(&mut buf, 100_000); // 10 billion pixels, way past MAX_TEXTURE_PIXELS
        assert_eq!(
            decode_texture_raw(&buf),
            Err(FigureFormatError::TextureTooLarge {
                width: 100_000,
                height: 100_000,
            })
        );
    }

    #[test]
    fn decode_texture_raw_rejects_a_declared_size_it_does_not_have_bytes_for() {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 4);
        push_u32(&mut buf, 4); // declares 64 bytes of pixels
        buf.push(0);
        buf.extend_from_slice(&[0u8; 10]); // far fewer than declared
        assert!(matches!(
            decode_texture_raw(&buf),
            Err(FigureFormatError::UnexpectedEnd { .. })
        ));
    }

    fn valid_skeleton_bytes() -> Vec<u8> {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 2); // joint_count
        // Joint 0: root.
        push_i32(&mut buf, -1);
        push_f32s(&mut buf, &[1.0, 0.0, 0.0, 0.0]);
        push_f32s(&mut buf, &[0.0, 1.0, 0.0, 0.0]);
        push_f32s(&mut buf, &[0.0, 0.0, 1.0, 0.0]);
        push_f32s(&mut buf, &[0.0, 0.0, 0.0, 1.0]);
        buf.push(4); // name_len
        buf.extend_from_slice(b"root");
        // Joint 1: child of 0.
        push_i32(&mut buf, 0);
        push_f32s(&mut buf, &[1.0, 0.0, 0.0, 0.0]);
        push_f32s(&mut buf, &[0.0, 1.0, 0.0, 0.0]);
        push_f32s(&mut buf, &[0.0, 0.0, 1.0, 0.0]);
        push_f32s(&mut buf, &[0.0, 1.0, 0.0, 1.0]);
        buf.push(3); // name_len
        buf.extend_from_slice(b"arm");
        // Rest pose, one record per joint, in the same order (this module's "two loops" reading).
        push_f32s(&mut buf, &[0.0, 0.0, 0.0]); // joint 0 translation
        push_f32s(&mut buf, &[0.0, 0.0, 0.0, 1.0]); // joint 0 rotation
        push_f32s(&mut buf, &[1.0, 1.0, 1.0]); // joint 0 scale
        push_f32s(&mut buf, &[0.0, 1.0, 0.0]); // joint 1 translation
        push_f32s(&mut buf, &[0.0, 0.0, 0.0, 1.0]); // joint 1 rotation
        push_f32s(&mut buf, &[1.0, 1.0, 1.0]); // joint 1 scale
        buf
    }

    #[test]
    fn decode_skeleton_round_trips_a_valid_payload() {
        let skeleton = decode_skeleton(&valid_skeleton_bytes()).expect("valid skeleton payload");
        assert_eq!(skeleton.joint_count(), 2);
        assert_eq!(skeleton.joints[0].parent, None);
        assert_eq!(skeleton.joints[0].name, "root");
        assert_eq!(skeleton.joints[1].parent, Some(0));
        assert_eq!(skeleton.joints[1].name, "arm");
        assert_eq!(skeleton.joints[1].translation, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn decode_skeleton_rejects_joint_count_over_the_documented_limit() {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, MAX_SKELETON_JOINTS + 1);
        assert_eq!(
            decode_skeleton(&buf),
            Err(FigureFormatError::CountExceedsLimit {
                what: "joint_count",
                count: u64::from(MAX_SKELETON_JOINTS) + 1,
                limit: u64::from(MAX_SKELETON_JOINTS),
            })
        );
    }

    #[test]
    fn decode_skeleton_rejects_a_parent_not_less_than_its_own_index() {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 1);
        push_i32(&mut buf, 0); // joint 0 claims parent 0 (itself): not < 0
        push_f32s(&mut buf, &[0.0; 16]);
        buf.push(0);
        assert_eq!(
            decode_skeleton(&buf),
            Err(FigureFormatError::InvalidJointParent {
                joint: 0,
                parent: 0
            })
        );
    }

    #[test]
    fn decode_skeleton_rejects_a_forward_reference_parent() {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 2);
        push_i32(&mut buf, 1); // joint 0 claims parent 1, which does not exist yet
        push_f32s(&mut buf, &[0.0; 16]);
        buf.push(0);
        push_i32(&mut buf, -1);
        push_f32s(&mut buf, &[0.0; 16]);
        buf.push(0);
        push_f32s(&mut buf, &[0.0; 3]);
        push_f32s(&mut buf, &[0.0, 0.0, 0.0, 1.0]);
        push_f32s(&mut buf, &[1.0; 3]);
        push_f32s(&mut buf, &[0.0; 3]);
        push_f32s(&mut buf, &[0.0, 0.0, 0.0, 1.0]);
        push_f32s(&mut buf, &[1.0; 3]);
        assert_eq!(
            decode_skeleton(&buf),
            Err(FigureFormatError::InvalidJointParent {
                joint: 0,
                parent: 1
            })
        );
    }

    #[test]
    fn decode_skeleton_rejects_a_name_length_over_the_documented_limit() {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 1);
        push_i32(&mut buf, -1);
        push_f32s(&mut buf, &[0.0; 16]);
        buf.push(255); // name_len, far over MAX_JOINT_NAME_LEN
        assert_eq!(
            decode_skeleton(&buf),
            Err(FigureFormatError::CountExceedsLimit {
                what: "joint name length",
                count: 255,
                limit: MAX_JOINT_NAME_LEN as u64,
            })
        );
    }

    // --- compute_skin_matrices / rest_pose_skin_matrices --------------------------------------

    const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    /// A 2-joint skeleton: root at the origin, child offset by `[0.0, 0.0, 1.0]` — the same shape
    /// as the showcase test's fixture, small enough to hand-check the resulting matrices.
    fn two_joint_skeleton() -> SkeletonData {
        SkeletonData {
            joints: vec![
                JointData {
                    parent: None,
                    inverse_bind: IDENTITY_MATRIX,
                    name: "root".to_string(),
                    translation: [0.0, 0.0, 0.0],
                    rotation: IDENTITY_QUAT,
                    scale: [1.0, 1.0, 1.0],
                },
                JointData {
                    parent: Some(0),
                    // Inverse of the bind-pose translation by [0, 0, 1].
                    inverse_bind: [
                        [1.0, 0.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0],
                        [0.0, 0.0, -1.0, 1.0],
                    ],
                    name: "child".to_string(),
                    translation: [0.0, 0.0, 1.0],
                    rotation: IDENTITY_QUAT,
                    scale: [1.0, 1.0, 1.0],
                },
            ],
        }
    }

    fn assert_matrix_close(actual: [[f32; 4]; 4], expected: [[f32; 4]; 4], tolerance: f32) {
        for col in 0..4 {
            for row in 0..4 {
                assert!(
                    (actual[col][row] - expected[col][row]).abs() < tolerance,
                    "mismatch at [{col}][{row}]: actual {actual:?}, expected {expected:?}"
                );
            }
        }
    }

    #[test]
    fn rest_pose_skin_matrices_is_the_identity_for_every_joint() {
        let skeleton = two_joint_skeleton();
        let matrices = rest_pose_skin_matrices(&skeleton);
        assert_eq!(matrices.len(), 2);
        for matrix in matrices {
            assert_matrix_close(matrix, IDENTITY_MATRIX, 1e-6);
        }
    }

    #[test]
    fn compute_skin_matrices_rejects_a_pose_count_mismatch() {
        let skeleton = two_joint_skeleton();
        let one_pose = [JointPose {
            translation: [0.0; 3],
            rotation: IDENTITY_QUAT,
            scale: [1.0; 3],
        }];
        assert_eq!(
            compute_skin_matrices(&skeleton, &one_pose),
            Err(FigureFormatError::CountExceedsLimit {
                what: "poses (must match skeleton.joints.len())",
                count: 1,
                limit: 2,
            })
        );
    }

    #[test]
    fn compute_skin_matrices_bends_the_child_joint_around_its_own_pivot() {
        let skeleton = two_joint_skeleton();
        // Root unposed; child rotated 90 degrees about X (quaternion for a 90-degree rotation:
        // half-angle 45 degrees, sin/cos(45 deg) = sqrt(2)/2).
        let half_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
        let poses = [
            JointPose {
                translation: [0.0, 0.0, 0.0],
                rotation: IDENTITY_QUAT,
                scale: [1.0, 1.0, 1.0],
            },
            JointPose {
                translation: [0.0, 0.0, 1.0],
                rotation: [half_sqrt2, 0.0, 0.0, half_sqrt2],
                scale: [1.0, 1.0, 1.0],
            },
        ];
        let matrices = compute_skin_matrices(&skeleton, &poses).expect("matching pose count");
        // Root's skin matrix is still the identity: it was not re-posed.
        assert_matrix_close(matrices[0], IDENTITY_MATRIX, 1e-5);

        // A vertex bound fully to the child joint, at bind-space position (0, 0, 2) (one unit
        // above the joint's own pivot at bind-space Z = 1): the bend pivots it around the joint,
        // swinging it out along -Y instead of staying at (0, 0, 2), while a vertex exactly at the
        // pivot (0, 0, 1) stays put.
        let pivot = apply_matrix(matrices[1], [0.0, 0.0, 1.0]);
        assert_point_close(pivot, [0.0, 0.0, 1.0], 1e-5);
        let tip = apply_matrix(matrices[1], [0.0, 0.0, 2.0]);
        assert_point_close(tip, [0.0, -1.0, 1.0], 1e-4);
    }

    fn assert_point_close(actual: [f32; 3], expected: [f32; 3], tolerance: f32) {
        for axis in 0..3 {
            assert!(
                (actual[axis] - expected[axis]).abs() < tolerance,
                "point mismatch: actual {actual:?}, expected {expected:?}"
            );
        }
    }

    /// Applies a column-major 4x4 matrix to a point (`w = 1`), for the pose-composition tests.
    fn apply_matrix(m: [[f32; 4]; 4], point: [f32; 3]) -> [f32; 3] {
        let [x, y, z] = point;
        [
            m[0][0] * x + m[1][0] * y + m[2][0] * z + m[3][0],
            m[0][1] * x + m[1][1] * y + m[2][1] * z + m[3][1],
            m[0][2] * x + m[1][2] * y + m[2][2] * z + m[3][2],
        ]
    }

    fn valid_figure_bytes() -> Vec<u8> {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, 1); // part_count
        push_u64(&mut buf, 100);
        push_u64(&mut buf, 200);
        push_u32(&mut buf, 2); // texture_count
        push_u64(&mut buf, 300);
        push_u64(&mut buf, 301);
        push_u64(&mut buf, 400); // skeleton_id
        push_f32s(&mut buf, &[-1.0, -1.0, 0.0]);
        push_f32s(&mut buf, &[1.0, 1.0, 2.0]);
        buf
    }

    #[test]
    fn decode_figure_manifest_round_trips_a_valid_payload() {
        let figure = decode_figure_manifest(&valid_figure_bytes()).expect("valid figure payload");
        assert_eq!(
            figure.parts,
            vec![FigurePart {
                mesh_id: 100,
                material_id: 200,
            }]
        );
        assert_eq!(figure.texture_ids, vec![300, 301]);
        assert_eq!(figure.skeleton_id, 400);
        assert_eq!(figure.bounds_min, [-1.0, -1.0, 0.0]);
        assert_eq!(figure.bounds_max, [1.0, 1.0, 2.0]);
    }

    #[test]
    fn decode_figure_manifest_rejects_part_count_over_the_documented_limit() {
        let mut buf = Vec::new();
        push_u32(&mut buf, FORMAT_VERSION);
        push_u32(&mut buf, MAX_FIGURE_PARTS + 1);
        assert_eq!(
            decode_figure_manifest(&buf),
            Err(FigureFormatError::CountExceedsLimit {
                what: "part_count",
                count: u64::from(MAX_FIGURE_PARTS) + 1,
                limit: u64::from(MAX_FIGURE_PARTS),
            })
        );
    }

    // --- Property tests (contract §2 rule 9 mandate: arbitrary bytes, truncated and
    //     single-byte-mutated valid payloads must never panic) --------------------------------

    mod property_tests {
        use super::*;
        use proptest::prelude::*;

        fn all_decoders_never_panic(bytes: &[u8]) {
            let _ = decode_mesh(bytes);
            let _ = decode_material(bytes);
            let _ = decode_texture_raw(bytes);
            let _ = decode_skeleton(bytes);
            let _ = decode_figure_manifest(bytes);
        }

        proptest! {
            /// Arbitrary byte sequences, entirely unrelated to any of these formats, must never
            /// panic any decoder — only `Ok` or `Err` is acceptable.
            #[test]
            fn decoders_never_panic_on_arbitrary_bytes(
                bytes in prop::collection::vec(any::<u8>(), 0..1024)
            ) {
                all_decoders_never_panic(&bytes);
            }

            /// Flipping any single byte of a valid payload (of any of the five kinds) must never
            /// panic its decoder.
            #[test]
            fn decoders_never_panic_on_single_byte_mutation(
                which in 0usize..5,
                index in 0usize..256,
                new_byte in any::<u8>(),
            ) {
                let mut bytes = match which {
                    0 => valid_mesh_bytes(),
                    1 => valid_material_bytes(),
                    2 => valid_texture_bytes(),
                    3 => valid_skeleton_bytes(),
                    _ => valid_figure_bytes(),
                };
                if index < bytes.len() {
                    bytes[index] = new_byte;
                }
                all_decoders_never_panic(&bytes);
            }

            /// Truncating a valid payload (of any of the five kinds) to any shorter length must
            /// never panic its decoder.
            #[test]
            fn decoders_never_panic_on_truncation(which in 0usize..5, len in 0usize..256) {
                let bytes = match which {
                    0 => valid_mesh_bytes(),
                    1 => valid_material_bytes(),
                    2 => valid_texture_bytes(),
                    3 => valid_skeleton_bytes(),
                    _ => valid_figure_bytes(),
                };
                let truncated = &bytes[..len.min(bytes.len())];
                all_decoders_never_panic(truncated);
            }
        }
    }
}
