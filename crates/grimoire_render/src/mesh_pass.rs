//! GPU mesh pass (plan 0002 WP2.3, PBR shading added in WP2.5): a depth buffer, static per-mesh
//! vertex/index buffers, and — from WP2.5 — real PBR shading (GGX microfacet specular with
//! height-correlated Smith visibility, Schlick Fresnel, metallic-roughness parameters, an analytic
//! ambient term, geometric specular anti-aliasing and optional textures); see `mesh.wgsl` for the
//! shading maths itself, which this module only feeds with per-frame and per-instance data.
//!
//! Drawn in [`crate::RenderLayer::World`], before the existing sprite pass (contract §6: sprites
//! and meshes share layers 1-3): [`crate::WgpuRenderer::render_stage`] runs this pass first,
//! clearing colour and depth, then runs the unchanged sprite pass on top with `LoadOp::Load` (no
//! depth test), so sprites always draw over meshes — exactly like before WP2.3, from the sprite
//! pass's point of view.
//!
//! **Registered vs. drawn (contract §6, PO decision V-20, 2026-09-16):** a [`crate::MeshInstance`]
//! whose `material` and `transform` are valid but whose `mesh` handle was never registered with
//! this renderer is no longer counted as drawn by [`crate::StageStats::meshes_drawn`] — it is
//! counted separately, in [`crate::StageStats::meshes_rejected_unregistered`], and is still
//! silently *not* rasterised by this pass, without a panic. The structural checks (`layer`,
//! `transform`, `material`) stay shared code in `crate::stage`, applied identically by every
//! renderer; only the registry check itself — [`MeshPass::is_registered`] here — is specific to a
//! renderer that owns one, which is why [`crate::NullRenderer`] (no registry of its own) never
//! reports this counter. The same is true of [`MeshPass::is_texture_registered`], added in WP2.5
//! for [`crate::TextureHandle`], except an unregistered *texture* handle is never an error: a
//! material's texture slot simply falls back to its factor (see `mesh.wgsl`'s header comment).
//!
//! **Point-light budget and clustered forward+ (plan 0002 WP3.4, engine ADR-0015 "compute
//! clustering"):** [`crate::LightBudget`] (`Low` 32 / `High` 256, contract §6, PRD-0003 FR-11),
//! chosen once at construction (`MeshPass::new`), sizes the `cluster_layout` storage buffers
//! `cluster_pass::ClusterPass` owns. [`build_light_list`] filters `StageFrame::point_lights` to the
//! valid ones (contract §6 `PointLight::is_valid`), clamps them to that budget (frame order,
//! reporting how many were dropped for [`crate::StageStats::point_lights_over_budget`]), and
//! applies [`crate::BulletLightCap`] to any `is_bullet_light` light's uploaded intensity (PRD-0003
//! rule 5 / FR-15, PO decision 2026-09-16 — see [`build_light_list`]'s doc comment). The result
//! goes to the GPU once per frame; [`ClusterPass::dispatch`] assigns lights to froxels on the GPU
//! (never the CPU — engine ADR-0015 measured a CPU assignment loop overrunning the WP3.3 render-CPU
//! budget at 256 lights) and `mesh.wgsl`'s fragment shader reads the result from group 4, replacing
//! the pre-WP3.4 fixed, unclustered array capped at 32.
//!
//! **Shadows (plan 0002 WP2.6, OF-3.2):** two techniques, selected per frame by
//! [`crate::StageFrame::shadow_config`]'s [`crate::ShadowMode`]. A depth-only shadow map for the
//! key light (`shadow.wgsl`, [`crate::shadow_pass::ShadowPass`], owned by this pass because it
//! already owns the registered meshes' GPU buffers the shadow pass draws) is fitted with an
//! orthographic frustum around the camera's ground target
//! ([`stage3d::key_light_view_projection`]) and PCF-sampled from `mesh.wgsl`'s fragment shader
//! through a comparison sampler bound in a third bind group (group 2). Blob shadows
//! (`blob_shadow.wgsl`, [`crate::BlobShadowInstance`]) are cheap alpha-blended decals drawn in the
//! *same* render pass as the opaque meshes, right after them: depth-tested against the
//! already-populated depth buffer (so a wall still occludes one, and an actor standing on one
//! still draws over it) but never depth-written. Both are built and skipped based on
//! [`crate::ShadowMode::wants_key_light_shadow_map`]/[`crate::ShadowMode::wants_blob_shadows`], so
//! neither technique costs anything on a frame that does not use it. Deferred: point-light shadow
//! casters (see [`crate::ShadowMode::KeyLightPlusPoints`]'s doc comment and this crate's WP2.6
//! ADR).
//!
//! **Mipmaps and multisampling (texture-quality package, strand B1, 2026-09-16):**
//! [`upload_texture`] generates a full CPU-side mip chain for every registered texture
//! ([`generate_mip_chain`]) instead of uploading a single level; the sampler's `mipmap_filter` is
//! `Linear` to match. Independently, [`MeshPass::render`]'s colour and depth targets multisample
//! at [`crate::Msaa`]'s configured level (default [`crate::Msaa::X4`]) and resolve into the pass's
//! previous, single-sample target before the sprite pass draws on top — the shadow pass and the
//! cluster compute dispatch above stay single-sampled, unaffected by this field (see
//! [`crate::Msaa`]'s doc comment for exactly what is and is not multisampled).
//!
//! Not part of the crate's public API (engine ADR-0002: `wgpu` stays invisible outside this
//! crate and its `grimoire_gpu` dependency).

use std::collections::HashMap;

use grimoire_gpu::{GpuContext, GpuError, wgpu};

use crate::cluster_layout::GpuPointLight;
use crate::cluster_pass::{ClusterCameraParams, ClusterFrameStats, ClusterPass};
use crate::mesh::{MeshData, MeshError, MeshRegistry};
use crate::shadow_pass::{ShadowCaster, ShadowPass};
use crate::stage3d::{self, AmbientLight, Camera25D, DirectionalLight, MAX_SKIN_JOINTS};
use crate::texture::{TextureData, TextureError, TextureRegistry};
use crate::{
    BlobShadowInstance, BulletLightCap, LightBudget, MeshHandle, MeshInstance, Msaa, PbrMaterial,
    PointLight, RenderLayer, ShadowConfig, SkinBinding, TextureHandle,
};

/// Depth-buffer format of the mesh pass. Guaranteed renderable on every `wgpu` backend, including
/// the software adapters (WARP, lavapipe) used in CI and in local tests (`GRIMOIRE_GPU_ADAPTER=
/// software`), so no downlevel fallback is needed.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Near clip plane of [`stage3d::view_projection`]. Not part of the [`Camera25D`] contract (which
/// has no near/far fields); a provisional constant until WP2.4 gives the camera rig a reason to
/// make it configurable.
const NEAR_PLANE: f32 = 0.05;
/// Far clip plane; see [`NEAR_PLANE`].
const FAR_PLANE: f32 = 2000.0;

const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Minimum clamped roughness, matching `mesh.wgsl`'s `MIN_ROUGHNESS`. A texture- or factor-sourced
/// roughness of exactly `0` would divide by zero in the shader's height-correlated Smith
/// visibility term; kept here too so [`MeshInstanceGpu`]'s doc comment and the CPU-side reference
/// tests (`tests` module below) have one named source for the value, even though the upload path
/// itself does not clamp (the shader already does, see `mesh.wgsl`'s header comment on why the
/// clamp lives there and not here).
#[cfg(test)]
const MIN_ROUGHNESS: f32 = 0.045;

mod instance_gpu {
    // bytemuck's derive macros expand to `unsafe impl` blocks, like `SpriteInstance` elsewhere in
    // this crate.
    #![allow(unsafe_code)]

    /// Per-instance data uploaded for the mesh pass: the model transform and the material inputs
    /// `mesh.wgsl`'s PBR shading consumes. An internal GPU upload layout, not a contract type:
    /// [`crate::MeshInstance`] and [`crate::PbrMaterial`] stay the public shape; this is what they
    /// get flattened into for the vertex buffer.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct MeshInstanceGpu {
        /// Column-major model-to-world transform, copied verbatim from [`crate::MeshInstance::transform`].
        pub transform: [[f32; 4]; 4],
        /// [`crate::PbrMaterial::base_color_factor`] of the instance's material.
        pub base_color: [f32; 4],
        /// [`crate::PbrMaterial::emissive_factor`] of the instance's material, `w` unused.
        pub emissive: [f32; 4],
        /// `x` = [`crate::PbrMaterial::metallic_factor`], `y` = [`crate::PbrMaterial::roughness_factor`],
        /// `z`/`w` reserved (always `0`).
        pub material_params: [f32; 4],
    }
}
use instance_gpu::MeshInstanceGpu;

/// Size of one [`MeshInstanceGpu`] in the instance buffer.
const MESH_INSTANCE_SIZE: u64 = std::mem::size_of::<MeshInstanceGpu>() as u64;

mod skinned_instance_gpu {
    // bytemuck's derive macros expand to `unsafe impl` blocks, like `MeshInstanceGpu` above.
    #![allow(unsafe_code)]

    /// Per-instance data for the skinning vertex path (P1 skinning addendum, contract §6
    /// changelog 2026-09-16): exactly [`super::instance_gpu::MeshInstanceGpu`]'s fields, plus
    /// where this instance's bone matrix palette starts in the storage buffer bound at group 3
    /// (`bone_offset`, contract §6 `SkinBinding::joint_offset`). Three `u32` of padding keep the
    /// struct's size a multiple of 16 bytes, matching every other GPU-uploaded struct in this
    /// crate; the shader never reads them.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct SkinnedMeshInstanceGpu {
        pub transform: [[f32; 4]; 4],
        pub base_color: [f32; 4],
        pub emissive: [f32; 4],
        pub material_params: [f32; 4],
        pub bone_offset: u32,
        pub _pad: [u32; 3],
    }
}
use skinned_instance_gpu::SkinnedMeshInstanceGpu;

/// Size of one [`SkinnedMeshInstanceGpu`] in the skinned instance buffer.
const SKINNED_MESH_INSTANCE_SIZE: u64 = std::mem::size_of::<SkinnedMeshInstanceGpu>() as u64;

/// Size in bytes of one bone matrix in the storage buffer bound at group 3 (`mat4x4<f32>`, std430
/// layout — a plain `[[f32; 4]; 4]` matches it exactly, no padding). Matches engine ADR-0013's
/// "256 Knochen x 64 Byte = 16 KiB" headroom calculation.
const BONE_MATRIX_SIZE: u64 = std::mem::size_of::<[[f32; 4]; 4]>() as u64;

mod camera_gpu {
    // bytemuck's derive macros expand to `unsafe impl` blocks, like `MeshInstanceGpu` above.
    #![allow(unsafe_code)]

    /// Per-frame camera and lighting uniform consumed by `mesh.wgsl`'s `Camera` struct. Field
    /// order and padding mirror the WGSL struct exactly (a unit test below freezes the byte size);
    /// see this file's `camera_uniform` for how it is filled in.
    ///
    /// `light_view_proj`/`shadow_params` are WP2.6's addition (OF-3.2): the key-light shadow
    /// map's light-space view-projection and, packed into one `vec4`, `x` = shadow-map texel size
    /// (`1.0 / map_size`, for PCF stepping), `y` = PCF kernel radius in texels (as a float, `0` =
    /// a single hard-edged tap), `z` = `1.0` if the shadow map should be sampled at all this frame
    /// (`0.0` otherwise, in which case `mesh.wgsl` skips the texture read entirely rather than
    /// sampling a possibly-stale map), `w` unused.
    ///
    /// `right`/`up`/`forward`/`cluster_proj` are WP3.4's addition (engine ADR-0015): the same
    /// camera-local basis and projection scalars `cluster_pass::ClusterCameraParams` carries,
    /// mirrored here so `mesh.wgsl`'s fragment shader can bucket a shaded point into the same
    /// froxel `cluster.wgsl`'s compute pass assigned each light into (`cluster_index_for` in both
    /// files, kept in sync by hand like the rest of this layout). Replaces the pre-WP3.4
    /// `light_count`/fixed-size `lights` array — lights now live in group 4's storage buffers,
    /// which need no compile-time count.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct CameraGpu {
        pub view_proj: [[f32; 4]; 4],
        pub eye: [f32; 4],
        pub light_dir: [f32; 4],
        pub key_light: [f32; 4],
        pub ambient_sky: [f32; 4],
        pub ambient_ground: [f32; 4],
        pub right: [f32; 4],
        pub up: [f32; 4],
        pub forward: [f32; 4],
        /// `x` = `f_over_aspect`, `y` = `f`, `z` = `near`, `w` = `far` — see
        /// `cluster_pass::ClusterCameraParams`.
        pub cluster_proj: [f32; 4],
        pub specular_aa_strength: f32,
        pub _pad1: u32,
        pub _pad2: u32,
        pub _pad3: u32,
        pub light_view_proj: [[f32; 4]; 4],
        pub shadow_params: [f32; 4],
    }
}
use camera_gpu::CameraGpu;

/// GPU-side geometry of one registered mesh: static vertex and index buffers, uploaded once at
/// [`MeshPass::register`] and never rewritten.
struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

/// GPU-side pixels of one registered texture (plan 0002 WP2.5), or of a fallback default. `texture`
/// is kept alongside `view` purely to keep the underlying GPU resource alive for as long as the
/// view is bound, mirroring [`GpuMesh`] and `grimoire_gpu::OffscreenTarget`.
struct GpuTexture {
    #[allow(
        dead_code,
        reason = "keeps the GPU texture alive for `view`; never read directly"
    )]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// Which registered textures (if any) a material resolves to, collapsed so "no handle" and "an
/// unregistered handle" are indistinguishable (both fall back to the default texture, see
/// [`MeshPass::texture_bind_key`]) — two materials that resolve to the same key share one bind
/// group and, when they also share a mesh, one draw call.
type TextureBindKey = (
    Option<TextureHandle>,
    Option<TextureHandle>,
    Option<TextureHandle>,
);

/// Largest instance count a buffer of at most `max_buffer_size` bytes can hold. Mirrors
/// `sprite_pass::max_instance_capacity`, sized for [`MeshInstanceGpu`] instead of `SpriteInstance`
/// (the two passes have different, independently evolving instance layouts, so a shared generic
/// helper would buy little); [`crate::sprite_pass`]'s pure `grown_capacity` sizing arithmetic
/// *is* shared — see its call sites below.
fn mesh_instance_capacity(max_buffer_size: u64) -> u32 {
    u32::try_from(max_buffer_size / MESH_INSTANCE_SIZE).unwrap_or(u32::MAX)
}

fn create_mesh_instance_buffer(
    context: &GpuContext,
    capacity: u32,
) -> Result<wgpu::Buffer, GpuError> {
    let size = u64::from(capacity) * MESH_INSTANCE_SIZE;
    context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire mesh instances"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })
}

/// Largest skinned-instance count a buffer of at most `max_buffer_size` bytes can hold; mirrors
/// [`mesh_instance_capacity`] for [`SkinnedMeshInstanceGpu`]'s (larger) layout.
fn skinned_mesh_instance_capacity(max_buffer_size: u64) -> u32 {
    u32::try_from(max_buffer_size / SKINNED_MESH_INSTANCE_SIZE).unwrap_or(u32::MAX)
}

