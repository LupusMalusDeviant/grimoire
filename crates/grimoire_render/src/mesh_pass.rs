//! GPU mesh pass (plan 0002 WP2.3): a depth buffer, static per-mesh vertex/index buffers and
//! deliberately provisional shading — base colour factor with one directional key light plus a
//! two-colour (sky/ground) ambient term, no GGX, no shadows, no textures. WP2.5 replaces the
//! shading; the geometry and depth-buffer plumbing here stay.
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
//! reports this counter.
//!
//! Not part of the crate's public API (engine ADR-0002: `wgpu` stays invisible outside this
//! crate and its `grimoire_gpu` dependency).

use std::collections::HashMap;

use grimoire_gpu::{GpuContext, GpuError, wgpu};

use crate::mesh::{MeshData, MeshError, MeshRegistry};
use crate::stage3d::{self, AmbientLight, Camera25D, DirectionalLight};
use crate::{MeshHandle, MeshInstance, PbrMaterial, RenderLayer};

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

mod instance_gpu {
    // bytemuck's derive macros expand to `unsafe impl` blocks, like `SpriteInstance` elsewhere in
    // this crate.
    #![allow(unsafe_code)]

    /// Per-instance data uploaded for the mesh pass: the model transform and the (currently very
    /// simple) material inputs the provisional shader in `mesh.wgsl` consumes. An internal GPU
    /// upload layout, not a contract type: [`crate::MeshInstance`] and [`crate::PbrMaterial`] stay
    /// the public shape; this is what they get flattened into for the vertex buffer.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct MeshInstanceGpu {
        /// Column-major model-to-world transform, copied verbatim from [`crate::MeshInstance::transform`].
        pub transform: [[f32; 4]; 4],
        /// [`crate::PbrMaterial::base_color_factor`] of the instance's material.
        pub base_color: [f32; 4],
        /// [`crate::PbrMaterial::emissive_factor`] of the instance's material, `w` unused.
        pub emissive: [f32; 4],
    }
}
use instance_gpu::MeshInstanceGpu;

/// Size of one [`MeshInstanceGpu`] in the instance buffer.
const MESH_INSTANCE_SIZE: u64 = std::mem::size_of::<MeshInstanceGpu>() as u64;

/// Camera and lighting uniform consumed by `mesh.wgsl`. Field layout (four `vec4<f32>` after the
/// `mat4x4<f32>`) must match the WGSL `Camera` struct exactly: 128 bytes, 32 `f32`s.
type MeshCameraUniform = [f32; 32];

/// GPU-side geometry of one registered mesh: static vertex and index buffers, uploaded once at
/// [`MeshPass::register`] and never rewritten.
struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

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

/// Builds the uniform `mesh.wgsl` reads: the view-projection matrix plus the key light and
/// ambient term flattened into GPU-friendly vectors. An invalid or missing key light
/// (contract §6 `DirectionalLight::is_valid`) falls back to no directional contribution at all,
/// consistent with `StageStats::key_light_rejected_invalid` already flagging it elsewhere.
fn camera_uniform(
    view_proj: [[f32; 4]; 4],
    key_light: Option<&DirectionalLight>,
    ambient: &AmbientLight,
) -> MeshCameraUniform {
    let mut uniform: MeshCameraUniform = [0.0; 32];
    uniform[..16].copy_from_slice(bytemuck::cast_slice(view_proj.as_slice()));

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
    uniform[16..19].copy_from_slice(&light_dir);
    uniform[20..23].copy_from_slice(&key_color);

    let (sky, ground) = ambient_terms(ambient);
    uniform[24..27].copy_from_slice(&sky);
    uniform[28..31].copy_from_slice(&ground);

    uniform
}

