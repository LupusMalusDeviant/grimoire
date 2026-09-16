//! Assets → Render adapter (this crate's `adapters` module table, contract §9.1): decodes the P1
//! "Figuren in der Engine" pack payloads (`grimoire_render::figure_format`) from a
//! `grimoire_assets::AssetSource`/`AssetStore` and registers them with a
//! `grimoire_render::WgpuRenderer`.
//!
//! `grimoire_assets` and `grimoire_render` have no edge to each other (engine crate map §1, both
//! crates' "ausdrücklich verboten" columns list the other explicitly, every edge kind): every pure
//! byte-to-value decoder lives in `grimoire_render::figure_format` and knows nothing about packs;
//! every pack-specific lookup (paths, ids, kinds) lives here, in the one crate allowed to depend on
//! both (contract §1's crate table, row `grimoire` (Fassade)).
//!
//! **Kind numbering — a deviation from the shared spec, flagged rather than silently worked
//! around** (see this crate's PR description for the full report): the spec that Strand A (the
//! offline converter) and this module were both built against assigns `MESH = 2` and
//! `MATERIAL = 3`, describing them as "Standardwerte..., die benutzt werden, wo sie passen". But
//! `grimoire_assets::ids::AssetKind` documents `2..=5` as reserved for *future* engine kinds, and
//! `grimoire_assets::pack::classify_kind` rejects them outright — in both `PackReader` (reading)
//! and `PackWriter::add` (writing) — with `PackError::ReservedKind`. Using `2`/`3` as written is
//! therefore not possible without a contract change to `grimoire_assets` §12 (unreserving them),
//! which neither strand of this package owns. This module instead places every figure kind in the
//! `0x8000..=0xFFFF` application-defined range contract §12 already frees for exactly this use
//! ("keine Vertragsänderung nötig") — [`FNP_MATERIAL`] at `0x8004` instead of the spec's `3`, the
//! other four kinds exactly as the spec assigned them (`0x8000`-`0x8003` were already free).
//!
//! Not part of an animation system: [`load_figure`] loads geometry, materials, textures and the
//! skeleton and registers them; it never computes a pose. A caller wanting to *draw* a loaded
//! figure supplies a `Vec<[[f32; 4]; 4]>` of skinning matrices — see
//! `grimoire_render::figure_format::{rest_pose_skin_matrices, compute_skin_matrices}` — and builds
//! the `MeshInstance`/`SkinBinding`/`StageFrame::joint_matrices` entries itself; that is a
//! per-frame scene decision, not something a one-shot loader should own.

use grimoire_assets::{AssetError, AssetId, AssetKind, AssetPath, AssetStore};
use grimoire_render::figure_format::{self, FigureFormatError, MaterialPayload, SkeletonData};
use grimoire_render::{
    MeshError, MeshHandle, PbrMaterial, TextureError, TextureHandle, WgpuRenderer,
};

/// `FNP_MESH` (shared spec): geometry of one figure part, application-defined kind
/// (`0x8000..=0xFFFF`, contract §12 — no contract change needed for this range itself).
pub const FNP_MESH: AssetKind = AssetKind(0x8000);
/// `FNP_TEXTURE_RAW` (shared spec): one of a figure's textures, raw RGBA8.
pub const FNP_TEXTURE_RAW: AssetKind = AssetKind(0x8001);
/// `FNP_SKELETON` (shared spec): one figure's bone hierarchy, bind matrices and rest pose.
pub const FNP_SKELETON: AssetKind = AssetKind(0x8002);
/// `FNP_FIGURE` (shared spec): which parts, textures and skeleton make up one figure.
pub const FNP_FIGURE: AssetKind = AssetKind(0x8003);
/// `FNP_MATERIAL` — **at `0x8004`, not the shared spec's `3`**; see this module's doc comment for
/// why `3` could not be used as written.
pub const FNP_MATERIAL: AssetKind = AssetKind(0x8004);