fn create_skinned_mesh_instance_buffer(
    context: &GpuContext,
    capacity: u32,
) -> Result<wgpu::Buffer, GpuError> {
    let size = u64::from(capacity) * SKINNED_MESH_INSTANCE_SIZE;
    context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire skinned mesh instances"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })
}

/// Largest bone-matrix count a storage buffer of at most `max_storage_buffer_binding_size` bytes
/// can hold. Engine ADR-0013 measured `max_storage_buffer_binding_size` at 128 MiB or more on
/// every CI target adapter, so [`MAX_SKIN_JOINTS`] (256, 16 KiB) is nowhere near this ceiling for a
/// single instance; this only bounds how many *instances'* palettes can be concatenated into one
/// frame's buffer.
fn bone_matrix_capacity(max_storage_buffer_binding_size: u64) -> u32 {
    u32::try_from(max_storage_buffer_binding_size / BONE_MATRIX_SIZE).unwrap_or(u32::MAX)
}

fn create_bone_buffer(context: &GpuContext, capacity: u32) -> Result<wgpu::Buffer, GpuError> {
    // At least one matrix: a zero-sized buffer is invalid to create and to bind, and this buffer
    // is always bound (in the skinned pipeline's bind group) even on a frame with no skinned
    // instances at all.
    let size = u64::from(capacity.max(1)) * BONE_MATRIX_SIZE;
    context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire mesh bone matrices"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })
}

/// Creates the mesh pass's depth buffer at `sample_count` (texture-quality package, strand B1:
/// `1` for [`Msaa::Off`], `4` for [`Msaa::X4`] — [`Msaa::sample_count`]), matching whatever sample
/// count the colour target this frame renders into uses (`msaa_color_view`/`view` in
/// [`MeshPass::render`]): `wgpu` requires every attachment of a render pass, colour and depth
/// alike, to agree on sample count.
fn create_depth_view(
    context: &GpuContext,
    width: u32,
    height: u32,
    sample_count: u32,
) -> Result<wgpu::TextureView, GpuError> {
    let texture = context.capture_errors(|device| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grimoire mesh depth buffer"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    })?;
    Ok(texture.create_view(&wgpu::TextureViewDescriptor::default()))
}

/// Creates the mesh pass's multisampled colour target (texture-quality package, strand B1,
/// [`Msaa::X4`] only — [`MeshPass::render`] never has a `msaa_color_view` at all under
/// [`Msaa::Off`], so this is never called then): a `RENDER_ATTACHMENT`-only texture at
/// `sample_count` (never sampled or read back directly — [`MeshPass::render`] resolves it into
/// the pass's previous, single-sample colour target every frame, exactly like the depth buffer
/// above needing no `TEXTURE_BINDING` usage).
fn create_msaa_color_view(
    context: &GpuContext,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    sample_count: u32,
) -> Result<wgpu::TextureView, GpuError> {
    let texture = context.capture_errors(|device| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grimoire mesh msaa colour target"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    })?;
    Ok(texture.create_view(&wgpu::TextureViewDescriptor::default()))
}

/// Multisample state shared by every pipeline drawn into the mesh pass's render pass (the rigid,
/// skinned and blob-shadow pipelines — [`MeshPass::render`]'s header comment on draw order): all
/// three must agree on sample count with each other and with the pass's colour/depth attachments,
/// so this is the one place that builds it.
fn multisample_state(sample_count: u32) -> wgpu::MultisampleState {
    wgpu::MultisampleState {
        count: sample_count,
        mask: !0,
        alpha_to_coverage_enabled: false,
    }
}

fn upload_mesh(context: &GpuContext, data: &MeshData) -> Result<GpuMesh, GpuError> {
    let vertex_bytes: &[u8] = bytemuck::cast_slice(data.vertices.as_slice());
    let index_bytes: &[u8] = bytemuck::cast_slice(data.indices.as_slice());
    let vertex_buffer = context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire mesh vertices"),
            size: vertex_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })?;
    let index_buffer = context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire mesh indices"),
            size: index_bytes.len() as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })?;
    context
        .queue()
        .write_buffer(&vertex_buffer, 0, vertex_bytes);
    context.queue().write_buffer(&index_buffer, 0, index_bytes);
    Ok(GpuMesh {
        vertex_buffer,
        index_buffer,
        index_count: u32::try_from(data.indices.len()).unwrap_or(u32::MAX),
    })
}

/// Decodes one sRGB-encoded channel (`0.0..=1.0`) to linear light, the exact transfer function
/// `Rgba8UnormSrgb` hardware sampling applies (IEC 61966-2-1) — used here so the CPU-side mip
/// averaging below agrees with what the GPU would compute if it could average texels itself.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Encodes one linear-light channel (`0.0..=1.0`) back to sRGB; the inverse of [`srgb_to_linear`].
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// `srgb_to_linear` for every possible input byte, computed once per process (mipmap CPU
/// generation, texture-quality package strand B1). [`box_filter_downsample`] decodes every
/// *source* texel it reads — the large majority of this module's sRGB transfer-function calls,
/// since a full mip chain reads roughly 4/3 of a texture's source pixel count in total, each up to
/// three channels — while only *encoding* once per (far fewer) output texel, so this table is
/// worth building; a matching encode table is not, both because encode inputs are not bytes and
/// because there are so many fewer of them. Values are bit-identical to calling
/// [`srgb_to_linear`] directly (this table is exactly [`srgb_to_linear`]'s output at every byte
/// value, not an approximation), so replacing the call with a lookup changes performance only,
/// never a single averaged pixel's value.
fn srgb_to_linear_lut() -> &'static [f32; 256] {
    static TABLE: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0f32; 256];
        for (byte, slot) in table.iter_mut().enumerate() {
            *slot = srgb_to_linear(byte as f32 / 255.0);
        }
        table
    })
}

/// Averages `texels` (anywhere from 1 to 9 RGBA8 texels — see [`box_filter_downsample`]'s doc
/// comment for why the box can widen past 2x2 to a 2x3, 3x2 or 3x3 box at the last row/column of
/// an odd source dimension) into one output texel for the next mip level (mipmap CPU generation,
/// texture-quality package strand B1). `color_space` decides how the colour channels are
/// averaged: [`crate::texture::TextureColorSpace::Srgb`] decodes each to linear, averages, then
/// re-encodes (sRGB → linear → mitteln → sRGB, per the Festlegung);
/// [`crate::texture::TextureColorSpace::Linear`] averages the raw bytes directly. Alpha is
/// **always** averaged directly, regardless of `color_space`: `Rgba8UnormSrgb` only applies the
/// sRGB transfer function to R/G/B on the GPU, never to A (it stays plain `Unorm`), so decoding
/// alpha here would disagree with how the sampled texture is actually interpreted downstream.
fn average_texels(texels: &[[u8; 4]], color_space: crate::texture::TextureColorSpace) -> [u8; 4] {
    debug_assert!(!texels.is_empty() && texels.len() <= 9);
    let count = texels.len() as f32;
    let mut linear_sum = [0f32; 3];
    let mut alpha_sum = 0f32;
    let lut =
        matches!(color_space, crate::texture::TextureColorSpace::Srgb).then(srgb_to_linear_lut);
    for texel in texels {
        for (channel, sum) in linear_sum.iter_mut().enumerate() {
            *sum += match lut {
                Some(table) => table[texel[channel] as usize],
                None => f32::from(texel[channel]) / 255.0,
            };
        }
        alpha_sum += f32::from(texel[3]) / 255.0;
    }
    let mut out = [0u8; 4];
    for (channel, sum) in linear_sum.into_iter().enumerate() {
        let mean_linear = sum / count;
        let mean = match color_space {
            crate::texture::TextureColorSpace::Srgb => linear_to_srgb(mean_linear),
            crate::texture::TextureColorSpace::Linear => mean_linear,
        };
        out[channel] = (mean.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    out[3] = ((alpha_sum / count).clamp(0.0, 1.0) * 255.0).round() as u8;
    out
}

/// Downsamples one RGBA8 mip level (`width` x `height`, row-major, top row first) to the next
/// (`next_width` x `next_height`) with a box filter (mipmap CPU generation, texture-quality
/// package strand B1). `next_width`/`next_height` must each be `(dimension / 2).max(1)` (the same
/// halving `wgpu` itself uses to size a texture's mip chain) — every *destination* column/row gets
/// a source box of 2 columns/rows, **except the last one when the source dimension is odd**, whose
/// box widens to 3 columns/rows, absorbing the one extra remaining source column/row instead of
/// dropping it (so the last destination texel, when *both* dimensions are odd, draws from a 3x3 —
/// up to 9-texel — box, not the usual 2x2). This way the chain's level count always matches what
/// `wgpu` allows for `(width, height)` (`1 + floor(log2(max(width, height)))`, the standard mip
/// chain length), and no source pixel is ever skipped or read out of bounds, for square,
/// non-square and odd sizes alike.
fn box_filter_downsample(
    pixels: &[u8],
    width: u32,
    height: u32,
    next_width: u32,
    next_height: u32,
    color_space: crate::texture::TextureColorSpace,
) -> Vec<u8> {
    let mut out = vec![0u8; next_width as usize * next_height as usize * 4];
    for dy in 0..next_height {
        let y_start = dy * 2;
        let y_count = if dy + 1 == next_height {
            height - y_start
        } else {
            2
        };
        for dx in 0..next_width {
            let x_start = dx * 2;
            let x_count = if dx + 1 == next_width {
                width - x_start
            } else {
                2
            };
            // At most 3x3 (both dimensions odd, last row and column at once).
            let mut texels: [[u8; 4]; 9] = [[0; 4]; 9];
            let mut count = 0usize;
            for y in y_start..y_start + y_count {
                for x in x_start..x_start + x_count {
                    let index = (y as usize * width as usize + x as usize) * 4;
                    texels[count] = [
                        pixels[index],
                        pixels[index + 1],
                        pixels[index + 2],
                        pixels[index + 3],
                    ];
                    count += 1;
                }
            }
            let averaged = average_texels(&texels[..count], color_space);
            let out_index = (dy as usize * next_width as usize + dx as usize) * 4;
            out[out_index..out_index + 4].copy_from_slice(&averaged);
        }
    }
    out
}

/// Builds the full mip chain for `data`'s pixels, level 0 first (a byte-for-byte copy of
/// `data.pixels`) down to a final 1x1 level, each later level a box-filtered downsample of the one
/// before it ([`box_filter_downsample`]). Pure CPU computation, no GPU dependency — matches
/// [`crate::texture`]'s "entirely GPU-free" module doc comment; [`upload_texture`] is the only
/// caller and does the actual GPU upload of every returned level.
fn generate_mip_chain(data: &TextureData) -> Vec<(u32, u32, Vec<u8>)> {
    let mut levels: Vec<(u32, u32, Vec<u8>)> = vec![(data.width, data.height, data.pixels.clone())];
    loop {
        let (width, height) = {
            let (width, height, _) = levels.last().expect("level 0 was pushed above");
            (*width, *height)
        };
        if width == 1 && height == 1 {
            break;
        }
        let next_width = (width / 2).max(1);
        let next_height = (height / 2).max(1);
        let next_pixels = {
            let (_, _, pixels) = levels.last().expect("level 0 was pushed above");
            box_filter_downsample(
                pixels,
                width,
                height,
                next_width,
                next_height,
                data.color_space,
            )
        };
        levels.push((next_width, next_height, next_pixels));
    }
    levels
}

/// Uploads `data`'s pixels to a new GPU texture, plus its full CPU-generated mip chain (mipmap
/// generation, texture-quality package strand B1): [`crate::TextureColorSpace::Srgb`] becomes
/// `Rgba8UnormSrgb`, [`crate::TextureColorSpace::Linear`] becomes `Rgba8Unorm` (contract §6: base
/// colour is sRGB, normal/ORM are linear).
///
/// **Mip generation lives here, on the CPU, not on the GPU:** [`generate_mip_chain`] computes
/// every level via a box filter (sRGB pixels decoded to linear, averaged, re-encoded — see its doc
/// comment) before any of it reaches the GPU; there is no compute- or render-pass-based
/// downsampling pipeline in this crate, deliberately, since this package's [`TextureData`] only
/// carries plain CPU pixels to begin with. No special-casing for normal maps: `mesh.wgsl`
/// renormalizes the sampled normal after decoding regardless of which mip level was sampled, so an
/// averaged (and therefore shorter-than-unit-length) normal vector in a lower mip is corrected
/// there, exactly like a GPU-generated mip chain would need the same renormalization. The 1x1
/// fallback textures ([`create_fallback_texture`]) are unaffected: they stay single-level, sampled
/// unconditionally regardless of mip level. [`TextureData`] and the contract are unchanged — this
/// is purely an upload-time implementation detail of this crate.
fn upload_texture(context: &GpuContext, data: &TextureData) -> Result<GpuTexture, GpuError> {
    let format = match data.color_space {
        crate::texture::TextureColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        crate::texture::TextureColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
    };
    let levels = generate_mip_chain(data);
    let mip_level_count = u32::try_from(levels.len()).unwrap_or(u32::MAX);
    let size = wgpu::Extent3d {
        width: data.width,
        height: data.height,
        depth_or_array_layers: 1,
    };
    let texture = context.capture_errors(|device| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grimoire material texture"),
            size,
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    })?;
    // `TextureData::validate` (run by `TextureRegistry::register` before this is ever called)
    // already guarantees `width * 4` fits a `u32` and `pixels.len() == width * height * 4`; every
    // later level is this function's own output, already exactly `level_width * level_height * 4`
    // bytes ([`box_filter_downsample`]).
    for (level, (level_width, level_height, level_pixels)) in levels.iter().enumerate() {
        context.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            level_pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(level_width * 4),
                rows_per_image: Some(*level_height),
            },
            wgpu::Extent3d {
                width: *level_width,
                height: *level_height,
                depth_or_array_layers: 1,
            },
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(GpuTexture { texture, view })
}

/// Uploads `data`'s pixels as a single-level GPU texture, without [`generate_mip_chain`] — the
/// pre-B1 upload path, kept only so
/// [`crate::WgpuRenderer::register_texture_single_level_for_measurement`] can give
/// `tests/offscreen.rs` a no-mipmap baseline to measure [`upload_texture`]'s relative cost against
/// (mirrors [`crate::WgpuRenderer::render_stage_with_specular_aa`]'s reason for existing, applied
/// to mip generation instead of specular AA). **Not part of the render contract** and never used by
/// [`MeshPass::register_texture`], the only path a game ever calls.
fn upload_texture_single_level(
    context: &GpuContext,
    data: &TextureData,
) -> Result<GpuTexture, GpuError> {
    let format = match data.color_space {
        crate::texture::TextureColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        crate::texture::TextureColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
    };
    let size = wgpu::Extent3d {
        width: data.width,
        height: data.height,
        depth_or_array_layers: 1,
    };
    let texture = context.capture_errors(|device| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grimoire material texture (single-level measurement)"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    })?;
    context.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data.pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(data.width * 4),
            rows_per_image: Some(data.height),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(GpuTexture { texture, view })
}

/// Creates a 1x1 fallback texture holding `texel` (already in `format`'s encoding), used when a
/// material's texture slot is `None` or points at an unregistered handle (`mesh.wgsl`'s header
/// comment explains why sampling this reproduces the material's plain factor).
fn create_fallback_texture(
    context: &GpuContext,
    format: wgpu::TextureFormat,
    texel: [u8; 4],
    label: &'static str,
) -> Result<GpuTexture, GpuError> {
    let size = wgpu::Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: 1,
    };
    let texture = context.capture_errors(|device| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    })?;
    context.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &texel,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(GpuTexture { texture, view })
}

