//! Depth-only shadow map pass for the key light (plan 0002 WP2.6, OF-3.2). Builds a fitted
//! orthographic light-space view-projection ([`crate::stage3d::key_light_view_projection`]) and
//! renders every caster's transform into a square [`wgpu::TextureFormat::Depth32Float`] texture
//! with a slope-scaled depth bias (`shadow.wgsl`, no fragment stage — a genuine depth-only
//! pipeline). [`crate::mesh_pass::MeshPass`] owns one of these (it already owns the registered
//! meshes' GPU vertex/index buffers this pass draws) and samples the resulting depth texture with
//! a comparison sampler for PCF filtering in `mesh.wgsl`.
//!
//! Deliberately decoupled from [`crate::mesh::MeshRegistry`]/[`crate::MeshHandle`]: this module
//! only ever sees the GPU buffers and transform of each already-resolved [`ShadowCaster`], so it
//! has no reason to know how a caster was validated or registered — that stays `mesh_pass`'s job,
//! mirroring the same validate-then-hand-off shape as [`crate::mesh_pass::MeshPass::render`]'s own
//! mesh grouping.
//!
//! Not part of the crate's public API (engine ADR-0002: `wgpu` stays invisible outside this crate
//! and its `grimoire_gpu` dependency).

use grimoire_gpu::{GpuContext, GpuError, wgpu};

use crate::gpu_timer::PassTimestamps;
use crate::mesh::MeshVertex;
use crate::stage3d::ShadowConfig;

/// Depth-buffer format of the shadow map, matching [`crate::mesh_pass`]'s own choice of
/// [`wgpu::TextureFormat::Depth32Float`]: guaranteed renderable on every backend this engine
/// targets, including the software adapters (WARP, lavapipe) used in CI and local offscreen tests.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

mod instance_gpu {
    // bytemuck's derive macros expand to `unsafe impl` blocks, like `MeshInstanceGpu` in
    // `mesh_pass.rs`.
    #![allow(unsafe_code)]

    /// Per-instance data uploaded for the shadow pass: just the model transform (`shadow.wgsl`).
    /// Unlike `mesh_pass::MeshInstanceGpu` this carries no material data — the shadow map never
    /// shades a caster, only records its depth.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct ShadowInstanceGpu {
        pub transform: [[f32; 4]; 4],
    }
}
use instance_gpu::ShadowInstanceGpu;

/// Size of one [`ShadowInstanceGpu`] in the instance buffer.
const SHADOW_INSTANCE_SIZE: u64 = std::mem::size_of::<ShadowInstanceGpu>() as u64;

mod uniform_gpu {
    // bytemuck's derive macros expand to `unsafe impl` blocks, like `CameraGpu` in `mesh_pass.rs`.
    #![allow(unsafe_code)]

    /// Per-frame uniform `shadow.wgsl` reads: only the light-space view-projection matrix.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct ShadowUniformGpu {
        pub light_view_proj: [[f32; 4]; 4],
    }
}
use uniform_gpu::ShadowUniformGpu;

/// Largest instance count a buffer of at most `max_buffer_size` bytes can hold, mirroring
/// `mesh_pass::mesh_instance_capacity`/`sprite_pass::max_instance_capacity` (each pass sizes its
/// own, independently evolving instance layout).
fn shadow_instance_capacity(max_buffer_size: u64) -> u32 {
    u32::try_from(max_buffer_size / SHADOW_INSTANCE_SIZE).unwrap_or(u32::MAX)
}

fn create_instance_buffer(context: &GpuContext, capacity: u32) -> Result<wgpu::Buffer, GpuError> {
    let size = u64::from(capacity) * SHADOW_INSTANCE_SIZE;
    context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire shadow instances"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })
}

fn create_map_view(context: &GpuContext, size: u32) -> Result<wgpu::TextureView, GpuError> {
    let texture = context.capture_errors(|device| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("grimoire shadow map"),
            size: wgpu::Extent3d {
                width: size.max(1),
                height: size.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    })?;
    Ok(texture.create_view(&wgpu::TextureViewDescriptor::default()))
}