/// Failure loading a figure (P1 "Figuren in der Engine" package). `#[non_exhaustive]`: new
/// variants are additive.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum FigureLoadError {
    /// Reading or decoding one pack entry failed (a missing id, a hash mismatch, a kind mismatch,
    /// or [`FigureFormatError`] wrapped by [`AssetStore::load`] into
    /// [`AssetError::Decode`]).
    #[error("asset error loading figure: {0}")]
    Asset(#[from] AssetError),
    /// Registering a decoded mesh with the renderer failed.
    #[error("failed to register a figure mesh with the renderer: {0}")]
    Mesh(#[from] MeshError),
    /// Registering a decoded texture with the renderer failed.
    #[error("failed to register a figure texture with the renderer: {0}")]
    Texture(#[from] TextureError),
    /// A material referenced a texture index outside its figure's own texture list (shared spec:
    /// "Index in die Texturliste der Figur"), a cross-payload consistency check
    /// [`figure_format`]'s per-payload decoders cannot make on their own.
    #[error(
        "material references texture index {index}, but the figure has only {texture_count} textures"
    )]
    TextureIndexOutOfRange {
        /// The offending index.
        index: u32,
        /// Number of textures the figure actually lists.
        texture_count: u32,
    },
}

/// One loaded figure part: its registered mesh and resolved material (texture indices already
/// turned into [`TextureHandle`]s registered with the renderer that produced them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadedFigurePart {
    /// Registered geometry, ready for [`grimoire_render::MeshInstance::mesh`].
    pub mesh: MeshHandle,
    /// Resolved material, ready to push onto [`grimoire_render::StageFrame::materials`].
    pub material: PbrMaterial,
}

/// A fully loaded figure (P1 "Figuren in der Engine" package): every part's mesh registered, every
/// material resolved to registered [`TextureHandle`]s, and the decoded skeleton — everything
/// [`load_figure`] can determine without a pose (see this module's doc comment for what a caller
/// still supplies to actually draw it).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedFigure {
    /// Mesh/material pairs making up this figure, in the order the figure manifest listed them.
    pub parts: Vec<LoadedFigurePart>,
    /// The figure's decoded skeleton (joint hierarchy, inverse bind matrices, rest pose).
    pub skeleton: SkeletonData,
    /// Axis-aligned bounding box minimum, model space.
    pub bounds_min: [f32; 3],
    /// Axis-aligned bounding box maximum, model space.
    pub bounds_max: [f32; 3],
}

/// Builds the [`AssetId`] of `figures/<figure_name>/<leaf>` (shared spec path convention).
///
/// # Errors
/// [`FigureLoadError::Asset`] if `figure_name`/`leaf` do not form a valid [`AssetPath`] (contract
/// §12: ASCII `[a-z0-9_.-]` and `/` only).
fn figure_path_id(figure_name: &str, leaf: &str) -> Result<AssetId, FigureLoadError> {
    let path = AssetPath::new(&format!("figures/{figure_name}/{leaf}"))?;
    Ok(AssetId::from_path(&path))
}

fn resolve_texture_index(
    index: Option<u32>,
    texture_handles: &[TextureHandle],
) -> Result<Option<TextureHandle>, FigureLoadError> {
    let Some(index) = index else {
        return Ok(None);
    };
    texture_handles
        .get(index as usize)
        .copied()
        .map(Some)
        .ok_or(FigureLoadError::TextureIndexOutOfRange {
            index,
            // `texture_handles.len()` is bounded by `MAX_FIGURE_TEXTURES` (a u32), checked when
            // the figure manifest was decoded.
            texture_count: u32::try_from(texture_handles.len()).unwrap_or(u32::MAX),
        })
}

/// Resolves `payload`'s texture indices against `texture_handles` (in figure-texture-list order)
/// into a [`PbrMaterial`].
///
/// # Errors
/// [`FigureLoadError::TextureIndexOutOfRange`] if any texture index is out of range for
/// `texture_handles`.
fn resolve_material(
    payload: MaterialPayload,
    texture_handles: &[TextureHandle],
) -> Result<PbrMaterial, FigureLoadError> {
    let mut material = PbrMaterial::default();
    material.base_color_factor = payload.base_color_factor;
    material.metallic_factor = payload.metallic_factor;
    material.roughness_factor = payload.roughness_factor;
    material.emissive_factor = payload.emissive_factor;
    material.alpha_mode = payload.alpha_mode;
    material.base_color_texture =
        resolve_texture_index(payload.base_color_texture_index, texture_handles)?;
    material.normal_texture = resolve_texture_index(payload.normal_texture_index, texture_handles)?;
    material.occlusion_roughness_metallic_texture = resolve_texture_index(
        payload.occlusion_roughness_metallic_texture_index,
        texture_handles,
    )?;
    Ok(material)
}