/// Ambient term flattened to the two colours `mesh.wgsl` mixes by `normal.z`
/// ([`AmbientLight::Flat`] simply uses the same colour for both), each pre-multiplied by
/// intensity. An invalid ambient (contract §6 `AmbientLight::is_valid`) falls back to black —
/// matched by `StageStats::ambient_rejected_invalid` already flagging it, computed independently
/// by the existing `stage::extract_stage3d`.
fn ambient_terms(ambient: &AmbientLight) -> ([f32; 3], [f32; 3]) {
    if !ambient.is_valid() {
        return ([0.0; 3], [0.0; 3]);
    }
    match *ambient {
        AmbientLight::Flat { color, intensity } => {
            let value = [
                color[0] * intensity,
                color[1] * intensity,
                color[2] * intensity,
            ];
            (value, value)
        }
        AmbientLight::Hemisphere {
            sky_color,
            ground_color,
            intensity,
        } => (
            [
                sky_color[0] * intensity,
                sky_color[1] * intensity,
                sky_color[2] * intensity,
            ],
            [
                ground_color[0] * intensity,
                ground_color[1] * intensity,
                ground_color[2] * intensity,
            ],
        ),
    }
}

/// Splits `lights` into the ones the configured light budget can hold (at most `budget`, kept in
/// frame order) and how many were dropped because there were more. Never panics.
pub(crate) fn clamp_light_budget<T>(lights: &[T], budget: usize) -> (&[T], usize) {
    if lights.len() <= budget {
        (lights, 0)
    } else {
        (&lights[..budget], lights.len() - budget)
    }
}

/// Filters `point_lights` to the valid ones (contract §6 `PointLight::is_valid`), clamps them to
/// `light_budget` (frame order, plan 0002 WP3.4's [`crate::LightBudget`]) and converts each
/// survivor into [`cluster_layout::GpuPointLight`] (WP3.1, engine ADR-0013's frozen layout — see
/// this module's doc comment) for [`ClusterPass::dispatch`]. Returns the list and how many valid
/// lights were dropped for budget reasons (`0` unless the frame really exceeds `light_budget`; see
/// [`crate::StageStats::point_lights_over_budget`]).
///
/// **Bullet-light cap (PRD-0003 rule 5 / FR-15, PO decision 2026-09-16):** a light with
/// [`PointLight::is_bullet_light`] set has its uploaded `intensity` multiplied by
/// `bullet_light_cap`'s [`BulletLightCap::clamped_floor_contribution`] before conversion — the
/// *only* place this cap is applied (`mesh.wgsl`'s shading equation reads the already-capped
/// intensity and does not know which lights were bullet lights at all). Scaling `intensity` rather
/// than `color` is equivalent (the shading equation only ever uses their product,
/// `light.color * light.intensity`) and keeps `cluster_layout::GpuPointLight::color` exactly
/// [`PointLight::color`], matching that type's own doc comment.
fn build_light_list(
    point_lights: &[PointLight],
    light_budget: usize,
    bullet_light_cap: &BulletLightCap,
) -> (Vec<GpuPointLight>, usize) {
    let valid: Vec<&PointLight> = point_lights
        .iter()
        .filter(|light| light.is_valid())
        .collect();
    let (used, dropped) = clamp_light_budget(&valid, light_budget);
    let cap = bullet_light_cap.clamped_floor_contribution();
    let lights = used
        .iter()
        .map(|light| GpuPointLight {
            position: light.position,
            range: light.range,
            color: light.color,
            intensity: if light.is_bullet_light {
                light.intensity * cap
            } else {
                light.intensity
            },
        })
        .collect();
    (lights, dropped)
}

/// Builds the uniform `mesh.wgsl` reads: the view-projection matrix, the eye position, the key
/// light and ambient term flattened into GPU-friendly vectors, the WP3.4 clustering camera basis
/// (`cluster_camera`, matching what [`ClusterPass::dispatch`] uploads to the compute pass so both
/// agree on every froxel), the OF-3.5 specular-AA toggle, and (WP2.6) the key-light shadow map's
/// light-space view-projection plus its `shadow_params` (see [`CameraGpu`]'s doc comment for the
/// packing). An invalid or missing key light (contract §6 `DirectionalLight::is_valid`) falls back
/// to no directional contribution at all, consistent with `StageStats::key_light_rejected_invalid`
/// already flagging it elsewhere.
#[allow(clippy::too_many_arguments)]
fn camera_uniform(
    view_proj: [[f32; 4]; 4],
    eye: [f32; 3],
    key_light: Option<&DirectionalLight>,
    ambient: &AmbientLight,
    cluster_camera: &ClusterCameraParams,
    specular_aa: bool,
    light_view_proj: [[f32; 4]; 4],
    shadow_params: [f32; 4],
) -> CameraGpu {
    let (light_dir, key_color) = match key_light.filter(|light| light.is_valid()) {
        Some(light) => {
            let length_squared = light.direction.iter().map(|c| c * c).sum::<f32>();
            let length = length_squared.sqrt();
            let towards_light = [
                -light.direction[0] / length,
                -light.direction[1] / length,
                -light.direction[2] / length,
            ];
            let color = [
                light.color[0] * light.intensity,
                light.color[1] * light.intensity,
                light.color[2] * light.intensity,
            ];
            (towards_light, color)
        }
        None => ([0.0; 3], [0.0; 3]),
    };
    let (sky, ground) = ambient_terms(ambient);

    CameraGpu {
        view_proj,
        eye: [eye[0], eye[1], eye[2], 0.0],
        light_dir: [light_dir[0], light_dir[1], light_dir[2], 0.0],
        key_light: [key_color[0], key_color[1], key_color[2], 0.0],
        ambient_sky: [sky[0], sky[1], sky[2], 0.0],
        ambient_ground: [ground[0], ground[1], ground[2], 0.0],
        right: [
            cluster_camera.right[0],
            cluster_camera.right[1],
            cluster_camera.right[2],
            0.0,
        ],
        up: [
            cluster_camera.up[0],
            cluster_camera.up[1],
            cluster_camera.up[2],
            0.0,
        ],
        forward: [
            cluster_camera.forward[0],
            cluster_camera.forward[1],
            cluster_camera.forward[2],
            0.0,
        ],
        cluster_proj: [
            cluster_camera.f_over_aspect,
            cluster_camera.f,
            cluster_camera.near,
            cluster_camera.far,
        ],
        specular_aa_strength: if specular_aa { 1.0 } else { 0.0 },
        _pad1: 0,
        _pad2: 0,
        _pad3: 0,
        light_view_proj,
        shadow_params,
    }
}

/// Return value of [`MeshPass::render`] (plan 0002 WP3.4 grew this from a plain draw-call count):
/// everything [`crate::WgpuRenderer::render_stage_impl`] needs to fill in
/// [`crate::StageStats`]'s base and WP3.4 fields. `StageStats::point_lights_over_budget` is
/// deliberately not here: `crate::stage::stage_stats_from_base` derives it from the already
/// structurally-computed `point_lights_drawn` count and the renderer's configured light budget, the
/// same "single source of truth, no duplicated validation" shape `meshes_rejected_unregistered`
/// already uses (contract §6, PO decision V-20).
pub(crate) struct MeshPassStats {
    /// Draw calls issued across every sub-pass this call touched; see [`MeshPass::render`]'s doc
    /// comment.
    pub draw_calls: u32,
    /// The clustered forward+ pass's own frame stats (WP3.4) —
    /// [`crate::StageStats::clusters_with_lights`]/[`crate::StageStats::light_cluster_index_entries`].
    pub cluster: ClusterFrameStats,
}

/// GPU mesh pipeline, depth buffer, mesh registry and texture registry, owned by
/// [`crate::WgpuRenderer`]. From WP2.6 (this module's header doc comment) it also owns the
/// key-light shadow map ([`ShadowPass`]) and the blob-shadow decal pipeline.
pub(crate) struct MeshPass {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    registry: MeshRegistry,
    gpu_meshes: HashMap<MeshHandle, GpuMesh>,
    texture_registry: TextureRegistry,
    gpu_textures: HashMap<TextureHandle, GpuTexture>,
    default_base_color: GpuTexture,
    default_normal: GpuTexture,
    default_orm: GpuTexture,
    sampler: wgpu::Sampler,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_groups: HashMap<TextureBindKey, wgpu::BindGroup>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u32,
    /// Colour format of the pass's (single-sample) output target, kept only to recreate
    /// `msaa_color_view` at a new size on [`MeshPass::resize`] (texture-quality package, strand
    /// B1) — never a `wgpu` type this crate's public API exposes.
    color_format: wgpu::TextureFormat,
    /// Multisampling level chosen at construction (texture-quality package, strand B1), fixed for
    /// this pass's lifetime like `light_budget` below — WP3.4's `light_budget` field is the
    /// precedent for a construction-time-only render parameter.
    msaa: Msaa,
    /// The mesh pass's multisampled colour target, `Some` only under [`Msaa::X4`] — `None` under
    /// [`Msaa::Off`], in which case [`MeshPass::render`] draws straight into its caller-provided
    /// target exactly as before this field existed. Resolved into that target every frame when
    /// `Some` ([`MeshPass::render`]'s render-pass `resolve_target`); recreated on
    /// [`MeshPass::resize`] alongside `depth_view` below.
    msaa_color_view: Option<wgpu::TextureView>,
    depth_view: wgpu::TextureView,
    depth_size: (u32, u32),
    /// Key-light shadow map (plan 0002 WP2.6): depth texture, comparison sampler, its own
    /// depth-only pipeline and instance buffer. Owned here (not standalone) because this pass
    /// already owns the registered meshes' GPU vertex/index buffers it draws.
    shadow_pass: ShadowPass,
    /// Layout of the mesh pipeline's group 2 (shadow-sampling): a `texture_depth_2d` plus a
    /// `sampler_comparison`, matching `mesh.wgsl`'s `shadow_map`/`shadow_sampler` bindings.
    shadow_bind_group_layout: wgpu::BindGroupLayout,
    /// Bind group for group 2, rebuilt whenever [`ShadowPass::configure`] actually replaces the
    /// shadow map's texture view (so it never goes stale); left as-is on a frame that skips the
    /// shadow map render entirely (`mesh.wgsl`'s `shadow_params.z` gates sampling regardless).
    shadow_sampling_bind_group: wgpu::BindGroup,
    /// Blob-shadow decal pipeline (plan 0002 WP2.6, `blob_shadow.wgsl`): alpha-blended, depth
    /// tested against the opaque meshes' depth buffer but never depth-writing, drawn in the same
    /// render pass right after them. Reuses `camera_bind_group_layout`/`camera_bind_group` (group
    /// 0) for `view_proj`; needs no texture group of its own.
    blob_pipeline: wgpu::RenderPipeline,
    blob_instance_buffer: wgpu::Buffer,
    blob_instance_capacity: u32,
    /// Second vertex path for skinned meshes (P1 skinning addendum, contract §6 changelog
    /// 2026-09-16, this module's header doc comment): its own pipeline (shares `fs_main` with
    /// `pipeline`), instance buffer and bone matrix storage buffer (group 3). An instance with
    /// `skin == None` never touches any of these — [`MeshPass::render`] groups instances by
    /// [`MeshInstance::skin`] before choosing a pipeline.
    skinned_pipeline: wgpu::RenderPipeline,
    skinned_instance_buffer: wgpu::Buffer,
    skinned_instance_capacity: u32,
    /// Layout of the skinned pipeline's group 3: one read-only storage buffer of bone matrices.
    bone_bind_group_layout: wgpu::BindGroupLayout,
    /// Storage buffer holding every skinned instance's bone matrix palette for the current frame,
    /// concatenated in [`crate::StageFrame::joint_matrices`] order; grows like `instance_buffer`.
    bone_buffer: wgpu::Buffer,
    /// Bind group for group 3, rebuilt whenever `bone_buffer` is recreated (buffer identity
    /// changes the binding, like `shadow_sampling_bind_group` above).
    bone_bind_group: wgpu::BindGroup,
    /// Capacity of `bone_buffer` in bone matrices (at least 1, see [`create_bone_buffer`]).
    bone_capacity: u32,
    /// Clustered forward+ light assignment (plan 0002 WP3.4, engine ADR-0015): owns the
    /// `cluster_layout` storage buffers (sized once for `light_budget`) and group 4's bind group,
    /// bound identically by `pipeline` and `skinned_pipeline` (both share `mesh.wgsl`'s `fs_main`).
    cluster_pass: ClusterPass,
    /// Configured light budget (`crate::LightBudget::light_count`), fixed for this pass's
    /// lifetime — WP3.4 does not support changing it after construction, matching how
    /// `RendererConfig`'s other construction-time parameters work.
    light_budget: usize,
}

/// One `texture_2d<f32>` binding entry of the texture bind group layout (group 1), all four
/// bindings the same shape except for the sampler.
fn texture_layout_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// Layout of the mesh pipeline's group 2 (plan 0002 WP2.6): the shadow map's depth texture plus
/// its comparison sampler, matching `mesh.wgsl`'s `shadow_map`/`shadow_sampler` bindings.
fn create_shadow_bind_group_layout(context: &GpuContext) -> wgpu::BindGroupLayout {
    context
        .device()
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grimoire mesh shadow layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        })
}

/// Builds the group-2 bind group from `shadow_pass`'s current view/sampler; called once at
/// construction and again whenever [`ShadowPass::configure`] replaces the view (see
/// [`MeshPass::shadow_sampling_bind_group`]'s doc comment).
fn create_shadow_sampling_bind_group(
    context: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    shadow_pass: &ShadowPass,
) -> wgpu::BindGroup {
    context
        .device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grimoire mesh shadow bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(shadow_pass.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(shadow_pass.sampler()),
                },
            ],
        })
}

/// Layout of the skinned mesh pipeline's group 3 (P1 skinning addendum, contract §6 changelog
/// 2026-09-16): a single read-only storage buffer of bone matrices, visible only to the vertex
/// stage — engine ADR-0013 measured `vertex_storage` (this exact capability) as `true` on all
/// three CI target adapters, with `max_storage_buffers_per_shader_stage` and
/// `max_storage_buffer_binding_size` far above what one more bound storage buffer needs.
fn create_bone_bind_group_layout(context: &GpuContext) -> wgpu::BindGroupLayout {
    context
        .device()
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grimoire mesh bone layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        })
}

fn create_bone_bind_group(
    context: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    context
        .device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grimoire mesh bone bind group"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
}