fn create_pipeline(
    context: &GpuContext,
    bind_group_layout: &wgpu::BindGroupLayout,
    depth_bias_constant: i32,
    depth_bias_slope_scale: f32,
) -> Result<wgpu::RenderPipeline, GpuError> {
    let device = context.device();
    let shader = context.capture_errors(|device| {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grimoire shadow shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shadow.wgsl").into()),
        })
    })?;
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grimoire shadow layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    let vertex_attributes: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![
        0 => Float32x3,
    ];
    let instance_attributes: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        1 => Float32x4,
        2 => Float32x4,
        3 => Float32x4,
        4 => Float32x4,
    ];
    context.capture_errors(|device| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("grimoire shadow pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<MeshVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &vertex_attributes,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: SHADOW_INSTANCE_SIZE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attributes,
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // Mirrors `mesh_pass`'s pipeline: the procedural test meshes do not guarantee
                // consistent winding (`crate::procedural`'s module doc).
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState {
                    constant: depth_bias_constant,
                    slope_scale: depth_bias_slope_scale,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            // Depth-only: no colour target, no fragment stage at all.
            fragment: None,
            multiview_mask: None,
            cache: None,
        })
    })
}

/// One shadow caster, already resolved to its GPU buffers by the caller ([`crate::mesh_pass`]):
/// exactly the [`crate::MeshInstance`]s [`crate::StageStats::shadow_casters_drawn`] counts (valid,
/// on [`crate::RenderLayer::World`], and registered with the renderer).
pub(crate) struct ShadowCaster<'a> {
    pub vertex_buffer: &'a wgpu::Buffer,
    pub index_buffer: &'a wgpu::Buffer,
    pub index_count: u32,
    pub transform: [[f32; 4]; 4],
}

/// Owns the shadow map texture, its depth-only pipeline, the light-space uniform and a growable
/// instance buffer. See this module's doc comment for the overall design.
pub(crate) struct ShadowPass {
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    map_view: wgpu::TextureView,
    map_size: u32,
    depth_bias_constant: i32,
    depth_bias_slope_scale: f32,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u32,
}

impl ShadowPass {
    pub(crate) fn new(context: &GpuContext, config: &ShadowConfig) -> Result<Self, GpuError> {
        let device = context.device();
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grimoire shadow uniform layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire shadow uniform"),
            size: std::mem::size_of::<ShadowUniformGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grimoire shadow bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let map_size = config.map_size.max(1);
        let map_view = create_map_view(context, map_size)?;
        let pipeline = create_pipeline(
            context,
            &bind_group_layout,
            config.depth_bias_constant,
            config.depth_bias_slope_scale,
        )?;
        // Comparison sampler for hardware PCF taps in `mesh.wgsl` (`textureSampleCompareLevel`).
        // `ClampToEdge` keeps a tap that strays just outside the map at the frustum edge reading
        // that edge's own depth rather than wrapping to the opposite side.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("grimoire shadow comparison sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let instance_capacity =
            16u32.min(shadow_instance_capacity(device.limits().max_buffer_size).max(1));
        let instance_buffer = create_instance_buffer(context, instance_capacity)?;

        Ok(Self {
            bind_group_layout,
            uniform_buffer,
            bind_group,
            pipeline,
            sampler,
            map_view,
            map_size,
            depth_bias_constant: config.depth_bias_constant,
            depth_bias_slope_scale: config.depth_bias_slope_scale,
            instance_buffer,
            instance_capacity,
        })
    }

    /// Rebuilds the shadow map texture (if [`ShadowConfig::map_size`] changed) and/or the pipeline
    /// (if either bias field changed) since the last call or construction. A no-op otherwise, so
    /// calling this once every frame — even when the config never changes — is cheap.
    pub(crate) fn configure(
        &mut self,
        context: &GpuContext,
        config: &ShadowConfig,
    ) -> Result<(), GpuError> {
        let map_size = config.map_size.max(1);
        if map_size != self.map_size {
            self.map_view = create_map_view(context, map_size)?;
            self.map_size = map_size;
        }
        if config.depth_bias_constant != self.depth_bias_constant
            || config.depth_bias_slope_scale != self.depth_bias_slope_scale
        {
            self.pipeline = create_pipeline(
                context,
                &self.bind_group_layout,
                config.depth_bias_constant,
                config.depth_bias_slope_scale,
            )?;
            self.depth_bias_constant = config.depth_bias_constant;
            self.depth_bias_slope_scale = config.depth_bias_slope_scale;
        }
        Ok(())
    }

