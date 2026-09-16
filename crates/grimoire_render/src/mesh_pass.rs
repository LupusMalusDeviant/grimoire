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
//! **Point-light budget (WP2.5):** `mesh.wgsl`'s `Camera` uniform carries a fixed-size array of at
//! most [`MAX_POINT_LIGHTS`] lights. This is *not* the `Low 32`/`High 256` count budget contract §6
//! assigns to WP3.4's clustered forward+ pass (deliberately not part of this contract); it is a
//! smaller, purely internal limit this pass's simple, unclustered P1 light loop needs regardless of
//! any configured budget. [`clamp_light_budget`] keeps the first [`MAX_POINT_LIGHTS`] valid lights
//! (frame order) and reports how many were dropped; [`MeshPass::render`] logs a warning if any
//! were.
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
//! Not part of the crate's public API (engine ADR-0002: `wgpu` stays invisible outside this
//! crate and its `grimoire_gpu` dependency).

use std::collections::HashMap;

use grimoire_gpu::{GpuContext, GpuError, wgpu};

use crate::mesh::{MeshData, MeshError, MeshRegistry};
use crate::shadow_pass::{ShadowCaster, ShadowPass};
use crate::stage3d::{self, AmbientLight, Camera25D, DirectionalLight, MAX_SKIN_JOINTS};
use crate::texture::{TextureData, TextureError, TextureRegistry};
use crate::{
    BlobShadowInstance, MeshHandle, MeshInstance, PbrMaterial, PointLight, RenderLayer,
    ShadowConfig, SkinBinding, TextureHandle,
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

/// Largest number of point lights `mesh.wgsl`'s `Camera.lights` array holds; must match the
/// literal `32` in that file's `array<PointLightGpu, 32>` (no shared codegen between WGSL and
/// Rust, like every other layout constant in this file). See this module's doc comment for why
/// this is not the contract's `Low 32`/`High 256` budget.
pub(crate) const MAX_POINT_LIGHTS: usize = 32;

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

    use super::MAX_POINT_LIGHTS;

    /// One point light as uploaded to `mesh.wgsl`'s `Camera.lights` array. `color` is already
    /// pre-multiplied by [`crate::PointLight::intensity`], matching the existing key-light
    /// convention. `_pad` mirrors WGSL's uniform-address-space alignment of `vec3<f32>` (16
    /// bytes), which the hand-written `PointLightGpu` struct in `mesh.wgsl` also carries.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct PointLightGpu {
        pub position: [f32; 3],
        pub range: f32,
        pub color: [f32; 3],
        pub _pad: f32,
    }

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
    #[repr(C)]
    #[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct CameraGpu {
        pub view_proj: [[f32; 4]; 4],
        pub eye: [f32; 4],
        pub light_dir: [f32; 4],
        pub key_light: [f32; 4],
        pub ambient_sky: [f32; 4],
        pub ambient_ground: [f32; 4],
        pub light_count: u32,
        pub specular_aa_strength: f32,
        pub _pad1: u32,
        pub _pad2: u32,
        pub light_view_proj: [[f32; 4]; 4],
        pub shadow_params: [f32; 4],
        pub lights: [PointLightGpu; MAX_POINT_LIGHTS],
    }
}
use camera_gpu::{CameraGpu, PointLightGpu};

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

fn create_depth_view(
    context: &GpuContext,
    width: u32,
    height: u32,
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
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    })?;
    Ok(texture.create_view(&wgpu::TextureViewDescriptor::default()))
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