/// Builds the skinning vertex path's pipeline (P1 skinning addendum, contract §6 changelog
/// 2026-09-16, mesh pass module doc comment): the second vertex path alongside [`MeshPass`]'s
/// existing rigid-mesh pipeline, sharing `mesh.wgsl`'s `fs_main` fragment shader (and therefore
/// every shading behaviour — PBR, shadows, specular AA) but reading [`crate::MeshVertex::joints`]/
/// [`crate::MeshVertex::weights`] and a `bone_offset` per instance instead of transforming the raw
/// vertex position directly. An instance with `skin == None` never touches this pipeline or its
/// bone buffer (this module's `render` groups instances by `MeshInstance::skin` before choosing a
/// pipeline), so it costs nothing extra, as the contract requires.
#[allow(clippy::too_many_arguments)]
fn create_skinned_pipeline(
    context: &GpuContext,
    shader: &wgpu::ShaderModule,
    camera_bind_group_layout: &wgpu::BindGroupLayout,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
    shadow_bind_group_layout: &wgpu::BindGroupLayout,
    bone_bind_group_layout: &wgpu::BindGroupLayout,
    cluster_bind_group_layout: &wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
) -> Result<wgpu::RenderPipeline, GpuError> {
    let device = context.device();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grimoire skinned mesh layout"),
        bind_group_layouts: &[
            Some(camera_bind_group_layout),
            Some(texture_bind_group_layout),
            Some(shadow_bind_group_layout),
            Some(bone_bind_group_layout),
            // Group 4 (plan 0002 WP3.4): read-only cluster/light buffers, shared with the rigid
            // pipeline's layout below at the same group index — both feed the same `fs_main`.
            Some(cluster_bind_group_layout),
        ],
        immediate_size: 0,
    });

    let vertex_attributes: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Uint16x4,
        4 => Float32x4,
    ];
    let instance_attributes: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        5 => Float32x4,
        6 => Float32x4,
        7 => Float32x4,
        8 => Float32x4,
        9 => Float32x4,
        10 => Float32x4,
        11 => Float32x4,
        12 => Uint32,
    ];

    context.capture_errors(|device| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("grimoire skinned mesh pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_skinned"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<crate::mesh::MeshVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex_attributes,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: SKINNED_MESH_INSTANCE_SIZE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attributes,
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: multisample_state(sample_count),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    })
}

/// Builds the blob-shadow decal pipeline (plan 0002 WP2.6, `blob_shadow.wgsl`): alpha-blended,
/// depth-tested against the opaque meshes' depth buffer (`DEPTH_FORMAT`) but never depth-writing,
/// vertex-pulled (no vertex buffer, only a per-instance one). Reuses `camera_bind_group_layout`
/// (group 0) so it can be drawn with the same bind group as the opaque meshes, without a uniform
/// buffer of its own (`blob_shadow.wgsl`'s header comment explains why that is sound). `sample_count`
/// (texture-quality package, strand B1) must match the rigid/skinned pipelines' above: all three
/// draw into the same render pass ([`MeshPass::render`]'s header comment on draw order), so `wgpu`
/// requires them to agree.
fn create_blob_pipeline(
    context: &GpuContext,
    camera_bind_group_layout: &wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
) -> Result<wgpu::RenderPipeline, GpuError> {
    let device = context.device();
    let shader = context.capture_errors(|device| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grimoire blob shadow shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blob_shadow.wgsl").into()),
        })
    })?;
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grimoire blob shadow layout"),
        bind_group_layouts: &[Some(camera_bind_group_layout)],
        immediate_size: 0,
    });
    let instance_attributes: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32,
        2 => Float32,
        3 => Float32,
    ];
    context.capture_errors(|device| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("grimoire blob shadow pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<crate::stage3d::BlobShadowInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &instance_attributes,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // Never occludes anything drawn after it; only reads the depth the opaque meshes
                // already wrote this pass (`mesh_pass.rs`'s header doc comment on draw order).
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: multisample_state(sample_count),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    })
}

/// Largest instance count a buffer of at most `max_buffer_size` bytes can hold, sized for
/// [`crate::stage3d::BlobShadowInstance`] (mirrors [`mesh_instance_capacity`]).
fn blob_instance_capacity(max_buffer_size: u64) -> u32 {
    u32::try_from(
        max_buffer_size / std::mem::size_of::<crate::stage3d::BlobShadowInstance>() as u64,
    )
    .unwrap_or(u32::MAX)
}

fn create_blob_instance_buffer(
    context: &GpuContext,
    capacity: u32,
) -> Result<wgpu::Buffer, GpuError> {
    let size =
        u64::from(capacity) * std::mem::size_of::<crate::stage3d::BlobShadowInstance>() as u64;
    context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire blob shadow instances"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })
}

/// Whether `skin`'s range fits entirely inside a `joint_matrices` table of `joint_matrices_len`
/// entries and `joint_count` is within `1..=MAX_SKIN_JOINTS` (P1 skinning addendum). Same
/// arithmetic as `crate::stage::skin_binding_is_valid`, duplicated here for the reason
/// [`MeshPass::is_drawable`]'s doc comment gives for the rest of that predicate.
fn skin_binding_fits(skin: SkinBinding, joint_matrices_len: usize) -> bool {
    skin.joint_count > 0
        && skin.joint_count <= MAX_SKIN_JOINTS
        && skin
            .joint_offset
            .checked_add(skin.joint_count)
            .is_some_and(|end| (end as usize) <= joint_matrices_len)
}