    /// The shadow map's current depth texture view, for `mesh_pass` to bind as a sampled texture.
    pub(crate) fn view(&self) -> &wgpu::TextureView {
        &self.map_view
    }

    /// The comparison sampler PCF taps use in `mesh.wgsl`.
    pub(crate) fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// Current shadow map resolution (after the most recent [`ShadowPass::configure`]).
    pub(crate) fn map_size(&self) -> u32 {
        self.map_size
    }

    /// Renders every `caster`'s transform into the shadow map from `light_view_proj`. Always
    /// clears the depth buffer to `1.0` first, even with an empty `casters` slice, so a stale
    /// depth map from a previous frame's geometry never lingers into this one. Returns the number
    /// of draw calls issued (`0` for an empty `casters`).
    ///
    /// # Errors
    /// [`GpuError`] if uploading the per-frame uniform/instance data or submitting the render pass
    /// fails (for example out of memory).
    pub(crate) fn render(
        &mut self,
        context: &GpuContext,
        light_view_proj: [[f32; 4]; 4],
        casters: &[ShadowCaster<'_>],
        timestamps: Option<PassTimestamps>,
    ) -> Result<u32, GpuError> {
        let uniform = ShadowUniformGpu { light_view_proj };
        context
            .queue()
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));

        let count = u32::try_from(casters.len()).map_err(|_| {
            GpuError::Validation(format!("{} shadow casters exceed u32::MAX", casters.len()))
        })?;
        if count > 0 {
            let max_buffer_size = context.device().limits().max_buffer_size;
            let capacity = crate::sprite_pass::grown_capacity(
                self.instance_capacity,
                count,
                shadow_instance_capacity(max_buffer_size),
            )
            .ok_or_else(|| {
                GpuError::Validation(format!(
                    "{count} shadow casters exceed the device buffer limit of {max_buffer_size} bytes"
                ))
            })?;
            if capacity != self.instance_capacity {
                self.instance_buffer = create_instance_buffer(context, capacity)?;
                self.instance_capacity = capacity;
            }
            let instances: Vec<ShadowInstanceGpu> = casters
                .iter()
                .map(|caster| ShadowInstanceGpu {
                    transform: caster.transform,
                })
                .collect();
            context.queue().write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(instances.as_slice()),
            );
        }

        let map_view = &self.map_view;
        let pipeline = &self.pipeline;
        let bind_group = &self.bind_group;
        let instance_buffer = &self.instance_buffer;
        context.capture_errors(move |device| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("grimoire shadow encoder"),
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("grimoire shadow pass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: map_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: timestamps.as_ref().map(PassTimestamps::render),
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                if count > 0 {
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, bind_group, &[]);
                    pass.set_vertex_buffer(
                        1,
                        instance_buffer.slice(..u64::from(count) * SHADOW_INSTANCE_SIZE),
                    );
                    for (start, caster) in casters.iter().enumerate() {
                        let start = start as u32;
                        pass.set_vertex_buffer(0, caster.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            caster.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..caster.index_count, 0, start..start + 1);
                    }
                }
            }
            context.queue().submit([encoder.finish()]);
        })?;

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_instance_gpu_is_64_bytes() {
        assert_eq!(std::mem::size_of::<ShadowInstanceGpu>(), 64);
    }

    #[test]
    fn shadow_uniform_gpu_is_64_bytes() {
        assert_eq!(std::mem::size_of::<ShadowUniformGpu>(), 64);
    }

    #[test]
    fn shadow_instance_capacity_matches_buffer_size() {
        assert_eq!(shadow_instance_capacity(SHADOW_INSTANCE_SIZE * 10), 10);
        assert_eq!(shadow_instance_capacity(u64::MAX), u32::MAX);
    }
}
