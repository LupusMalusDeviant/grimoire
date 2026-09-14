//! Instanced sprite pipeline: one draw call for all sprites of a frame.

use grimoire_gpu::{GpuContext, GpuError, wgpu};

use crate::SpriteInstance;

/// Size of one [`SpriteInstance`] in the instance buffer.
const INSTANCE_SIZE: u64 = std::mem::size_of::<SpriteInstance>() as u64;

/// Vertex layout of [`SpriteInstance`]; offsets follow its `#[repr(C)]` field order.
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x2,
    1 => Float32x2,
    2 => Float32,
    3 => Uint32,
    4 => Float32x4,
];

/// Corners of the two triangles forming a quad, generated in the vertex shader.
const QUAD_VERTICES: u32 = 6;

/// Capacity the instance buffer must have to hold `required` sprites: unchanged while it fits,
/// otherwise the next power of two (saturating at `u32::MAX`).
pub(crate) const fn grown_capacity(current: u32, required: u32) -> u32 {
    if required <= current {
        return current;
    }
    match required.checked_next_power_of_two() {
        Some(capacity) => capacity,
        None => u32::MAX,
    }
}

pub(crate) struct SpritePass {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    capacity: u32,
}

impl SpritePass {
    pub(crate) fn new(
        context: &GpuContext,
        target_format: wgpu::TextureFormat,
        initial_capacity: u32,
    ) -> Result<Self, GpuError> {
        let device = context.device();
        let shader = context.capture_errors(|device| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("grimoire sprite shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
            })
        })?;

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire camera uniform"),
            size: std::mem::size_of::<[[f32; 4]; 4]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grimoire camera layout"),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grimoire camera bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grimoire sprite layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = context.capture_errors(|device| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("grimoire sprite pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: INSTANCE_SIZE,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &INSTANCE_ATTRIBUTES,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // Rotation and mirrored half sizes may flip the winding.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })?;

        let capacity = initial_capacity.max(1);
        let instance_buffer = create_instance_buffer(context, capacity)?;
        Ok(Self {
            pipeline,
            camera_buffer,
            bind_group,
            instance_buffer,
            capacity,
        })
    }

    /// Uploads the camera and the sprites, growing the instance buffer if needed.
    /// Returns the number of instances to draw.
    pub(crate) fn prepare(
        &mut self,
        context: &GpuContext,
        view_projection: &[[f32; 4]; 4],
        sprites: &[SpriteInstance],
    ) -> Result<u32, GpuError> {
        let count = u32::try_from(sprites.len()).map_err(|_| {
            GpuError::Validation(format!("{} sprites exceed u32::MAX", sprites.len()))
        })?;
        let capacity = grown_capacity(self.capacity, count);
        if capacity != self.capacity {
            log::debug!(
                "growing sprite instance buffer from {} to {capacity}",
                self.capacity
            );
            self.instance_buffer = create_instance_buffer(context, capacity)?;
            self.capacity = capacity;
        }

        let queue = context.queue();
        queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(view_projection.as_slice()),
        );
        if count > 0 {
            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(sprites));
        }
        Ok(count)
    }

    /// Records the sprite draw into `pass`. Returns the number of draw calls issued.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, count: u32) -> u32 {
        if count == 0 {
            return 0;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(
            0,
            self.instance_buffer
                .slice(..u64::from(count) * INSTANCE_SIZE),
        );
        pass.draw(0..QUAD_VERTICES, 0..count);
        1
    }
}

fn create_instance_buffer(context: &GpuContext, capacity: u32) -> Result<wgpu::Buffer, GpuError> {
    let size = u64::from(capacity) * INSTANCE_SIZE;
    let max = context.device().limits().max_buffer_size;
    if size > max {
        return Err(GpuError::Validation(format!(
            "sprite instance buffer of {size} bytes exceeds the device limit of {max} bytes"
        )));
    }
    context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire sprite instances"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    #[test]
    fn capacity_is_kept_while_it_fits() {
        assert_eq!(grown_capacity(16, 0), 16);
        assert_eq!(grown_capacity(16, 16), 16);
        assert_eq!(grown_capacity(1000, 999), 1000);
    }

    #[test]
    fn capacity_grows_to_next_power_of_two() {
        assert_eq!(grown_capacity(16, 17), 32);
        assert_eq!(grown_capacity(16, 10_000), 16_384);
        assert_eq!(grown_capacity(16_384, 16_385), 32_768);
        assert_eq!(grown_capacity(1000, 1024), 1024);
    }

    #[test]
    fn capacity_saturates() {
        assert_eq!(grown_capacity(1, u32::MAX), u32::MAX);
        assert_eq!(grown_capacity(1, (1 << 31) + 1), u32::MAX);
    }

    #[test]
    fn instance_attributes_match_sprite_layout() {
        let offsets = [
            offset_of!(SpriteInstance, position),
            offset_of!(SpriteInstance, half_size),
            offset_of!(SpriteInstance, rotation),
            offset_of!(SpriteInstance, shape),
            offset_of!(SpriteInstance, color),
        ];
        for (attribute, offset) in INSTANCE_ATTRIBUTES.iter().zip(offsets) {
            assert_eq!(attribute.offset, offset as u64);
        }
        let last = &INSTANCE_ATTRIBUTES[4];
        assert_eq!(last.offset + last.format.size(), INSTANCE_SIZE);
    }
}