/// GPU mesh pipeline, depth buffer and mesh registry, owned by [`crate::WgpuRenderer`].
pub(crate) struct MeshPass {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    registry: MeshRegistry,
    gpu_meshes: HashMap<MeshHandle, GpuMesh>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u32,
    depth_view: wgpu::TextureView,
    depth_size: (u32, u32),
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
            size: std::mem::size_of::<MeshCameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grimoire mesh camera bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grimoire mesh layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let vertex_attributes: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x3,
            2 => Float32x2,
        ];
        let instance_attributes: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
            3 => Float32x4,
            4 => Float32x4,
            5 => Float32x4,
            6 => Float32x4,
            7 => Float32x4,
            8 => Float32x4,
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
                        // Provisional pass: materials draw opaque regardless of `alpha_mode`;
                        // WP2.5/WP2.6 add real blending and alpha test.
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

        Ok(Self {
            pipeline,
            camera_buffer,
            bind_group,
            registry: MeshRegistry::default(),
            gpu_meshes: HashMap::new(),
            instance_buffer,
            instance_capacity,
            depth_view,
            depth_size: (width.max(1), height.max(1)),
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

    /// Runs the mesh pass: always clears `view` to `clear_color` and the depth buffer to `1.0`
    /// (even with no usable camera or no drawable instances, so the sprite pass that follows can
    /// unconditionally `LoadOp::Load` afterwards), then draws every [`MeshInstance`] that is
    /// simultaneously: on [`RenderLayer::World`], structurally valid (finite `transform`, a
    /// `material` index pointing at a valid [`PbrMaterial`] in `materials` — the same rules
    /// `stage::extract_stage3d` already applies), and whose `mesh` handle is registered with GPU
    /// buffers here. Grouped by mesh handle (first-seen order) into one `draw_indexed` call per
    /// distinct mesh referenced this frame.
    ///
    /// Returns the number of draw calls issued (`0` if there is no usable camera or nothing to
    /// draw), for [`crate::StageStats::base`]'s `draw_calls` (contract §6: "all passes").
    ///
    /// # Errors
    /// [`GpuError`] if uploading the per-frame uniform/instance data or submitting the render pass
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
        meshes: &[MeshInstance],
        materials: &[PbrMaterial],
    ) -> Result<u32, GpuError> {
        let view_proj =
            camera.map(|camera| stage3d::view_projection(camera, aspect, NEAR_PLANE, FAR_PLANE));
        let usable_view_proj =
            view_proj.filter(|matrix| matrix.iter().flatten().all(|c| c.is_finite()));

        let mut order: Vec<MeshHandle> = Vec::new();
        let mut groups: HashMap<MeshHandle, Vec<MeshInstanceGpu>> = HashMap::new();
        if usable_view_proj.is_some() {
            for mesh in meshes {
                if mesh.layer != RenderLayer::World {
                    continue;
                }
                if !mesh.transform.iter().flatten().all(|c| c.is_finite()) {
                    continue;
                }
                let Some(material) = materials.get(mesh.material.0 as usize) else {
                    continue;
                };
                if !material.is_valid() {
                    continue;
                }
                if !self.is_registered(mesh.mesh) {
                    continue;
                }
                groups
                    .entry(mesh.mesh)
                    .or_insert_with(|| {
                        order.push(mesh.mesh);
                        Vec::new()
                    })
                    .push(MeshInstanceGpu {
                        transform: mesh.transform,
                        base_color: material.base_color_factor,
                        emissive: [
                            material.emissive_factor[0],
                            material.emissive_factor[1],
                            material.emissive_factor[2],
                            0.0,
                        ],
                    });
            }
        }

        let mut instances = Vec::new();
        let mut draw_ranges: Vec<(MeshHandle, u32, u32)> = Vec::new();
        for handle in &order {
            let group = &groups[handle];
            let start = u32::try_from(instances.len()).unwrap_or(u32::MAX);
            let len = u32::try_from(group.len()).unwrap_or(u32::MAX);
            draw_ranges.push((*handle, start, len));
            instances.extend_from_slice(group);
        }

        let uniform = camera_uniform(usable_view_proj.unwrap_or(IDENTITY), key_light, ambient);
        context.queue().write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(uniform.as_slice()),
        );

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

        let draw_calls = if count > 0 {
            u32::try_from(draw_ranges.len()).unwrap_or(u32::MAX)
        } else {
            0
        };

        let gpu_meshes = &self.gpu_meshes;
        let pipeline = &self.pipeline;
        let bind_group = &self.bind_group;
        let instance_buffer = &self.instance_buffer;
        let depth_view = &self.depth_view;
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
                    pass.set_bind_group(0, bind_group, &[]);
                    pass.set_vertex_buffer(
                        1,
                        instance_buffer.slice(..u64::from(count) * MESH_INSTANCE_SIZE),
                    );
                    for (handle, start, len) in &draw_ranges {
                        let gpu_mesh = &gpu_meshes[handle];
                        pass.set_vertex_buffer(0, gpu_mesh.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            gpu_mesh.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..gpu_mesh.index_count, 0, *start..(*start + *len));
                    }
                }
            }
            context.queue().submit([encoder.finish()]);
        })?;

        Ok(draw_calls)
    }
}