impl MeshPass {
    pub(crate) fn new(
        context: &GpuContext,
        color_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        light_budget: LightBudget,
        msaa: Msaa,
    ) -> Result<Self, GpuError> {
        let sample_count = msaa.sample_count();
        let device = context.device();
        let shader = context.capture_errors(|device| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("grimoire mesh shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("mesh.wgsl").into()),
            })
        })?;

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire mesh camera uniform"),
            size: std::mem::size_of::<CameraGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("grimoire mesh camera layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grimoire mesh camera bind group"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("grimoire mesh texture layout"),
                entries: &[
                    texture_layout_entry(0),
                    texture_layout_entry(1),
                    texture_layout_entry(2),
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("grimoire material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // Trilinear filtering between mip levels (texture-quality package, strand B1): now
            // that `upload_texture` always uploads a full mip chain, blending the two nearest
            // levels avoids a visible seam where the shader's automatic LOD selection
            // (`textureSample`'s implicit derivatives in `mesh.wgsl`) crosses a level boundary.
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        // Sampling these reproduces the material's plain factors exactly (mesh.wgsl's header
        // comment): white base colour, tangent-space "straight up" normal (0.5, 0.5, 1.0 packed),
        // and neutral ORM (occlusion 1 = no darkening, roughness/metallic factors pass through
        // unchanged since they multiply a sampled 1.0).
        let default_base_color = create_fallback_texture(
            context,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            [255, 255, 255, 255],
            "grimoire default base colour texture",
        )?;
        let default_normal = create_fallback_texture(
            context,
            wgpu::TextureFormat::Rgba8Unorm,
            [128, 128, 255, 255],
            "grimoire default normal texture",
        )?;
        let default_orm = create_fallback_texture(
            context,
            wgpu::TextureFormat::Rgba8Unorm,
            [255, 255, 255, 255],
            "grimoire default ORM texture",
        )?;

        let shadow_bind_group_layout = create_shadow_bind_group_layout(context);

        // Clustered forward+ (plan 0002 WP3.4, engine ADR-0015): built before the pipeline
        // layouts below so both can bind its group-4 layout at the same index. `light_budget`
        // (`crate::LightBudget::light_count`) sizes its storage buffers for this pass's lifetime.
        let cluster_pass = ClusterPass::new(context, light_budget.light_count())?;

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grimoire mesh layout"),
            bind_group_layouts: &[
                Some(&camera_bind_group_layout),
                Some(&texture_bind_group_layout),
                Some(&shadow_bind_group_layout),
                // No group 3 for the rigid pipeline (that is the skinned pipeline's bone matrix
                // buffer, `vs_main`/`fs_main` never reference it) — `None` keeps group 4 (cluster
                // buffers, read by `fs_main`, shared with `skinned_pipeline`'s layout below) at
                // the same index in both pipelines.
                None,
                Some(cluster_pass.fragment_bind_group_layout()),
            ],
            immediate_size: 0,
        });

        let vertex_attributes: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x3,
            2 => Float32x2,
        ];
        let instance_attributes: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
            3 => Float32x4,
            4 => Float32x4,
            5 => Float32x4,
            6 => Float32x4,
            7 => Float32x4,
            8 => Float32x4,
            9 => Float32x4,
        ];

        let pipeline = context.capture_errors(|device| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("grimoire mesh pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<crate::mesh::MeshVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &vertex_attributes,
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: MESH_INSTANCE_SIZE,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &instance_attributes,
                        }),
                    ],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // The procedural test meshes do not guarantee consistent winding (see
                    // `crate::procedural`'s module doc), like the sprite pass's own rotation.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: multisample_state(sample_count),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        // Materials still draw opaque regardless of `alpha_mode`: real blending and
                        // alpha test are not part of plan 0002 WP2.5 (PBR shading and specular AA)
                        // and remain future work with no work package assigned yet.
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })?;

        let depth_view = create_depth_view(context, width, height, sample_count)?;
        // Multisampled colour target (texture-quality package, strand B1): only under
        // `Msaa::X4` (`sample_count > 1`) — see `msaa_color_view`'s doc comment.
        let msaa_color_view = if sample_count > 1 {
            Some(create_msaa_color_view(
                context,
                color_format,
                width,
                height,
                sample_count,
            )?)
        } else {
            None
        };
        let instance_capacity =
            16u32.min(mesh_instance_capacity(device.limits().max_buffer_size).max(1));
        let instance_buffer = create_mesh_instance_buffer(context, instance_capacity)?;

        let shadow_pass = ShadowPass::new(context, &ShadowConfig::default())?;
        let shadow_sampling_bind_group =
            create_shadow_sampling_bind_group(context, &shadow_bind_group_layout, &shadow_pass);
        let blob_pipeline = create_blob_pipeline(
            context,
            &camera_bind_group_layout,
            color_format,
            sample_count,
        )?;
        let blob_instance_capacity =
            16u32.min(blob_instance_capacity(device.limits().max_buffer_size).max(1));
        let blob_instance_buffer = create_blob_instance_buffer(context, blob_instance_capacity)?;

        // Second vertex path for skinned meshes (P1 skinning addendum, contract §6 changelog
        // 2026-09-16): its own pipeline sharing `fs_main`/`shader` with `pipeline` above, plus a
        // group-3 bone matrix storage buffer (engine ADR-0013 measured `vertex_storage` on every
        // CI target adapter with headroom far past a single 256-bone, 16 KiB palette).
        let bone_bind_group_layout = create_bone_bind_group_layout(context);
        let skinned_pipeline = create_skinned_pipeline(
            context,
            &shader,
            &camera_bind_group_layout,
            &texture_bind_group_layout,
            &shadow_bind_group_layout,
            &bone_bind_group_layout,
            cluster_pass.fragment_bind_group_layout(),
            color_format,
            sample_count,
        )?;
        let skinned_instance_capacity =
            16u32.min(skinned_mesh_instance_capacity(device.limits().max_buffer_size).max(1));
        let skinned_instance_buffer =
            create_skinned_mesh_instance_buffer(context, skinned_instance_capacity)?;
        let bone_capacity = MAX_SKIN_JOINTS
            .min(bone_matrix_capacity(device.limits().max_storage_buffer_binding_size).max(1));
        let bone_buffer = create_bone_buffer(context, bone_capacity)?;
        let bone_bind_group =
            create_bone_bind_group(context, &bone_bind_group_layout, &bone_buffer);

        Ok(Self {
            pipeline,
            camera_buffer,
            camera_bind_group,
            registry: MeshRegistry::default(),
            gpu_meshes: HashMap::new(),
            texture_registry: TextureRegistry::default(),
            gpu_textures: HashMap::new(),
            default_base_color,
            default_normal,
            default_orm,
            sampler,
            texture_bind_group_layout,
            texture_bind_groups: HashMap::new(),
            instance_buffer,
            instance_capacity,
            color_format,
            msaa,
            msaa_color_view,
            depth_view,
            depth_size: (width.max(1), height.max(1)),
            shadow_pass,
            shadow_bind_group_layout,
            shadow_sampling_bind_group,
            blob_pipeline,
            blob_instance_buffer,
            blob_instance_capacity,
            skinned_pipeline,
            skinned_instance_buffer,
            skinned_instance_capacity,
            bone_bind_group_layout,
            bone_buffer,
            bone_bind_group,
            bone_capacity,
            cluster_pass,
            light_budget: light_budget.light_count(),
        })
    }

    /// Recreates the depth buffer — and, under [`Msaa::X4`] (texture-quality package, strand B1),
    /// the multisampled colour target — for a new colour target size; a no-op if `width`/`height`
    /// match the current depth buffer already.
    pub(crate) fn resize(
        &mut self,
        context: &GpuContext,
        width: u32,
        height: u32,
    ) -> Result<(), GpuError> {
        let size = (width.max(1), height.max(1));
        if size == self.depth_size {
            return Ok(());
        }
        let sample_count = self.msaa.sample_count();
        self.depth_view = create_depth_view(context, width, height, sample_count)?;
        self.msaa_color_view = if sample_count > 1 {
            Some(create_msaa_color_view(
                context,
                self.color_format,
                width,
                height,
                sample_count,
            )?)
        } else {
            None
        };
        self.depth_size = size;
        Ok(())
    }

    /// Validates `mesh`, and if valid, uploads its geometry as static GPU buffers and returns its
    /// deterministically assigned [`MeshHandle`] (contract §6: "the registry itself... is WP2.3's
    /// job"). The registry is left unchanged if either validation or the GPU upload fails, so
    /// handles stay contiguous from `0` across successful registrations only.
    ///
    /// # Errors
    /// [`MeshError`] if `mesh` is structurally invalid ([`MeshData::validate`]) or its buffers
    /// could not be created (for example out of memory).
    pub(crate) fn register(
        &mut self,
        context: &GpuContext,
        mesh: MeshData,
    ) -> Result<MeshHandle, MeshError> {
        let handle = self.registry.register(mesh)?;
        let data = self
            .registry
            .get(handle)
            .expect("just registered above, so it is present");
        let gpu_mesh =
            upload_mesh(context, data).map_err(|error| MeshError::Gpu(error.to_string()))?;
        self.gpu_meshes.insert(handle, gpu_mesh);
        Ok(handle)
    }

    /// Whether `handle` has been registered (and its GPU buffers uploaded) with this pass —
    /// exactly the check [`MeshPass::render`] applies to decide whether to draw a mesh instance.
    /// Also used by [`crate::WgpuRenderer::render_stage`] to fill
    /// [`crate::StageStats::meshes_rejected_unregistered`] (contract §6, PO decision V-20,
    /// 2026-09-16).
    pub(crate) fn is_registered(&self, handle: MeshHandle) -> bool {
        self.gpu_meshes.contains_key(&handle)
    }

    /// Validates `texture`, and if valid, uploads its pixels as a GPU texture and returns its
    /// deterministically assigned [`TextureHandle`] (plan 0002 WP2.5, mirroring
    /// [`MeshPass::register`]). The registry is left unchanged if either validation or the GPU
    /// upload fails.
    ///
    /// # Errors
    /// [`TextureError`] if `texture` is structurally invalid ([`TextureData::validate`]) or its
    /// GPU texture could not be created (for example out of memory).
    pub(crate) fn register_texture(
        &mut self,
        context: &GpuContext,
        texture: TextureData,
    ) -> Result<TextureHandle, TextureError> {
        let handle = self.texture_registry.register(texture)?;
        let data = self
            .texture_registry
            .get(handle)
            .expect("just registered above, so it is present");
        let gpu_texture =
            upload_texture(context, data).map_err(|error| TextureError::Gpu(error.to_string()))?;
        self.gpu_textures.insert(handle, gpu_texture);
        Ok(handle)
    }

    /// Whether `handle` has been registered (and its GPU texture uploaded) with this pass. Unlike
    /// [`MeshPass::is_registered`] this is never a rejection reason: an unregistered texture handle
    /// in a material simply falls back to the default texture (see [`MeshPass::texture_bind_key`]).
    pub(crate) fn is_texture_registered(&self, handle: TextureHandle) -> bool {
        self.gpu_textures.contains_key(&handle)
    }

    /// [`MeshPass::register_texture`], but via [`upload_texture_single_level`] instead of
    /// [`upload_texture`] — see that function's doc comment for why this exists at all
    /// (measurement only, never used by [`MeshPass::register_texture`] itself).
    ///
    /// # Errors
    /// Same as [`MeshPass::register_texture`].
    pub(crate) fn register_texture_single_level(
        &mut self,
        context: &GpuContext,
        texture: TextureData,
    ) -> Result<TextureHandle, TextureError> {
        let handle = self.texture_registry.register(texture)?;
        let data = self
            .texture_registry
            .get(handle)
            .expect("just registered above, so it is present");
        let gpu_texture = upload_texture_single_level(context, data)
            .map_err(|error| TextureError::Gpu(error.to_string()))?;
        self.gpu_textures.insert(handle, gpu_texture);
        Ok(handle)
    }

    /// Resolves `material`'s three optional texture slots to a [`TextureBindKey`]: a handle that
    /// is `None` or not registered with this pass collapses to `None` in the key, so it shares a
    /// bind group (and, for materials that otherwise match, a draw call) with every other material
    /// that also falls back to the default for that slot.
    fn texture_bind_key(&self, material: &PbrMaterial) -> TextureBindKey {
        let resolve = |handle: Option<TextureHandle>| {
            handle.filter(|handle| self.is_texture_registered(*handle))
        };
        (
            resolve(material.base_color_texture),
            resolve(material.normal_texture),
            resolve(material.occlusion_roughness_metallic_texture),
        )
    }

    /// Builds and caches the bind group for `key` if it is not already cached. Bind groups are
    /// never evicted in P1 (bounded by the number of distinct texture combinations a game actually
    /// uses, like [`MeshPass::gpu_meshes`] never evicting a registered mesh).
    fn ensure_texture_bind_group(&mut self, context: &GpuContext, key: TextureBindKey) {
        if self.texture_bind_groups.contains_key(&key) {
            return;
        }
        let (base_key, normal_key, orm_key) = key;
        let base_view = base_key
            .and_then(|handle| self.gpu_textures.get(&handle))
            .map_or(&self.default_base_color.view, |texture| &texture.view);
        let normal_view = normal_key
            .and_then(|handle| self.gpu_textures.get(&handle))
            .map_or(&self.default_normal.view, |texture| &texture.view);
        let orm_view = orm_key
            .and_then(|handle| self.gpu_textures.get(&handle))
            .map_or(&self.default_orm.view, |texture| &texture.view);
        let bind_group = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("grimoire mesh texture bind group"),
                layout: &self.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(base_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(normal_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(orm_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
        self.texture_bind_groups.insert(key, bind_group);
    }

    /// Runs the mesh pass: always clears `view` to `clear_color` and the depth buffer to `1.0`
    /// (even with no usable camera or no drawable instances, so the sprite pass that follows can
    /// unconditionally `LoadOp::Load` afterwards), then draws every [`MeshInstance`] that is
    /// simultaneously: on [`RenderLayer::World`], structurally valid (finite `transform`, a
    /// `material` index pointing at a valid [`PbrMaterial`] in `materials`, a [`MeshInstance::skin`]
    /// that fits `joint_matrices` when present — the same rules `stage::extract_stage3d` already
    /// applies), and whose `mesh` handle is registered with GPU buffers here. Grouped by mesh
    /// handle and resolved texture set (first-seen order) into one `draw_indexed` call per distinct
    /// combination referenced this frame — usually one per mesh, more only when the same mesh is
    /// drawn with materials that resolve to different textures.
    ///
    /// **Skinning (P1 addendum, this module's header doc comment):** an instance with
    /// `skin == Some(binding)` is drawn through the second vertex path (its own pipeline, instance
    /// buffer and group-3 bone matrix storage buffer) instead of the rigid one; `joint_matrices` is
    /// uploaded to that storage buffer verbatim (it is `StageFrame::joint_matrices`, already in the
    /// layout `SkinBinding::joint_offset` indexes into) once per call, regardless of how many
    /// instances reference it. An instance with `skin == None` never touches the skinned pipeline,
    /// its buffers, or an extra draw call.
    ///
    /// `point_lights` are shaded through the same GGX term as the key light, clamped to this
    /// pass's configured [`crate::LightBudget`] and clustered on the GPU (plan 0002 WP3.4, see
    /// this module's doc comment and [`build_light_list`]); `bullet_light_cap` is applied to any
    /// `is_bullet_light` light's uploaded intensity before clustering. `specular_aa` is the OF-3.5
    /// toggle (production rendering always passes `true`, see
    /// [`crate::WgpuRenderer::render_stage_with_specular_aa`]).
    ///
    /// **Shadows (WP2.6):** if `shadow_config` is valid, its mode wants a key-light shadow map,
    /// `key_light` is present and valid, and `camera` is present, this method first (re)configures
    /// and renders [`ShadowPass`] from a light-space view-projection fitted around `camera`'s
    /// ground target ([`stage3d::key_light_view_projection`]), then binds it (group 2) so
    /// `mesh.wgsl` PCF-samples it for the key light's own contribution only. Otherwise the shadow
    /// map render is skipped entirely (no cost on a frame that does not use it) and `mesh.wgsl`'s
    /// `shadow_params.z` tells the shader to skip sampling too. Blob shadows
    /// (`blob_shadows`, filtered to [`crate::BlobShadowInstance::is_valid`]) are drawn, if
    /// `shadow_config`'s mode wants them, in the *same* render pass right after the opaque meshes —
    /// depth-tested against them but never depth-writing (`blob_shadow.wgsl`'s header comment).
    ///
    /// Returns [`MeshPassStats`]: the number of draw calls issued across every sub-pass this call
    /// touches (`0` if there is no usable camera or nothing to draw, for
    /// [`crate::StageStats::base`]'s `draw_calls`, contract §6: "all passes"), how many valid
    /// point lights this frame exceeded the configured light budget (WP3.4), and the clustered
    /// forward+ pass's own frame stats.
    ///
    /// # Errors
    /// [`GpuError`] if uploading the per-frame uniform/instance data or submitting a render pass
    /// fails (for example out of memory).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &mut self,
        context: &GpuContext,
        view: &wgpu::TextureView,
        clear_color: wgpu::Color,
        aspect: f32,
        camera: Option<&Camera25D>,
        key_light: Option<&DirectionalLight>,
        ambient: &AmbientLight,
        point_lights: &[PointLight],
        bullet_light_cap: &BulletLightCap,
        meshes: &[MeshInstance],
        materials: &[PbrMaterial],
        specular_aa: bool,
        shadow_config: &ShadowConfig,
        blob_shadows: &[BlobShadowInstance],
        joint_matrices: &[[[f32; 4]; 4]],
    ) -> Result<MeshPassStats, GpuError> {
        let view_proj =
            camera.map(|camera| stage3d::view_projection(camera, aspect, NEAR_PLANE, FAR_PLANE));
        let usable_view_proj =
            view_proj.filter(|matrix| matrix.iter().flatten().all(|c| c.is_finite()));

        let mut order: Vec<(MeshHandle, TextureBindKey)> = Vec::new();
        let mut groups: HashMap<(MeshHandle, TextureBindKey), Vec<MeshInstanceGpu>> =
            HashMap::new();
        let mut skinned_order: Vec<(MeshHandle, TextureBindKey)> = Vec::new();
        let mut skinned_groups: HashMap<(MeshHandle, TextureBindKey), Vec<SkinnedMeshInstanceGpu>> =
            HashMap::new();
        if usable_view_proj.is_some() {
            for mesh in meshes {
                if !self.is_drawable(mesh, materials, joint_matrices.len()) {
                    continue;
                }
                let material = &materials[mesh.material.0 as usize];
                let key = (mesh.mesh, self.texture_bind_key(material));
                let emissive = [
                    material.emissive_factor[0],
                    material.emissive_factor[1],
                    material.emissive_factor[2],
                    0.0,
                ];
                let material_params = [
                    material.metallic_factor,
                    material.roughness_factor,
                    0.0,
                    0.0,
                ];
                if let Some(skin) = mesh.skin {
                    skinned_groups
                        .entry(key)
                        .or_insert_with(|| {
                            skinned_order.push(key);
                            Vec::new()
                        })
                        .push(SkinnedMeshInstanceGpu {
                            transform: mesh.transform,
                            base_color: material.base_color_factor,
                            emissive,
                            material_params,
                            bone_offset: skin.joint_offset,
                            _pad: [0; 3],
                        });
                } else {
                    groups
                        .entry(key)
                        .or_insert_with(|| {
                            order.push(key);
                            Vec::new()
                        })
                        .push(MeshInstanceGpu {
                            transform: mesh.transform,
                            base_color: material.base_color_factor,
                            emissive,
                            material_params,
                        });
                }
            }
        }

        for &(_, texture_key) in order.iter().chain(skinned_order.iter()) {
            self.ensure_texture_bind_group(context, texture_key);
        }

        let mut instances = Vec::new();
        let mut draw_ranges: Vec<((MeshHandle, TextureBindKey), u32, u32)> = Vec::new();
        for key in &order {
            let group = &groups[key];
            let start = u32::try_from(instances.len()).unwrap_or(u32::MAX);
            let len = u32::try_from(group.len()).unwrap_or(u32::MAX);
            draw_ranges.push((*key, start, len));
            instances.extend_from_slice(group);
        }

        let mut skinned_instances = Vec::new();
        let mut skinned_draw_ranges: Vec<((MeshHandle, TextureBindKey), u32, u32)> = Vec::new();
        for key in &skinned_order {
            let group = &skinned_groups[key];
            let start = u32::try_from(skinned_instances.len()).unwrap_or(u32::MAX);
            let len = u32::try_from(group.len()).unwrap_or(u32::MAX);
            skinned_draw_ranges.push((*key, start, len));
            skinned_instances.extend_from_slice(group);
        }

        let eye = camera.map_or([0.0; 3], stage3d::eye_position);
        let (gpu_lights, point_lights_over_budget) =
            build_light_list(point_lights, self.light_budget, bullet_light_cap);
        if point_lights_over_budget > 0 {
            let total_valid = point_lights_over_budget + gpu_lights.len();
            log::warn!(
                "point light budget exceeded: {total_valid} valid point lights this frame, clustering only the first {} (crate::LightBudget, contract §6)",
                self.light_budget
            );
        }
        // Clustered forward+ camera basis (plan 0002 WP3.4): the same values, computed once, feed
        // both `mesh.wgsl`'s uniform below and the compute dispatch further down, so a fragment's
        // froxel and a light's froxel agree by construction. Degenerate (but finite) when there is
        // no usable camera this frame — nothing is drawn in that case (`usable_view_proj.is_none()`
        // already empties `order`/`skinned_order` above), so which froxel a light lands in cannot
        // affect the picture.
        let cluster_camera = match (camera, usable_view_proj) {
            (Some(camera), Some(_)) => {
                stage3d::cluster_camera_params(camera, aspect, NEAR_PLANE, FAR_PLANE)
            }
            _ => ClusterCameraParams::degenerate(NEAR_PLANE, FAR_PLANE),
        };
        let cluster_stats = self
            .cluster_pass
            .dispatch(context, &gpu_lights, &cluster_camera)?;

        // Key-light shadow map (WP2.6): skipped entirely (no `configure`/render cost) unless the
        // mode wants it and there is a valid key light and camera to fit it around.
        let key_light_valid = key_light.is_some_and(DirectionalLight::is_valid);
        let want_shadow_map = shadow_config.is_valid()
            && shadow_config.mode.wants_key_light_shadow_map()
            && key_light_valid
            && camera.is_some();
        let mut light_view_proj = IDENTITY;
        let mut shadow_draw_calls = 0u32;
        if want_shadow_map {
            let light = key_light.expect("key_light_valid implies Some");
            let camera_ref = camera.expect("want_shadow_map implies Some");
            self.shadow_pass.configure(context, shadow_config)?;
            self.shadow_sampling_bind_group = create_shadow_sampling_bind_group(
                context,
                &self.shadow_bind_group_layout,
                &self.shadow_pass,
            );
            light_view_proj = stage3d::key_light_view_projection(
                light.direction,
                camera_ref.target,
                shadow_config,
            );
            shadow_draw_calls = self.render_shadow_map(
                context,
                light_view_proj,
                meshes,
                materials,
                joint_matrices.len(),
            )?;
        }
        let shadow_params = [
            1.0 / self.shadow_pass.map_size() as f32,
            shadow_config.pcf_radius as f32,
            if want_shadow_map { 1.0 } else { 0.0 },
            0.0,
        ];

        let uniform = camera_uniform(
            usable_view_proj.unwrap_or(IDENTITY),
            eye,
            key_light,
            ambient,
            &cluster_camera,
            specular_aa,
            light_view_proj,
            shadow_params,
        );
        context
            .queue()
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));

        // Blob shadows (WP2.6): drawn in the same render pass as the opaque meshes below, right
        // after them, so the depth buffer they already wrote occludes a disc behind a wall or lets
        // an actor standing on one draw over it (`blob_shadow.wgsl`'s header comment).
        let blob_instances: Vec<BlobShadowInstance> =
            if shadow_config.mode.wants_blob_shadows() && usable_view_proj.is_some() {
                blob_shadows
                    .iter()
                    .copied()
                    .filter(BlobShadowInstance::is_valid)
                    .collect()
            } else {
                Vec::new()
            };
        let blob_count = u32::try_from(blob_instances.len()).unwrap_or(u32::MAX);
        if blob_count > 0 {
            let max_buffer_size = context.device().limits().max_buffer_size;
            let capacity = crate::sprite_pass::grown_capacity(
                self.blob_instance_capacity,
                blob_count,
                blob_instance_capacity(max_buffer_size),
            )
            .ok_or_else(|| {
                GpuError::Validation(format!(
                    "{blob_count} blob shadows exceed the device buffer limit of {max_buffer_size} bytes"
                ))
            })?;
            if capacity != self.blob_instance_capacity {
                self.blob_instance_buffer = create_blob_instance_buffer(context, capacity)?;
                self.blob_instance_capacity = capacity;
            }
            context.queue().write_buffer(
                &self.blob_instance_buffer,
                0,
                bytemuck::cast_slice(blob_instances.as_slice()),
            );
        }

        let count = u32::try_from(instances.len()).map_err(|_| {
            GpuError::Validation(format!(
                "{} mesh instances exceed u32::MAX",
                instances.len()
            ))
        })?;
        if count > 0 {
            let max_buffer_size = context.device().limits().max_buffer_size;
            let capacity = crate::sprite_pass::grown_capacity(
                self.instance_capacity,
                count,
                mesh_instance_capacity(max_buffer_size),
            )
            .ok_or_else(|| {
                GpuError::Validation(format!(
                    "{count} mesh instances exceed the device buffer limit of {max_buffer_size} bytes"
                ))
            })?;
            if capacity != self.instance_capacity {
                self.instance_buffer = create_mesh_instance_buffer(context, capacity)?;
                self.instance_capacity = capacity;
            }
            context.queue().write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(instances.as_slice()),
            );
        }

        // Skinned instance buffer (P1 addendum): same growth pattern as `instance_buffer` above,
        // sized for the larger `SkinnedMeshInstanceGpu` layout.
        let skinned_count = u32::try_from(skinned_instances.len()).map_err(|_| {
            GpuError::Validation(format!(
                "{} skinned mesh instances exceed u32::MAX",
                skinned_instances.len()
            ))
        })?;
        if skinned_count > 0 {
            let max_buffer_size = context.device().limits().max_buffer_size;
            let capacity = crate::sprite_pass::grown_capacity(
                self.skinned_instance_capacity,
                skinned_count,
                skinned_mesh_instance_capacity(max_buffer_size),
            )
            .ok_or_else(|| {
                GpuError::Validation(format!(
                    "{skinned_count} skinned mesh instances exceed the device buffer limit of {max_buffer_size} bytes"
                ))
            })?;
            if capacity != self.skinned_instance_capacity {
                self.skinned_instance_buffer =
                    create_skinned_mesh_instance_buffer(context, capacity)?;
                self.skinned_instance_capacity = capacity;
            }
            context.queue().write_buffer(
                &self.skinned_instance_buffer,
                0,
                bytemuck::cast_slice(skinned_instances.as_slice()),
            );
        }

        // Bone matrix storage buffer (P1 addendum, group 3): uploaded verbatim whenever the frame
        // carries any joint matrices at all, independent of `skinned_count` — cheap (a frame with a
        // non-empty `StageFrame::joint_matrices` but zero *drawable* skinned instances this frame is
        // an edge case, not worth a second condition to special-case away).
        let bone_count = u32::try_from(joint_matrices.len()).unwrap_or(u32::MAX);
        if bone_count > 0 {
            let max_binding_size = context.device().limits().max_storage_buffer_binding_size;
            let capacity = crate::sprite_pass::grown_capacity(
                self.bone_capacity,
                bone_count,
                bone_matrix_capacity(max_binding_size),
            )
            .ok_or_else(|| {
                GpuError::Validation(format!(
                    "{bone_count} bone matrices exceed the device storage-buffer binding limit of {max_binding_size} bytes"
                ))
            })?;
            if capacity != self.bone_capacity {
                self.bone_buffer = create_bone_buffer(context, capacity)?;
                self.bone_bind_group = create_bone_bind_group(
                    context,
                    &self.bone_bind_group_layout,
                    &self.bone_buffer,
                );
                self.bone_capacity = capacity;
            }
            context.queue().write_buffer(
                &self.bone_buffer,
                0,
                bytemuck::cast_slice(joint_matrices),
            );
        }

        let mesh_draw_calls = if count > 0 {
            u32::try_from(draw_ranges.len()).unwrap_or(u32::MAX)
        } else {
            0
        };
        let skinned_draw_calls = if skinned_count > 0 {
            u32::try_from(skinned_draw_ranges.len()).unwrap_or(u32::MAX)
        } else {
            0
        };
        let blob_draw_calls = u32::from(blob_count > 0);
        let draw_calls = shadow_draw_calls + mesh_draw_calls + skinned_draw_calls + blob_draw_calls;

        let gpu_meshes = &self.gpu_meshes;
        let texture_bind_groups = &self.texture_bind_groups;
        let pipeline = &self.pipeline;
        let camera_bind_group = &self.camera_bind_group;
        let shadow_sampling_bind_group = &self.shadow_sampling_bind_group;
        let instance_buffer = &self.instance_buffer;
        let depth_view = &self.depth_view;
        // Multisampled colour target (texture-quality package, strand B1): under `Msaa::X4`, the
        // pass renders into `msaa_color_view` and resolves into the caller-provided `view`
        // (`resolve_target`); the multisampled attachment itself is discarded afterwards (only the
        // resolved image is ever read back or presented). Under `Msaa::Off` this is exactly the
        // pre-B1 render pass: straight into `view`, no resolve.
        let (color_view, resolve_target, color_store) = match self.msaa_color_view.as_ref() {
            Some(msaa_view) => (msaa_view, Some(view), wgpu::StoreOp::Discard),
            None => (view, None, wgpu::StoreOp::Store),
        };
        let blob_pipeline = &self.blob_pipeline;
        let skinned_pipeline = &self.skinned_pipeline;
        let skinned_instance_buffer = &self.skinned_instance_buffer;
        let bone_bind_group = &self.bone_bind_group;
        let blob_instance_buffer = &self.blob_instance_buffer;
        // Clustered forward+ (plan 0002 WP3.4): group 4, identical for the rigid and skinned
        // pipelines below (both share `mesh.wgsl`'s `fs_main`), already refreshed by
        // `self.cluster_pass.dispatch` above this frame.
        let cluster_fragment_bind_group = self.cluster_pass.fragment_bind_group();
        context.capture_errors(move |device| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("grimoire mesh encoder"),
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("grimoire mesh pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: color_view,
                        depth_slice: None,
                        resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear_color),
                            store: color_store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                if count > 0 {
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, camera_bind_group, &[]);
                    pass.set_bind_group(2, shadow_sampling_bind_group, &[]);
                    pass.set_bind_group(4, cluster_fragment_bind_group, &[]);
                    pass.set_vertex_buffer(
                        1,
                        instance_buffer.slice(..u64::from(count) * MESH_INSTANCE_SIZE),
                    );
                    for ((mesh_handle, texture_key), start, len) in &draw_ranges {
                        let gpu_mesh = &gpu_meshes[mesh_handle];
                        pass.set_bind_group(1, &texture_bind_groups[texture_key], &[]);
                        pass.set_vertex_buffer(0, gpu_mesh.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            gpu_mesh.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..gpu_mesh.index_count, 0, *start..(*start + *len));
                    }
                }
                // Skinned meshes (P1 addendum), same pass, right after the rigid ones: same depth
                // buffer, same camera and shadow-sampling bind groups (0, 2), but the skinning
                // pipeline, its own instance buffer, and the bone matrix storage buffer (group 3)
                // instead. Draw order relative to the rigid meshes does not matter (the depth test
                // decides occlusion either way, exactly like the blob-shadow ordering note below).
                if skinned_count > 0 {
                    pass.set_pipeline(skinned_pipeline);
                    pass.set_bind_group(0, camera_bind_group, &[]);
                    pass.set_bind_group(2, shadow_sampling_bind_group, &[]);
                    pass.set_bind_group(3, bone_bind_group, &[]);
                    pass.set_bind_group(4, cluster_fragment_bind_group, &[]);
                    pass.set_vertex_buffer(
                        1,
                        skinned_instance_buffer
                            .slice(..u64::from(skinned_count) * SKINNED_MESH_INSTANCE_SIZE),
                    );
                    for ((mesh_handle, texture_key), start, len) in &skinned_draw_ranges {
                        let gpu_mesh = &gpu_meshes[mesh_handle];
                        pass.set_bind_group(1, &texture_bind_groups[texture_key], &[]);
                        pass.set_vertex_buffer(0, gpu_mesh.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            gpu_mesh.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..gpu_mesh.index_count, 0, *start..(*start + *len));
                    }
                }
                // Blob shadows (WP2.6), same pass, right after the opaque meshes: see this
                // method's doc comment and `blob_shadow.wgsl`'s header comment for why draw order
                // relative to the opaque meshes does not matter (the depth test already decides
                // occlusion correctly either way).
                if blob_count > 0 {
                    pass.set_pipeline(blob_pipeline);
                    pass.set_bind_group(0, camera_bind_group, &[]);
                    pass.set_vertex_buffer(
                        0,
                        blob_instance_buffer.slice(
                            ..u64::from(blob_count)
                                * std::mem::size_of::<BlobShadowInstance>() as u64,
                        ),
                    );
                    pass.draw(0..6, 0..blob_count);
                }
            }
            context.queue().submit([encoder.finish()]);
        })?;

        Ok(MeshPassStats {
            draw_calls,
            cluster: cluster_stats,
        })
    }

    /// Whether `mesh` is drawable this frame under contract §6's shared mesh acceptance rules:
    /// `layer == RenderLayer::World`, a finite `transform`, a `material` index pointing at a valid
    /// [`PbrMaterial`], a [`MeshInstance::skin`] that is either absent or fits inside a
    /// `joint_matrices` table of `joint_matrices_len` entries (P1 skinning addendum; mirrors
    /// `crate::stage::skin_binding_is_valid`'s arithmetic, duplicated for the same reason as the
    /// rest of this predicate), and `mesh` registered with this pass. Exactly the predicate
    /// `crate::stage::extract_stage3d` applies for [`crate::StageStats::meshes_drawn`] (and, from
    /// WP2.6, [`crate::StageStats::shadow_casters_drawn`]) — duplicated here, not shared code,
    /// because this module additionally needs the registered [`GpuMesh`] itself, which
    /// `crate::stage` never sees (contract §6: only the boolean registration check crosses that
    /// boundary).
    fn is_drawable(
        &self,
        mesh: &MeshInstance,
        materials: &[PbrMaterial],
        joint_matrices_len: usize,
    ) -> bool {
        mesh.layer == RenderLayer::World
            && mesh.transform.iter().flatten().all(|c| c.is_finite())
            && materials
                .get(mesh.material.0 as usize)
                .is_some_and(PbrMaterial::is_valid)
            && mesh
                .skin
                .is_none_or(|skin| skin_binding_fits(skin, joint_matrices_len))
            && self.is_registered(mesh.mesh)
    }

    /// Renders every drawable, **unskinned** mesh instance's transform
    /// ([`MeshPass::is_drawable`]) into the key-light shadow map from `light_view_proj` (plan 0002
    /// WP2.6). A skinned instance (`MeshInstance::skin.is_some()`) is deliberately excluded: this
    /// package casts a shadow from each mesh's static rest-pose geometry, and drawing that shape
    /// for a bent, animated figure would read as more wrong than no shadow at all. Consequently
    /// this is *not quite* the set [`crate::StageStats::shadow_casters_drawn`] counts once a frame
    /// contains skinned instances — that counter still equals `meshes_drawn` (contract §6
    /// unchanged), a known, documented gap left for a future work package (correct skinned shadow
    /// casting needs the same skin matrix the vertex shader applies, run for depth-only output
    /// too). Returns the number of draw calls issued.
    ///
    /// Builds the caster list in two steps — first collecting `(handle, transform)` pairs with
    /// [`MeshPass::is_drawable`] (which needs `&self` as a whole), then resolving each handle's
    /// GPU buffers via a direct `&self.gpu_meshes` field borrow — so the borrow checker can see the
    /// second step's borrow is disjoint from the later `&mut self.shadow_pass` call, instead of one
    /// long-lived whole-`self` borrow blocking it.
    ///
    /// # Errors
    /// [`GpuError`] if uploading the per-frame uniform/instance data or submitting the render pass
    /// fails (for example out of memory).
    fn render_shadow_map(
        &mut self,
        context: &GpuContext,
        light_view_proj: [[f32; 4]; 4],
        meshes: &[MeshInstance],
        materials: &[PbrMaterial],
        joint_matrices_len: usize,
    ) -> Result<u32, GpuError> {
        let accepted: Vec<(MeshHandle, [[f32; 4]; 4])> = meshes
            .iter()
            .filter(|mesh| {
                mesh.skin.is_none() && self.is_drawable(mesh, materials, joint_matrices_len)
            })
            .map(|mesh| (mesh.mesh, mesh.transform))
            .collect();
        let gpu_meshes = &self.gpu_meshes;
        let casters: Vec<ShadowCaster<'_>> = accepted
            .iter()
            .map(|&(handle, transform)| {
                let gpu_mesh = &gpu_meshes[&handle];
                ShadowCaster {
                    vertex_buffer: &gpu_mesh.vertex_buffer,
                    index_buffer: &gpu_mesh.index_buffer,
                    index_count: gpu_mesh.index_count,
                    transform,
                }
            })
            .collect();
        self.shadow_pass.render(context, light_view_proj, &casters)
    }
}