/// Uploads `data`'s pixels to a new GPU texture: [`crate::TextureColorSpace::Srgb`] becomes
/// `Rgba8UnormSrgb`, [`crate::TextureColorSpace::Linear`] becomes `Rgba8Unorm` (contract §6:
/// base colour is sRGB, normal/ORM are linear). No mipmaps (see `texture.rs`'s module doc on
/// scope).
fn upload_texture(context: &GpuContext, data: &TextureData) -> Result<GpuTexture, GpuError> {
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
            label: Some("grimoire material texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    })?;
    // `TextureData::validate` (run by `TextureRegistry::register` before this is ever called)
    // already guarantees `width * 4` fits a `u32` and `pixels.len() == width * height * 4`.
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

/// Splits `lights` into the ones the mesh pass's fixed-size shader array can hold (at most
/// `budget`, kept in frame order) and how many were dropped because there were more. Never
/// panics. See this module's doc comment on why `budget` is [`MAX_POINT_LIGHTS`] here and not
/// contract §6's `Low 32`/`High 256` (WP3.4's job).
pub(crate) fn clamp_light_budget<T>(lights: &[T], budget: usize) -> (&[T], usize) {
    if lights.len() <= budget {
        (lights, 0)
    } else {
        (&lights[..budget], lights.len() - budget)
    }
}

/// Filters `point_lights` to the valid ones (contract §6 `PointLight::is_valid`) and clamps them
/// to [`MAX_POINT_LIGHTS`] (frame order), returning the fixed-size GPU array `mesh.wgsl` expects,
/// how many of it are actually lit, and how many valid lights were dropped for budget reasons
/// (`0` unless the frame really does exceed [`MAX_POINT_LIGHTS`]).
fn build_light_array(
    point_lights: &[PointLight],
) -> ([PointLightGpu; MAX_POINT_LIGHTS], u32, usize) {
    let valid: Vec<&PointLight> = point_lights
        .iter()
        .filter(|light| light.is_valid())
        .collect();
    let (used, dropped) = clamp_light_budget(&valid, MAX_POINT_LIGHTS);
    const ZERO_LIGHT: PointLightGpu = PointLightGpu {
        position: [0.0; 3],
        range: 0.0,
        color: [0.0; 3],
        _pad: 0.0,
    };
    let mut lights = [ZERO_LIGHT; MAX_POINT_LIGHTS];
    for (slot, light) in lights.iter_mut().zip(used.iter()) {
        *slot = PointLightGpu {
            position: light.position,
            range: light.range,
            color: [
                light.color[0] * light.intensity,
                light.color[1] * light.intensity,
                light.color[2] * light.intensity,
            ],
            _pad: 0.0,
        };
    }
    let light_count = u32::try_from(used.len()).unwrap_or(0);
    (lights, light_count, dropped)
}

/// Builds the uniform `mesh.wgsl` reads: the view-projection matrix, the eye position, the key
/// light and ambient term flattened into GPU-friendly vectors, the point-light array and count,
/// the OF-3.5 specular-AA toggle, and (WP2.6) the key-light shadow map's light-space
/// view-projection plus its `shadow_params` (see [`CameraGpu`]'s doc comment for the packing). An
/// invalid or missing key light (contract §6 `DirectionalLight::is_valid`) falls back to no
/// directional contribution at all, consistent with `StageStats::key_light_rejected_invalid`
/// already flagging it elsewhere.
#[allow(clippy::too_many_arguments)]
fn camera_uniform(
    view_proj: [[f32; 4]; 4],
    eye: [f32; 3],
    key_light: Option<&DirectionalLight>,
    ambient: &AmbientLight,
    lights: [PointLightGpu; MAX_POINT_LIGHTS],
    light_count: u32,
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
        light_count,
        specular_aa_strength: if specular_aa { 1.0 } else { 0.0 },
        _pad1: 0,
        _pad2: 0,
        light_view_proj,
        shadow_params,
        lights,
    }
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
fn create_skinned_pipeline(
    context: &GpuContext,
    shader: &wgpu::ShaderModule,
    camera_bind_group_layout: &wgpu::BindGroupLayout,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
    shadow_bind_group_layout: &wgpu::BindGroupLayout,
    bone_bind_group_layout: &wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
) -> Result<wgpu::RenderPipeline, GpuError> {
    let device = context.device();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grimoire skinned mesh layout"),
        bind_group_layouts: &[
            Some(camera_bind_group_layout),
            Some(texture_bind_group_layout),
            Some(shadow_bind_group_layout),
            Some(bone_bind_group_layout),
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
            multisample: wgpu::MultisampleState::default(),
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
/// buffer of its own (`blob_shadow.wgsl`'s header comment explains why that is sound).
fn create_blob_pipeline(
    context: &GpuContext,
    camera_bind_group_layout: &wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
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
            multisample: wgpu::MultisampleState::default(),
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
    ) -> Result<Self, GpuError> {
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
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grimoire mesh layout"),
            bind_group_layouts: &[
                Some(&camera_bind_group_layout),
                Some(&texture_bind_group_layout),
                Some(&shadow_bind_group_layout),
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
                multisample: wgpu::MultisampleState::default(),
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

        let depth_view = create_depth_view(context, width, height)?;
        let instance_capacity =
            16u32.min(mesh_instance_capacity(device.limits().max_buffer_size).max(1));
        let instance_buffer = create_mesh_instance_buffer(context, instance_capacity)?;

        let shadow_pass = ShadowPass::new(context, &ShadowConfig::default())?;
        let shadow_sampling_bind_group =
            create_shadow_sampling_bind_group(context, &shadow_bind_group_layout, &shadow_pass);
        let blob_pipeline = create_blob_pipeline(context, &camera_bind_group_layout, color_format)?;
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
            color_format,
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
        })
    }

    /// Recreates the depth buffer for a new colour target size; a no-op if `width`/`height` match
    /// the current depth buffer already.
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
        self.depth_view = create_depth_view(context, width, height)?;
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
    /// `point_lights` are shaded through the same GGX term as the key light, clamped to
    /// [`MAX_POINT_LIGHTS`] (see this module's doc comment); `specular_aa` is the OF-3.5 toggle
    /// (production rendering always passes `true`, see
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
    /// Returns the number of draw calls issued across every sub-pass this call touches (`0` if
    /// there is no usable camera or nothing to draw), for [`crate::StageStats::base`]'s
    /// `draw_calls` (contract §6: "all passes").
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
        meshes: &[MeshInstance],
        materials: &[PbrMaterial],
        specular_aa: bool,
        shadow_config: &ShadowConfig,
        blob_shadows: &[BlobShadowInstance],
        joint_matrices: &[[[f32; 4]; 4]],
    ) -> Result<u32, GpuError> {
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
        let (lights, light_count, dropped) = build_light_array(point_lights);
        if dropped > 0 {
            let total_valid = dropped + light_count as usize;
            log::warn!(
                "point light budget exceeded: {total_valid} valid point lights this frame, shading only the first {MAX_POINT_LIGHTS} (WP3.4 replaces this with clustered forward+ and a configurable Low 32 / High 256 budget)"
            );
        }

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
            lights,
            light_count,
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
        let blob_pipeline = &self.blob_pipeline;
        let skinned_pipeline = &self.skinned_pipeline;
        let skinned_instance_buffer = &self.skinned_instance_buffer;
        let bone_bind_group = &self.bone_bind_group;
        let blob_instance_buffer = &self.blob_instance_buffer;
        context.capture_errors(move |device| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("grimoire mesh encoder"),
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("grimoire mesh pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear_color),
                            store: wgpu::StoreOp::Store,
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

        Ok(draw_calls)
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
    use crate::mesh::MeshVertex;

    #[test]
    fn mesh_instance_gpu_is_112_bytes() {
        assert_eq!(std::mem::size_of::<MeshInstanceGpu>(), 112);
    }

    #[test]
    fn camera_gpu_is_1264_bytes() {
        // WP2.5's 1184 bytes plus WP2.6's `light_view_proj` (64) and `shadow_params` (16).
        assert_eq!(std::mem::size_of::<CameraGpu>(), 1264);
    }

    #[test]
    fn point_light_gpu_is_32_bytes() {
        assert_eq!(std::mem::size_of::<PointLightGpu>(), 32);
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

    #[test]
    fn camera_uniform_falls_back_to_no_light_or_ambient_when_invalid() {
        let (lights, count, dropped) = build_light_array(&[]);
        assert_eq!(count, 0);
        assert_eq!(dropped, 0);
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
            lights,
            count,
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
        let (lights, count, _) = build_light_array(&[]);
        let uniform = camera_uniform(
            IDENTITY,
            [0.0, 0.0, 0.0],
            None,
            &AmbientLight::Flat {
                color,
                intensity: 1.0,
            },
            lights,
            count,
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
        let (lights, count, _) = build_light_array(&[]);
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
            lights,
            count,
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
        let (lights, count, _) = build_light_array(&[]);
        let uniform = camera_uniform(
            matrix,
            [0.0, 0.0, 0.0],
            None,
            &AmbientLight::Flat {
                color: [0.0, 0.0, 0.0],
                intensity: 0.0,
            },
            lights,
            count,
            true,
            IDENTITY,
            [0.0; 4],
        );
        assert_eq!(uniform.view_proj, matrix);
    }

    #[test]
    fn camera_uniform_carries_the_eye_position_and_specular_aa_toggle() {
        let (lights, count, _) = build_light_array(&[]);
        let uniform = camera_uniform(
            IDENTITY,
            [1.0, 2.0, 3.0],
            None,
            &AmbientLight::default(),
            lights,
            count,
            false,
            IDENTITY,
            [0.0; 4],
        );
        assert_eq!(&uniform.eye[..3], [1.0, 2.0, 3.0]);
        assert_eq!(uniform.specular_aa_strength, 0.0);
    }

    #[test]
    fn camera_uniform_carries_the_shadow_matrix_and_params_through() {
        let (lights, count, _) = build_light_array(&[]);
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
            lights,
            count,
            true,
            light_view_proj,
            shadow_params,
        );
        assert_eq!(uniform.light_view_proj, light_view_proj);
        assert_eq!(uniform.shadow_params, shadow_params);
    }

    // --- clamp_light_budget / build_light_array (WP2.5) --------------------------------------

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
    fn build_light_array_drops_invalid_lights_without_counting_them_as_budget_drops() {
        let lights = vec![
            light_at(1.0),
            PointLight {
                intensity: f32::NAN,
                ..light_at(2.0)
            },
            light_at(3.0),
        ];
        let (gpu_lights, count, dropped) = build_light_array(&lights);
        assert_eq!(
            count, 2,
            "the NaN-intensity light is invalid, not budget-dropped"
        );
        assert_eq!(dropped, 0);
        assert_eq!(gpu_lights[0].position, [1.0, 0.0, 0.0]);
        assert_eq!(gpu_lights[1].position, [3.0, 0.0, 0.0]);
    }

    #[test]
    fn build_light_array_clamps_to_max_point_lights_and_reports_the_rest_dropped() {
        let lights: Vec<PointLight> = (0..MAX_POINT_LIGHTS + 5)
            .map(|i| light_at(i as f32))
            .collect();
        let (gpu_lights, count, dropped) = build_light_array(&lights);
        assert_eq!(count as usize, MAX_POINT_LIGHTS);
        assert_eq!(dropped, 5);
        assert_eq!(gpu_lights[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(
            gpu_lights[MAX_POINT_LIGHTS - 1].position,
            [(MAX_POINT_LIGHTS - 1) as f32, 0.0, 0.0]
        );
    }

    #[test]
    fn build_light_array_premultiplies_color_by_intensity() {
        let lights = [PointLight {
            color: [0.5, 0.25, 1.0],
            intensity: 2.0,
            ..PointLight::default()
        }];
        let (gpu_lights, count, _) = build_light_array(&lights);
        assert_eq!(count, 1);
        assert_eq!(gpu_lights[0].color, [1.0, 0.5, 2.0]);
    }

    #[test]
    fn build_light_array_zero_fills_unused_slots() {
        let (gpu_lights, count, _) = build_light_array(&[light_at(1.0)]);
        assert_eq!(count, 1);
        assert_eq!(
            gpu_lights[1],
            PointLightGpu {
                position: [0.0; 3],
                range: 0.0,
                color: [0.0; 3],
                _pad: 0.0,
            }
        );
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
}