#[cfg(test)]
mod tests {
    use std::mem::offset_of;

    use super::*;
    use crate::mesh::MeshVertex;

    #[test]
    fn mesh_instance_gpu_is_96_bytes() {
        assert_eq!(std::mem::size_of::<MeshInstanceGpu>(), 96);
    }

    #[test]
    fn camera_uniform_is_128_bytes() {
        assert_eq!(std::mem::size_of::<MeshCameraUniform>(), 128);
    }

    #[test]
    fn vertex_attributes_match_mesh_vertex_layout() {
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
        let last = &attributes[2];
        assert_eq!(
            last.offset + last.format.size(),
            std::mem::size_of::<MeshVertex>() as u64
        );
    }

    #[test]
    fn instance_attributes_match_mesh_instance_gpu_layout() {
        let attributes: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
            3 => Float32x4,
            4 => Float32x4,
            5 => Float32x4,
            6 => Float32x4,
            7 => Float32x4,
            8 => Float32x4,
        ];
        // The four transform columns are contiguous 16-byte chunks of `transform: [[f32; 4]; 4]`,
        // immediately followed by `base_color` and `emissive`.
        let offsets = [
            0,
            16,
            32,
            48,
            offset_of!(MeshInstanceGpu, base_color),
            offset_of!(MeshInstanceGpu, emissive),
        ];
        for (attribute, offset) in attributes.iter().zip(offsets) {
            assert_eq!(attribute.offset, offset as u64);
        }
        let last = &attributes[5];
        assert_eq!(last.offset + last.format.size(), MESH_INSTANCE_SIZE);
    }

    #[test]
    fn mesh_instance_capacity_matches_buffer_size() {
        assert_eq!(mesh_instance_capacity(MESH_INSTANCE_SIZE * 10), 10);
        assert_eq!(mesh_instance_capacity(u64::MAX), u32::MAX);
    }

    #[test]
    fn camera_uniform_falls_back_to_no_light_or_ambient_when_invalid() {
        let uniform = camera_uniform(
            IDENTITY,
            Some(&DirectionalLight {
                // Zero-length direction: DirectionalLight::is_valid() rejects it.
                direction: [0.0, 0.0, 0.0],
                ..DirectionalLight::default()
            }),
            &AmbientLight::Flat {
                color: [1.0, 1.0, 1.0],
                intensity: -1.0, // AmbientLight::is_valid() rejects a negative intensity.
            },
        );
        assert_eq!(
            &uniform[16..24],
            [0.0; 8],
            "light_dir and key_light both zero"
        );
        assert_eq!(
            &uniform[24..32],
            [0.0; 8],
            "ambient_sky and ambient_ground both zero"
        );
    }

    #[test]
    fn camera_uniform_uses_ambient_flat_for_both_sky_and_ground() {
        // Exactly representable in binary floating point, so the assertion below needs no
        // tolerance.
        let color = [0.25, 0.5, 0.75];
        let uniform = camera_uniform(
            IDENTITY,
            None,
            &AmbientLight::Flat {
                color,
                intensity: 1.0,
            },
        );
        assert_eq!(&uniform[24..27], color, "sky");
        assert_eq!(
            &uniform[28..31],
            color,
            "ground, same as sky for AmbientLight::Flat"
        );
    }

    #[test]
    fn camera_uniform_points_light_dir_towards_the_light() {
        let uniform = camera_uniform(
            IDENTITY,
            Some(&DirectionalLight {
                direction: [0.0, 0.0, -1.0], // light travels straight down
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
            }),
            &AmbientLight::Flat {
                color: [0.0, 0.0, 0.0],
                intensity: 0.0,
            },
        );
        // `light_dir` points *from a surface towards the light*: the negated travel direction.
        assert_eq!(&uniform[16..19], [0.0, 0.0, 1.0]);
        assert_eq!(&uniform[20..23], [1.0, 1.0, 1.0]);
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
            None,
            &AmbientLight::Flat {
                color: [0.0, 0.0, 0.0],
                intensity: 0.0,
            },
        );
        assert_eq!(
            &uniform[0..16],
            [
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0,
                16.0
            ]
        );
    }
}