#[cfg(test)]
mod tests {
    use std::mem::offset_of;

    use super::*;
    use crate::cluster_layout;
    use crate::mesh::MeshVertex;

    #[test]
    fn mesh_instance_gpu_is_112_bytes() {
        assert_eq!(std::mem::size_of::<MeshInstanceGpu>(), 112);
    }

    #[test]
    fn camera_gpu_is_304_bytes() {
        // WP3.4 dropped the fixed 32-light array (1024 bytes) and `light_count` (4 bytes) in
        // favour of group 4's storage buffers, adding the camera-local basis and clustering
        // projection scalars (4 `vec4`s, 64 bytes) plus one more `u32` pad (keeping the
        // `specular_aa_strength` block a 16-byte multiple): 1264 - 1024 - 4 + 64 + 4 = 304.
        assert_eq!(std::mem::size_of::<CameraGpu>(), 304);
    }

    #[test]
    fn vertex_attributes_match_mesh_vertex_layout() {
        // The rigid pipeline's vertex attributes: position, normal, uv only. Since P1's skinning
        // addendum grew `MeshVertex` with trailing `joints`/`weights`, this no longer covers the
        // whole struct (see `skinned_vertex_attributes_match_mesh_vertex_layout` for those two) —
        // deliberately: an instance with `skin == None` reads exactly these three fields and never
        // touches the rest, which is what "costs nothing extra" means for the rigid path.
        let attributes: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x3,
            2 => Float32x2,
        ];
        let offsets = [
            offset_of!(MeshVertex, position),
            offset_of!(MeshVertex, normal),
            offset_of!(MeshVertex, uv),
        ];
        for (attribute, offset) in attributes.iter().zip(offsets) {
            assert_eq!(attribute.offset, offset as u64);
        }
    }

    #[test]
    fn skinned_vertex_attributes_match_mesh_vertex_layout() {
        let attributes: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x3,
            2 => Float32x2,
            3 => Uint16x4,
            4 => Float32x4,
        ];
        let offsets = [
            offset_of!(MeshVertex, position),
            offset_of!(MeshVertex, normal),
            offset_of!(MeshVertex, uv),
            offset_of!(MeshVertex, joints),
            offset_of!(MeshVertex, weights),
        ];
        for (attribute, offset) in attributes.iter().zip(offsets) {
            assert_eq!(attribute.offset, offset as u64);
        }
        let last = &attributes[4];
        assert_eq!(
            last.offset + last.format.size(),
            std::mem::size_of::<MeshVertex>() as u64,
            "the skinning path reads every byte of MeshVertex, unlike the rigid path"
        );
    }

    #[test]
    fn skinned_mesh_instance_gpu_is_128_bytes() {
        assert_eq!(std::mem::size_of::<SkinnedMeshInstanceGpu>(), 128);
    }

    #[test]
    fn skinned_instance_attributes_match_skinned_mesh_instance_gpu_layout() {
        let attributes: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
            5 => Float32x4,
            6 => Float32x4,
            7 => Float32x4,
            8 => Float32x4,
            9 => Float32x4,
            10 => Float32x4,
            11 => Float32x4,
            12 => Uint32,
        ];
        // The four transform columns are contiguous 16-byte chunks of `transform: [[f32; 4]; 4]`,
        // immediately followed by `base_color`, `emissive`, `material_params` and `bone_offset`.
        let offsets = [
            0,
            16,
            32,
            48,
            offset_of!(SkinnedMeshInstanceGpu, base_color),
            offset_of!(SkinnedMeshInstanceGpu, emissive),
            offset_of!(SkinnedMeshInstanceGpu, material_params),
            offset_of!(SkinnedMeshInstanceGpu, bone_offset),
        ];
        for (attribute, offset) in attributes.iter().zip(offsets) {
            assert_eq!(attribute.offset, offset as u64);
        }
    }

    #[test]
    fn skinned_mesh_instance_capacity_matches_buffer_size() {
        assert_eq!(
            skinned_mesh_instance_capacity(SKINNED_MESH_INSTANCE_SIZE * 10),
            10
        );
        assert_eq!(skinned_mesh_instance_capacity(u64::MAX), u32::MAX);
    }

    #[test]
    fn bone_matrix_capacity_matches_buffer_size() {
        assert_eq!(bone_matrix_capacity(BONE_MATRIX_SIZE * 10), 10);
        assert_eq!(bone_matrix_capacity(u64::MAX), u32::MAX);
    }

    #[test]
    fn skin_binding_fits_checks_range_and_joint_count() {
        let binding = SkinBinding {
            joint_offset: 2,
            joint_count: 3,
        };
        assert!(skin_binding_fits(binding, 5));
        assert!(!skin_binding_fits(binding, 4), "range reaches past the end");
        assert!(!skin_binding_fits(
            SkinBinding {
                joint_offset: 0,
                joint_count: 0,
            },
            5
        ));
        assert!(!skin_binding_fits(
            SkinBinding {
                joint_offset: 0,
                joint_count: MAX_SKIN_JOINTS + 1,
            },
            usize::MAX
        ));
        assert!(!skin_binding_fits(
            SkinBinding {
                joint_offset: u32::MAX,
                joint_count: 1,
            },
            usize::MAX
        ));
    }

    #[test]
    fn instance_attributes_match_mesh_instance_gpu_layout() {
        let attributes: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
            3 => Float32x4,
            4 => Float32x4,
            5 => Float32x4,
            6 => Float32x4,
            7 => Float32x4,
            8 => Float32x4,
            9 => Float32x4,
        ];
        // The four transform columns are contiguous 16-byte chunks of `transform: [[f32; 4]; 4]`,
        // immediately followed by `base_color`, `emissive` and `material_params`.
        let offsets = [
            0,
            16,
            32,
            48,
            offset_of!(MeshInstanceGpu, base_color),
            offset_of!(MeshInstanceGpu, emissive),
            offset_of!(MeshInstanceGpu, material_params),
        ];
        for (attribute, offset) in attributes.iter().zip(offsets) {
            assert_eq!(attribute.offset, offset as u64);
        }
        let last = &attributes[6];
        assert_eq!(last.offset + last.format.size(), MESH_INSTANCE_SIZE);
    }

    #[test]
    fn mesh_instance_capacity_matches_buffer_size() {
        assert_eq!(mesh_instance_capacity(MESH_INSTANCE_SIZE * 10), 10);
        assert_eq!(mesh_instance_capacity(u64::MAX), u32::MAX);
    }

    /// Shared stand-in camera basis for `camera_uniform` tests below that do not care about the
    /// WP3.4 clustering fields themselves.
    fn test_cluster_camera() -> ClusterCameraParams {
        ClusterCameraParams::degenerate(NEAR_PLANE, FAR_PLANE)
    }

    #[test]
    fn camera_uniform_falls_back_to_no_light_or_ambient_when_invalid() {
        let uniform = camera_uniform(
            IDENTITY,
            [0.0, 0.0, 0.0],
            Some(&DirectionalLight {
                // Zero-length direction: DirectionalLight::is_valid() rejects it.
                direction: [0.0, 0.0, 0.0],
                ..DirectionalLight::default()
            }),
            &AmbientLight::Flat {
                color: [1.0, 1.0, 1.0],
                intensity: -1.0, // AmbientLight::is_valid() rejects a negative intensity.
            },
            &test_cluster_camera(),
            true,
            IDENTITY,
            [0.0; 4],
        );
        assert_eq!(uniform.light_dir, [0.0; 4], "no key light direction");
        assert_eq!(uniform.key_light, [0.0; 4], "no key light colour");
        assert_eq!(uniform.ambient_sky, [0.0; 4], "invalid ambient: sky zero");
        assert_eq!(
            uniform.ambient_ground, [0.0; 4],
            "invalid ambient: ground zero"
        );
    }

    #[test]
    fn camera_uniform_uses_ambient_flat_for_both_sky_and_ground() {
        // Exactly representable in binary floating point, so the assertion below needs no
        // tolerance.
        let color = [0.25, 0.5, 0.75];
        let uniform = camera_uniform(
            IDENTITY,
            [0.0, 0.0, 0.0],
            None,
            &AmbientLight::Flat {
                color,
                intensity: 1.0,
            },
            &test_cluster_camera(),
            true,
            IDENTITY,
            [0.0; 4],
        );
        assert_eq!(&uniform.ambient_sky[..3], color, "sky");
        assert_eq!(
            &uniform.ambient_ground[..3],
            color,
            "ground, same as sky for AmbientLight::Flat"
        );
    }

    #[test]
    fn camera_uniform_points_light_dir_towards_the_light() {
        let uniform = camera_uniform(
            IDENTITY,
            [0.0, 0.0, 0.0],
            Some(&DirectionalLight {
                direction: [0.0, 0.0, -1.0], // light travels straight down
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
            }),
            &AmbientLight::Flat {
                color: [0.0, 0.0, 0.0],
                intensity: 0.0,
            },
            &test_cluster_camera(),
            true,
            IDENTITY,
            [0.0; 4],
        );
        // `light_dir` points *from a surface towards the light*: the negated travel direction.
        assert_eq!(&uniform.light_dir[..3], [0.0, 0.0, 1.0]);
        assert_eq!(&uniform.key_light[..3], [1.0, 1.0, 1.0]);
    }

    #[test]
    fn camera_uniform_copies_view_projection_column_major() {
        let matrix = [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0, 16.0],
        ];
        let uniform = camera_uniform(
            matrix,
            [0.0, 0.0, 0.0],
            None,
            &AmbientLight::Flat {
                color: [0.0, 0.0, 0.0],
                intensity: 0.0,
            },
            &test_cluster_camera(),
            true,
            IDENTITY,
            [0.0; 4],
        );
        assert_eq!(uniform.view_proj, matrix);
    }

    #[test]
    fn camera_uniform_carries_the_eye_position_and_specular_aa_toggle() {
        let uniform = camera_uniform(
            IDENTITY,
            [1.0, 2.0, 3.0],
            None,
            &AmbientLight::default(),
            &test_cluster_camera(),
            false,
            IDENTITY,
            [0.0; 4],
        );
        assert_eq!(&uniform.eye[..3], [1.0, 2.0, 3.0]);
        assert_eq!(uniform.specular_aa_strength, 0.0);
    }

    #[test]
    fn camera_uniform_carries_the_shadow_matrix_and_params_through() {
        let light_view_proj = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 3.0, 0.0],
            [4.0, 5.0, 6.0, 1.0],
        ];
        let shadow_params = [1.0 / 1024.0, 1.0, 1.0, 0.0];
        let uniform = camera_uniform(
            IDENTITY,
            [0.0, 0.0, 0.0],
            None,
            &AmbientLight::default(),
            &test_cluster_camera(),
            true,
            light_view_proj,
            shadow_params,
        );
        assert_eq!(uniform.light_view_proj, light_view_proj);
        assert_eq!(uniform.shadow_params, shadow_params);
    }

    #[test]
    fn camera_uniform_carries_the_cluster_camera_basis_through() {
        let cluster_camera = ClusterCameraParams {
            eye: [1.0, 2.0, 3.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            forward: [0.0, 1.0, 0.0],
            f_over_aspect: 1.5,
            f: 2.0,
            near: 0.1,
            far: 500.0,
        };
        let uniform = camera_uniform(
            IDENTITY,
            [0.0, 0.0, 0.0],
            None,
            &AmbientLight::default(),
            &cluster_camera,
            true,
            IDENTITY,
            [0.0; 4],
        );
        assert_eq!(&uniform.right[..3], [1.0, 0.0, 0.0]);
        assert_eq!(&uniform.up[..3], [0.0, 0.0, 1.0]);
        assert_eq!(&uniform.forward[..3], [0.0, 1.0, 0.0]);
        assert_eq!(uniform.cluster_proj, [1.5, 2.0, 0.1, 500.0]);
    }

    // --- clamp_light_budget / build_light_list (WP2.5, WP3.4) --------------------------------

    #[test]
    fn clamp_light_budget_keeps_everything_under_budget() {
        let items = [1, 2, 3];
        let (kept, dropped) = clamp_light_budget(&items, 5);
        assert_eq!(kept, &items);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn clamp_light_budget_keeps_exactly_the_budget_at_the_boundary() {
        let items = [1, 2, 3];
        let (kept, dropped) = clamp_light_budget(&items, 3);
        assert_eq!(kept, &items);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn clamp_light_budget_drops_the_excess_keeping_frame_order() {
        let items = [1, 2, 3, 4, 5];
        let (kept, dropped) = clamp_light_budget(&items, 2);
        assert_eq!(kept, &[1, 2]);
        assert_eq!(dropped, 3);
    }

    #[test]
    fn clamp_light_budget_of_empty_slice_never_panics() {
        let items: [i32; 0] = [];
        let (kept, dropped) = clamp_light_budget(&items, 0);
        assert!(kept.is_empty());
        assert_eq!(dropped, 0);
    }

    fn light_at(x: f32) -> PointLight {
        PointLight {
            position: [x, 0.0, 0.0],
            ..PointLight::default()
        }
    }

    #[test]
    fn build_light_list_drops_invalid_lights_without_counting_them_as_budget_drops() {
        let lights = vec![
            light_at(1.0),
            PointLight {
                intensity: f32::NAN,
                ..light_at(2.0)
            },
            light_at(3.0),
        ];
        let (gpu_lights, dropped) = build_light_list(&lights, 32, &BulletLightCap::default());
        assert_eq!(
            gpu_lights.len(),
            2,
            "the NaN-intensity light is invalid, not budget-dropped"
        );
        assert_eq!(dropped, 0);
        assert_eq!(gpu_lights[0].position, [1.0, 0.0, 0.0]);
        assert_eq!(gpu_lights[1].position, [3.0, 0.0, 0.0]);
    }

    #[test]
    fn build_light_list_clamps_to_the_configured_budget_and_reports_the_rest_dropped() {
        let lights: Vec<PointLight> = (0..37).map(|i| light_at(i as f32)).collect();
        let (gpu_lights, dropped) = build_light_list(&lights, 32, &BulletLightCap::default());
        assert_eq!(gpu_lights.len(), 32);
        assert_eq!(dropped, 5);
        assert_eq!(gpu_lights[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(gpu_lights[31].position, [31.0, 0.0, 0.0]);
    }

    #[test]
    fn build_light_list_handles_the_full_high_budget_of_256_lights() {
        // Plan 0002 WP3.4 / PRD-0003 FR-11's "High 256" preset: every one of 256 valid lights
        // survives (frame order, nothing dropped) once the budget is configured to match —
        // demonstrating this layer actually carries the full contract budget through, not just
        // the pre-WP3.4 internal 32-light limit `mesh.wgsl`'s old fixed array capped at.
        let lights: Vec<PointLight> = (0..cluster_layout::LIGHT_BUDGET_HIGH)
            .map(|i| light_at(i as f32))
            .collect();
        let (gpu_lights, dropped) = build_light_list(
            &lights,
            cluster_layout::LIGHT_BUDGET_HIGH,
            &BulletLightCap::default(),
        );
        assert_eq!(gpu_lights.len(), cluster_layout::LIGHT_BUDGET_HIGH);
        assert_eq!(dropped, 0);
        assert_eq!(
            gpu_lights[cluster_layout::LIGHT_BUDGET_HIGH - 1].position,
            [(cluster_layout::LIGHT_BUDGET_HIGH - 1) as f32, 0.0, 0.0]
        );
    }

    #[test]
    fn build_light_list_keeps_color_and_intensity_separate() {
        // Unlike the pre-WP3.4 fixed array (which pre-multiplied colour by intensity for
        // `mesh.wgsl`'s old uniform layout), `cluster_layout::GpuPointLight` carries
        // `PointLight::intensity` verbatim (its own doc comment) so the bullet-light cap below
        // can scale intensity alone.
        let lights = [PointLight {
            color: [0.5, 0.25, 1.0],
            intensity: 2.0,
            ..PointLight::default()
        }];
        let (gpu_lights, _) = build_light_list(&lights, 32, &BulletLightCap::default());
        assert_eq!(gpu_lights.len(), 1);
        assert_eq!(gpu_lights[0].color, [0.5, 0.25, 1.0]);
        assert_eq!(gpu_lights[0].intensity, 2.0);
    }

    #[test]
    fn build_light_list_applies_the_bullet_light_cap_only_to_bullet_lights() {
        // PRD-0003 rule 5 / FR-15, PO decision 2026-09-16: `is_bullet_light` lights have their
        // uploaded intensity scaled by `BulletLightCap::clamped_floor_contribution`; every other
        // light is unaffected.
        let lights = [
            PointLight {
                intensity: 4.0,
                is_bullet_light: true,
                ..light_at(1.0)
            },
            PointLight {
                intensity: 4.0,
                is_bullet_light: false,
                ..light_at(2.0)
            },
        ];
        let cap = BulletLightCap {
            floor_contribution: 0.25,
        };
        let (gpu_lights, _) = build_light_list(&lights, 32, &cap);
        assert_eq!(gpu_lights[0].intensity, 1.0, "bullet light: 4.0 * 0.25 cap");
        assert_eq!(
            gpu_lights[1].intensity, 4.0,
            "non-bullet light: cap does not apply"
        );
    }

    #[test]
    fn build_light_list_of_no_lights_is_empty() {
        let (gpu_lights, dropped) = build_light_list(&[], 32, &BulletLightCap::default());
        assert!(gpu_lights.is_empty());
        assert_eq!(dropped, 0);
    }

    // --- Reference implementation of `mesh.wgsl`'s shading maths, for GPU-free unit tests -----
    //
    // These mirror the WGSL by hand (no shared codegen between WGSL and Rust exists in this
    // repository, so this is kept in sync manually, like `camera_uniform` above already mirrors
    // `mesh.wgsl`'s `Camera` struct layout). They exist only so the maths itself — as opposed to
    // the compiled shader's actual pixels, which `tests/offscreen.rs` checks separately — can be
    // unit tested without a GPU; nothing in the render path calls them, so they are `cfg(test)`
    // only.
    mod pbr_reference {
        use super::MIN_ROUGHNESS;

        pub(super) const PI: f32 = std::f32::consts::PI;

        pub(super) fn distribution_ggx(n_dot_h: f32, alpha_squared: f32) -> f32 {
            let d = n_dot_h * n_dot_h * (alpha_squared - 1.0) + 1.0;
            alpha_squared / (PI * d * d)
        }

        pub(super) fn visibility_smith_height_correlated(
            n_dot_l: f32,
            n_dot_v: f32,
            alpha_squared: f32,
        ) -> f32 {
            let v = n_dot_l * (n_dot_v * n_dot_v * (1.0 - alpha_squared) + alpha_squared).sqrt();
            let l = n_dot_v * (n_dot_l * n_dot_l * (1.0 - alpha_squared) + alpha_squared).sqrt();
            0.5 / (v + l).max(1e-6)
        }

        pub(super) fn fresnel_schlick(v_dot_h: f32, f0: [f32; 3]) -> [f32; 3] {
            let m = (1.0 - v_dot_h).clamp(0.0, 1.0);
            let m5 = m * m * m * m * m;
            [
                f0[0] + (1.0 - f0[0]) * m5,
                f0[1] + (1.0 - f0[1]) * m5,
                f0[2] + (1.0 - f0[2]) * m5,
            ]
        }

        /// Mirrors `mesh.wgsl`'s specular-AA widening: `alpha^2` (from `roughness^2`) widened by
        /// `min(2 * variance, 0.18)`, clamped to at least `MIN_ROUGHNESS^4`.
        pub(super) fn widen_alpha_squared_for_specular_aa(
            roughness: f32,
            normal_variance: f32,
        ) -> f32 {
            let roughness = roughness.clamp(MIN_ROUGHNESS, 1.0);
            let alpha = roughness * roughness;
            let widened = alpha * alpha + (2.0 * normal_variance).min(0.18);
            widened.clamp(MIN_ROUGHNESS.powi(4), 1.0)
        }

        /// Mirrors `mesh.wgsl`'s `point_light_falloff`.
        pub(super) fn point_light_falloff(distance: f32, range: f32) -> f32 {
            if distance >= range {
                return 0.0;
            }
            let x = distance / range;
            let x2 = x * x;
            let q = (1.0 - x2 * x2).clamp(0.0, 1.0);
            q * q / (1.0 + distance * distance)
        }
    }

    use pbr_reference::{
        distribution_ggx, fresnel_schlick, point_light_falloff, visibility_smith_height_correlated,
        widen_alpha_squared_for_specular_aa,
    };

    #[test]
    fn fresnel_schlick_at_normal_incidence_equals_f0() {
        let f0 = [0.04, 0.3, 0.9];
        let f = fresnel_schlick(1.0, f0);
        for (a, b) in f.iter().zip(f0) {
            assert!((a - b).abs() < 1e-6, "{f:?} vs {f0:?}");
        }
    }

    #[test]
    fn fresnel_schlick_at_grazing_angle_approaches_white() {
        let f = fresnel_schlick(0.0, [0.04, 0.04, 0.04]);
        for c in f {
            assert!((c - 1.0).abs() < 1e-6, "{f:?}");
        }
    }

    #[test]
    fn fresnel_schlick_is_monotonically_increasing_towards_grazing_angles() {
        let f0 = [0.04, 0.04, 0.04];
        let steep = fresnel_schlick(0.9, f0)[0];
        let shallow = fresnel_schlick(0.2, f0)[0];
        assert!(
            shallow > steep,
            "Fresnel must increase towards grazing angles: steep {steep}, shallow {shallow}"
        );
    }

    #[test]
    fn distribution_ggx_peaks_at_the_half_vector_aligned_with_the_normal() {
        let alpha_squared = 0.1;
        let aligned = distribution_ggx(1.0, alpha_squared);
        let off_axis = distribution_ggx(0.5, alpha_squared);
        assert!(
            aligned > off_axis,
            "GGX D must peak at n_dot_h = 1: aligned {aligned}, off-axis {off_axis}"
        );
    }

    #[test]
    fn distribution_ggx_is_never_negative_or_nan() {
        for n_dot_h in [0.0, 0.25, 0.5, 0.75, 1.0] {
            for alpha_squared in [1e-4, 0.01, 0.5, 1.0] {
                let d = distribution_ggx(n_dot_h, alpha_squared);
                assert!(
                    d.is_finite() && d >= 0.0,
                    "n_dot_h {n_dot_h}, a2 {alpha_squared}: {d}"
                );
            }
        }
    }

    #[test]
    fn visibility_is_symmetric_in_n_dot_l_and_n_dot_v() {
        let a = visibility_smith_height_correlated(0.3, 0.8, 0.2);
        let b = visibility_smith_height_correlated(0.8, 0.3, 0.2);
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
    }

    #[test]
    fn visibility_is_never_negative_or_nan() {
        for n_dot_l in [0.001, 0.3, 0.7, 1.0] {
            for n_dot_v in [0.001, 0.3, 0.7, 1.0] {
                for alpha_squared in [1e-4, 0.01, 0.5, 1.0] {
                    let vis = visibility_smith_height_correlated(n_dot_l, n_dot_v, alpha_squared);
                    assert!(
                        vis.is_finite() && vis >= 0.0,
                        "{n_dot_l} {n_dot_v} {alpha_squared}: {vis}"
                    );
                }
            }
        }
    }

    #[test]
    fn roughness_clamp_floors_at_min_roughness() {
        let a2_zero_roughness = widen_alpha_squared_for_specular_aa(0.0, 0.0);
        let expected_floor = MIN_ROUGHNESS.powi(4);
        assert!(
            (a2_zero_roughness - expected_floor).abs() < 1e-9,
            "{a2_zero_roughness} vs floor {expected_floor}"
        );
    }

    #[test]
    fn roughness_clamp_widens_monotonically_with_normal_variance() {
        let low_variance = widen_alpha_squared_for_specular_aa(0.2, 0.001);
        let high_variance = widen_alpha_squared_for_specular_aa(0.2, 0.5);
        assert!(
            high_variance > low_variance,
            "more shading-normal variance must widen alpha^2: low {low_variance}, high {high_variance}"
        );
    }

    #[test]
    fn roughness_clamp_never_exceeds_one() {
        let a2 = widen_alpha_squared_for_specular_aa(1.0, 1000.0);
        assert!(a2 <= 1.0, "{a2}");
    }

    #[test]
    fn point_light_falloff_is_zero_at_and_beyond_range() {
        assert_eq!(point_light_falloff(10.0, 10.0), 0.0);
        assert_eq!(point_light_falloff(11.0, 10.0), 0.0);
    }

    #[test]
    fn point_light_falloff_is_positive_within_range() {
        let value = point_light_falloff(5.0, 10.0);
        assert!(value > 0.0 && value.is_finite(), "{value}");
    }

    #[test]
    fn point_light_falloff_decreases_monotonically_with_distance() {
        let near = point_light_falloff(1.0, 10.0);
        let mid = point_light_falloff(5.0, 10.0);
        let far = point_light_falloff(9.0, 10.0);
        assert!(near > mid && mid > far, "near {near}, mid {mid}, far {far}");
    }

    #[test]
    fn point_light_falloff_at_zero_distance_is_at_most_one() {
        assert!(point_light_falloff(0.0, 10.0) <= 1.0);
    }

    // --- Mipmap CPU generation (texture-quality package, strand B1) ----------------------------

    use crate::texture::TextureColorSpace;

    fn texture(width: u32, height: u32, color_space: TextureColorSpace) -> TextureData {
        TextureData {
            width,
            height,
            pixels: vec![0u8; width as usize * height as usize * 4],
            color_space,
        }
    }

    /// The mip chain's level count `wgpu` itself would allow for `(width, height)`
    /// (`1 + floor(log2(max(width, height)))`, the standard chain length every GPU texture mip
    /// chain follows) — [`generate_mip_chain`] must match this exactly, or [`upload_texture`]'s
    /// `mip_level_count` would either under-use the chain or fail `wgpu`'s texture validation.
    fn expected_level_count(width: u32, height: u32) -> usize {
        let largest = width.max(height).max(1);
        (32 - largest.leading_zeros()) as usize
    }

    #[test]
    fn mip_chain_level_count_matches_wgpu_for_several_sizes() {
        for &(width, height) in &[
            (1, 1),
            (2, 2),
            (4, 4),
            (5, 5),
            (8, 3),
            (3, 8),
            (7, 1),
            (1, 7),
            (128, 64),
            (129, 65),
        ] {
            let levels = generate_mip_chain(&texture(width, height, TextureColorSpace::Linear));
            assert_eq!(
                levels.len(),
                expected_level_count(width, height),
                "size {width}x{height}"
            );
            // Every level halves (floored, minimum 1) the one before it, and the chain always
            // bottoms out at exactly 1x1 — never smaller, never skipped.
            let (last_width, last_height, last_pixels) = levels.last().expect("at least level 0");
            assert_eq!((*last_width, *last_height), (1, 1));
            assert_eq!(last_pixels.len(), 4);
            let (first_width, first_height, _) = levels[0];
            assert_eq!((first_width, first_height), (width, height));
        }
    }

    #[test]
    fn mip_chain_never_shrinks_a_dimension_below_one() {
        // A 1-pixel-wide (or -tall) texture: every level must keep that dimension at exactly 1,
        // never 0, and never read past it (see `box_filter_downsample`'s doc comment).
        let levels = generate_mip_chain(&texture(1, 16, TextureColorSpace::Linear));
        assert!(levels.iter().all(|(width, _, _)| *width == 1));
        assert_eq!(levels.last().expect("at least level 0").1, 1);
    }

    /// Odd-sized downsampling must not drop the last row/column: three columns of a *linear*
    /// (not sRGB, so no transfer-function rounding to account for) single-channel texture average
    /// to their arithmetic mean once `next_width` reaches `1` and every column is absorbed into
    /// that one destination texel — computable by hand: `(0 + 90 + 180) / 3 = 90`.
    #[test]
    fn odd_width_downsample_absorbs_every_column_instead_of_dropping_the_last() {
        let data = TextureData {
            width: 3,
            height: 1,
            pixels: vec![
                0, 0, 0, 255, // column 0
                90, 90, 90, 255, // column 1
                180, 180, 180, 255, // column 2
            ],
            color_space: TextureColorSpace::Linear,
        };
        let levels = generate_mip_chain(&data);
        assert_eq!(levels.len(), 2, "3x1 -> 1x1, no intermediate level");
        let (width, height, pixels) = &levels[1];
        assert_eq!((*width, *height), (1, 1));
        assert_eq!(pixels, &vec![90, 90, 90, 255]);
    }

    /// A genuine 1xN texture (the spec's explicit edge case): width stays `1` throughout, only
    /// height halves each level. Values chosen so every intermediate average is an exact integer,
    /// independently computable: rows `10, 20, 30, 40` -> `15, 35` -> `25`.
    #[test]
    fn one_by_n_texture_mip_chain_matches_hand_computed_means() {
        let data = TextureData {
            width: 1,
            height: 4,
            pixels: vec![
                10, 0, 0, 255, // row 0
                20, 0, 0, 255, // row 1
                30, 0, 0, 255, // row 2
                40, 0, 0, 255, // row 3
            ],
            color_space: TextureColorSpace::Linear,
        };
        let levels = generate_mip_chain(&data);
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0].0, 1);
        assert_eq!(levels[1].2, vec![15, 0, 0, 255, 35, 0, 0, 255]);
        assert_eq!(levels[2].2, vec![25, 0, 0, 255]);
    }

    #[test]
    fn identical_srgb_texels_average_to_the_same_value() {
        // Decode-average-encode is the identity when every input texel already agrees: the
        // "known mean" is trivially the value itself, independent of the sRGB transfer function's
        // exact shape.
        let data = TextureData {
            width: 2,
            height: 2,
            pixels: vec![
                200, 150, 50, 255, 200, 150, 50, 255, //
                200, 150, 50, 255, 200, 150, 50, 255,
            ],
            color_space: TextureColorSpace::Srgb,
        };
        let levels = generate_mip_chain(&data);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[1].2, vec![200, 150, 50, 255]);
    }

    /// The sRGB case's known mean, worked out by hand (also in this test's own doc comment so the
    /// number is auditable without running the test): a 2x2 checkerboard of sRGB-encoded black
    /// (`0`) and white (`255`) decodes to linear `0.0`/`1.0` (both endpoints of the sRGB transfer
    /// function are fixed points, so no curve arithmetic needed there), averages to linear `0.5`,
    /// and `1.055 * 0.5^(1/2.4) - 0.055 ≈ 0.7354`, i.e. byte `188` — nowhere near the naive
    /// (colour-space-unaware) byte average of `(0 + 0 + 255 + 255) / 4 = 128`, which is exactly
    /// the bug this test would catch. Alpha is checked separately: it is never sRGB-decoded
    /// (`Rgba8UnormSrgb` only applies the transfer function to R/G/B), so the same `0`/`255`
    /// checkerboard averages there as a plain byte mean, `127.5` rounded to `128`.
    #[test]
    fn known_mean_srgb_checkerboard_matches_hand_computed_value() {
        let data = TextureData {
            width: 2,
            height: 2,
            pixels: vec![
                0, 0, 0, 0, 255, 255, 255, 255, //
                255, 255, 255, 255, 0, 0, 0, 0,
            ],
            color_space: TextureColorSpace::Srgb,
        };
        let levels = generate_mip_chain(&data);
        let (width, height, pixels) = &levels[1];
        assert_eq!((*width, *height), (1, 1));
        assert_eq!(&pixels[0..3], &[188, 188, 188], "sRGB-aware RGB mean");
        assert_eq!(
            pixels[3], 128,
            "alpha is averaged linearly, never sRGB-decoded"
        );
    }

    #[test]
    fn srgb_and_linear_transfer_functions_round_trip_within_rounding() {
        for &byte in &[0u8, 1, 16, 64, 128, 200, 254, 255] {
            let value = f32::from(byte) / 255.0;
            let round_tripped = linear_to_srgb(srgb_to_linear(value));
            assert!(
                (round_tripped - value).abs() < 1e-4,
                "byte {byte}: {round_tripped} vs {value}"
            );
        }
    }
}