/// Wraps a [`FigureFormatError`] as the [`AssetError::Decode`] [`AssetStore::load`] would have
/// produced had it run this decode step itself — used for [`figure_format::validate_joint_indices`],
/// a cross-payload check that only makes sense once both the mesh and its skeleton are already
/// loaded, so it cannot run inside a `load` decode closure.
fn as_decode_error(id: AssetId, error: &FigureFormatError) -> FigureLoadError {
    FigureLoadError::Asset(AssetError::Decode {
        id,
        message: error.to_string(),
    })
}

/// Loads and registers the figure at `figures/<figure_name>/figure` (shared spec path convention)
/// through `store`, registering every mesh and texture with `renderer`.
///
/// Reads, in order: the figure manifest (kind [`FNP_FIGURE`]); its skeleton ([`FNP_SKELETON`]);
/// every texture it lists ([`FNP_TEXTURE_RAW`], registered via
/// [`WgpuRenderer::register_texture`] in list order, so a material's texture index resolves to the
/// matching handle); then, per part, its mesh ([`FNP_MESH`], cross-checked against the skeleton's
/// joint count via [`figure_format::validate_joint_indices`] before being registered via
/// [`WgpuRenderer::register_mesh`]) and material ([`FNP_MATERIAL`]).
///
/// # Errors
/// [`FigureLoadError`]; never panics for any pack content — every decode step is
/// [`figure_format`]'s own panic-free decoding (contract §2 rule 9), and every cross-payload
/// consistency check this function adds on top ([`figure_format::validate_joint_indices`],
/// texture index range) reports a [`FigureLoadError`] instead.
pub fn load_figure(
    store: &mut AssetStore,
    renderer: &mut WgpuRenderer,
    figure_name: &str,
) -> Result<LoadedFigure, FigureLoadError> {
    let figure_id = figure_path_id(figure_name, "figure")?;
    let manifest_handle =
        store.load(figure_id, FNP_FIGURE, figure_format::decode_figure_manifest)?;
    let manifest = store
        .get(manifest_handle)
        .expect("just loaded above, so it is present")
        .clone();

    let skeleton_handle = store.load(
        AssetId(manifest.skeleton_id),
        FNP_SKELETON,
        figure_format::decode_skeleton,
    )?;
    let skeleton = store
        .get(skeleton_handle)
        .expect("just loaded above, so it is present")
        .clone();
    let joint_count = skeleton.joint_count();

    let mut texture_handles = Vec::with_capacity(manifest.texture_ids.len());
    for &texture_id in &manifest.texture_ids {
        let texture_handle = store.load(
            AssetId(texture_id),
            FNP_TEXTURE_RAW,
            figure_format::decode_texture_raw,
        )?;
        let texture_data = store
            .get(texture_handle)
            .expect("just loaded above, so it is present")
            .clone();
        texture_handles.push(renderer.register_texture(texture_data)?);
    }

    let mut parts = Vec::with_capacity(manifest.parts.len());
    for part in &manifest.parts {
        let mesh_id = AssetId(part.mesh_id);
        let mesh_handle = store.load(mesh_id, FNP_MESH, figure_format::decode_mesh)?;
        let mesh_data = store
            .get(mesh_handle)
            .expect("just loaded above, so it is present")
            .clone();
        figure_format::validate_joint_indices(&mesh_data, joint_count)
            .map_err(|error| as_decode_error(mesh_id, &error))?;
        let mesh = renderer.register_mesh(mesh_data)?;

        let material_handle = store.load(
            AssetId(part.material_id),
            FNP_MATERIAL,
            figure_format::decode_material,
        )?;
        let material_payload = *store
            .get(material_handle)
            .expect("just loaded above, so it is present");
        let material = resolve_material(material_payload, &texture_handles)?;

        parts.push(LoadedFigurePart { mesh, material });
    }

    Ok(LoadedFigure {
        parts,
        skeleton,
        bounds_min: manifest.bounds_min,
        bounds_max: manifest.bounds_max,
    })
}
