//! Bullet pass, layer 6 (plan 0002 WP3.5, contract §6, engine ADR-0014 "billboard impostors").
//!
//! Two instanced draw calls cover every bullet the pass accepts
//! ([`crate::stage::is_accepted_bullet`]: palette space [`crate::BULLET_PASS_PALETTE_SPACE`],
//! finite geometry, table indices in range): all halos first, then all rims and bodies, so no
//! bullet's glow lies on top of another bullet's outline.
//! Each bullet is a camera-facing billboard on the ground plane (`Z = 0`, so the drawn centre is
//! exactly the position collision uses), projected with the frame's [`crate::Camera25D`] when
//! there is one and with [`crate::Camera2D`] otherwise, oriented along its flight direction.
//!
//! **Upload.** The extraction (the facade's Sigil adapter, WP5.3) reads the pool's
//! structure-of-arrays columns and hands the renderer one flat [`BulletInstance`] array; this pass
//! uploads it with a single `write_buffer` into a growing instance buffer — no per-bullet GPU call,
//! no allocation per frame. When every instance is accepted (the normal case) the frame's slice is
//! uploaded as is, byte for byte; only a frame with rejected instances is first compacted into a
//! reused staging vector.
//!
//! **Silhouette, palette, glow.** Silhouettes come from a signed-distance atlas baked once on the
//! CPU at construction ([`bake_silhouette_atlas`]), one cell per [`crate::bullet_silhouette`]
//! entry — PRD-0003 rule 3 through a texture instead of geometry, as engine ADR-0014 requires.
//! Palettes ([`crate::bullet_palette`]), the dark rim and the look parameters live in a uniform
//! uploaded once ([`BulletStyleGpu`]); the numbers follow stylebook v0 "Bullets" and are
//! provisional until the look review (P-11). Glow is drawn by this pass itself as a halo outside
//! the rim, never by a post effect (PRD-0003 rule 1).
//!
//! The pass draws after the (empty) post-processing resolve and telegraphy slots and before the
//! player marker (`crate::pass_graph`), without a depth test: no world geometry can occlude a
//! bullet.

use grimoire_gpu::{GpuContext, GpuError, wgpu};

use crate::stage::is_accepted_bullet;
use crate::{BulletInstance, StageFrame, bullet_palette, bullet_silhouette};

/// Size of one [`BulletInstance`] in the instance buffer.
const INSTANCE_SIZE: u64 = std::mem::size_of::<BulletInstance>() as u64;

/// Vertex layout of [`BulletInstance`]; offsets follow its `#[repr(C)]` field order. The two
/// trailing `u32`s read bytes 16..20 (`silhouette`, `palette`) and 20..24 (`palette_space`,
/// `glow`, `flags`) as little-endian words, which `bullet.wgsl` unpacks bitwise.
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x2,
    1 => Float32,
    2 => Float32,
    3 => Uint32,
    4 => Uint32,
];

/// Corners of the two triangles forming a quad, generated in the vertex shader.
const QUAD_VERTICES: u32 = 6;

/// Initial instance capacity; the buffer grows to the next power of two on demand.
const INITIAL_CAPACITY: u32 = 1024;

/// Texels along one edge of one atlas cell.
pub(crate) const ATLAS_CELL: u32 = 128;
/// Half extent of one atlas cell in silhouette-local units (1 = the bullet's radius). Covers the
/// visible radius, the rim and the full glow reach.
pub(crate) const ATLAS_EXTENT: f32 = 2.5;
/// Signed distance stored as texel value `0`.
pub(crate) const DISTANCE_MIN: f32 = -1.0;
/// Signed distance stored as texel value `255`; larger distances saturate.
pub(crate) const DISTANCE_MAX: f32 = 1.5;

/// Rice silhouette: half the distance between the capsule's two cap centres.
const RICE_HALF_LENGTH: f32 = 0.55;
/// Rice silhouette: capsule radius (so the grain spans the full radius along its axis).
const RICE_RADIUS: f32 = 0.45;
/// Diamond silhouette: half diagonals along and across the flight direction.
const DIAMOND_HALF_DIAGONALS: [f32; 2] = [1.0, 0.62];

/// Rim width in pixels (stylebook v0: 1.5 px).
const RIM_WIDTH_PX: f32 = 1.5;
/// Largest rim width in local units, so a bullet only a few pixels across is not swallowed by
/// its own outline.
const RIM_MAX_LOCAL: f32 = 0.5;
/// Rim colour, sRGB `#0A0510`, and its opacity 0.9 (stylebook v0).
const RIM_SRGB: u32 = 0x0A_05_10;
const RIM_ALPHA: f32 = 0.9;
/// Glow reach beyond the rim at full glow, in local units.
const GLOW_REACH: f32 = 1.0;
/// Halo opacity right outside the rim at full glow.
const GLOW_ALPHA: f32 = 0.55;
/// Depth inside the silhouette (local units) where the white-hot core starts and is complete.
const CORE_START: f32 = 0.22;
const CORE_END: f32 = 0.7;

/// Body and core colours per palette, sRGB, in [`bullet_palette`] order (stylebook v0, table
/// "Gegnerisch (HOSTILE)").
const PALETTE_SRGB: [(u32, u32); bullet_palette::COUNT as usize] = [
    (0xFF_2F_B4, 0xFF_E3_F4), // H0 Hexenmagenta
    (0xB6_FF_2E, 0xF6_FF_E0), // H1 Giftlimette
];

mod gpu_types {
    // bytemuck's derive macros expand to `unsafe impl` blocks, like `SpriteInstance` elsewhere in
    // this crate.
    #![allow(unsafe_code)]

    use crate::bullet_palette;

    /// Camera uniform of `bullet.wgsl` (112 bytes).
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct BulletCameraGpu {
        pub view_proj: [[f32; 4]; 4],
        pub axis_x: [f32; 4],
        pub axis_y: [f32; 4],
        pub viewport: [f32; 4],
    }

    /// Style uniform of `bullet.wgsl` (128 bytes); see the `Style` struct there for the packing.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct BulletStyleGpu {
        pub body: [[f32; 4]; bullet_palette::COUNT as usize],
        pub core: [[f32; 4]; bullet_palette::COUNT as usize],
        pub rim: [f32; 4],
        pub shape: [f32; 4],
        pub core_atlas: [f32; 4],
        pub distance: [f32; 4],
    }
}

use gpu_types::{BulletCameraGpu, BulletStyleGpu};

/// The bullets the pass draws: `bullets` itself while every instance is accepted, otherwise the
/// accepted ones compacted into the reused `staging` buffer. Shared by [`BulletPass::prepare`] and
/// `crate::measurement` (plan 0002 WP3.6).
pub(crate) fn accepted_bullets<'a>(
    bullets: &'a [BulletInstance],
    staging: &'a mut Vec<BulletInstance>,
) -> &'a [BulletInstance] {
    let Some(first) = bullets
        .iter()
        .position(|bullet| !is_accepted_bullet(bullet))
    else {
        return bullets;
    };
    staging.clear();
    staging.extend_from_slice(&bullets[..first]);
    staging.extend(
        bullets[first + 1..]
            .iter()
            .filter(|bullet| is_accepted_bullet(bullet)),
    );
    staging
}

/// How the bullet pass projects the ground plane this frame. The sprite pass draws the player
/// marker through the same projection under the 2.5D camera (plan 0002 WP3.6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BulletView {
    pub(crate) view_proj: [[f32; 4]; 4],
    pub(crate) axis_x: [f32; 3],
    pub(crate) axis_y: [f32; 3],
}

/// The projection of `frame`'s bullets for a target of the given `aspect` ratio: the frame's
/// [`crate::Camera25D`] (with the mesh pass's clip planes, so bullets and meshes line up) when
/// present, otherwise its [`crate::Camera2D`]. `None` if the chosen camera yields a non-finite
/// matrix — then nothing is drawn, exactly like the mesh pass treats an unusable camera.
pub(crate) fn bullet_view(frame: &StageFrame, aspect: f32) -> Option<BulletView> {
    let view = match &frame.camera_25d {
        Some(camera) => {
            let (axis_x, axis_y) = crate::stage3d::billboard_axes(camera);
            BulletView {
                view_proj: crate::stage3d::view_projection(
                    camera,
                    aspect,
                    crate::mesh_pass::NEAR_PLANE,
                    crate::mesh_pass::FAR_PLANE,
                ),
                axis_x,
                axis_y,
            }
        }
        None => BulletView {
            view_proj: frame.base.camera.view_projection(aspect),
            axis_x: [1.0, 0.0, 0.0],
            axis_y: [0.0, 1.0, 0.0],
        },
    };
    let finite = view.view_proj.iter().flatten().all(|c| c.is_finite())
        && view
            .axis_x
            .iter()
            .chain(&view.axis_y)
            .all(|c| c.is_finite());
    finite.then_some(view)
}

/// Converts one sRGB-encoded `0xRRGGBB` colour into linear RGB (the standard sRGB transfer
/// function).
pub(crate) fn srgb_hex_to_linear(hex: u32) -> [f32; 3] {
    let channel = |shift: u32| {
        let encoded = ((hex >> shift) & 0xFF) as f32 / 255.0;
        if encoded <= 0.040_45 {
            encoded / 12.92
        } else {
            ((encoded + 0.055) / 1.055).powf(2.4)
        }
    };
    [channel(16), channel(8), channel(0)]
}

fn style_uniform() -> BulletStyleGpu {
    let linear = |hex: u32| {
        let [r, g, b] = srgb_hex_to_linear(hex);
        [r, g, b, 1.0]
    };
    let [rim_r, rim_g, rim_b] = srgb_hex_to_linear(RIM_SRGB);
    BulletStyleGpu {
        body: PALETTE_SRGB.map(|(body, _)| linear(body)),
        core: PALETTE_SRGB.map(|(_, core)| linear(core)),
        rim: [rim_r, rim_g, rim_b, RIM_ALPHA],
        shape: [RIM_WIDTH_PX, RIM_MAX_LOCAL, GLOW_REACH, GLOW_ALPHA],
        core_atlas: [CORE_START, CORE_END, ATLAS_EXTENT, ATLAS_CELL as f32],
        distance: [DISTANCE_MIN, DISTANCE_MAX, 0.0, 0.0],
    }
}

/// Signed distance from `p` (silhouette-local units, +X along the flight direction) to the outline
/// of `silhouette`: negative inside, zero on the outline, positive outside. An index outside
/// [`bullet_silhouette`] falls back to the orb.
pub(crate) fn silhouette_distance(silhouette: u16, p: [f32; 2]) -> f32 {
    match silhouette {
        bullet_silhouette::RICE => {
            let along = p[0].clamp(-RICE_HALF_LENGTH, RICE_HALF_LENGTH);
            let (dx, dy) = (p[0] - along, p[1]);
            (dx * dx + dy * dy).sqrt() - RICE_RADIUS
        }
        bullet_silhouette::DIAMOND => rhombus_distance(p, DIAMOND_HALF_DIAGONALS),
        _ => (p[0] * p[0] + p[1] * p[1]).sqrt() - 1.0,
    }
}

/// Exact signed distance to a rhombus centred at the origin with half diagonals `b` (the standard
/// closed form by Inigo Quilez).
fn rhombus_distance(p: [f32; 2], b: [f32; 2]) -> f32 {
    let q = [p[0].abs(), p[1].abs()];
    // `ndot(a, b) = a.x * b.x - a.y * b.y`
    let bb = b[0] * b[0] + b[1] * b[1];
    let h = (((b[0] - 2.0 * q[0]) * b[0] - (b[1] - 2.0 * q[1]) * b[1]) / bb).clamp(-1.0, 1.0);
    let dx = q[0] - 0.5 * b[0] * (1.0 - h);
    let dy = q[1] - 0.5 * b[1] * (1.0 + h);
    let distance = (dx * dx + dy * dy).sqrt();
    let side = q[0] * b[1] + q[1] * b[0] - b[0] * b[1];
    if side < 0.0 { -distance } else { distance }
}

/// Bakes the silhouette atlas: [`bullet_silhouette::COUNT`] cells of [`ATLAS_CELL`] texels side by
/// side, one `R8Unorm` texel per signed distance sample at the texel centre, encoded linearly from
/// [`DISTANCE_MIN`] (`0`) to [`DISTANCE_MAX`] (`255`). Row 0 is the top (+Y) edge of each cell.
pub(crate) fn bake_silhouette_atlas() -> Vec<u8> {
    let cell = ATLAS_CELL as usize;
    let count = usize::from(bullet_silhouette::COUNT);
    let mut texels = vec![0u8; cell * count * cell];
    for row in 0..cell {
        let local_y = (1.0 - (row as f32 + 0.5) / cell as f32 * 2.0) * ATLAS_EXTENT;
        for silhouette in 0..count {
            for column in 0..cell {
                let local_x = ((column as f32 + 0.5) / cell as f32 * 2.0 - 1.0) * ATLAS_EXTENT;
                let distance = silhouette_distance(silhouette as u16, [local_x, local_y]);
                let encoded = ((distance - DISTANCE_MIN) / (DISTANCE_MAX - DISTANCE_MIN))
                    .clamp(0.0, 1.0)
                    * 255.0;
                texels[row * cell * count + silhouette * cell + column] = encoded.round() as u8;
            }
        }
    }
    texels
}

/// Capacity (in instances) of the largest instance buffer the device allows.
fn max_instance_capacity(max_buffer_size: u64) -> u32 {
    u32::try_from(max_buffer_size / INSTANCE_SIZE).unwrap_or(u32::MAX)
}

pub(crate) struct BulletPass {
    halo_pipeline: wgpu::RenderPipeline,
    body_pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    capacity: u32,
    /// Accepted instances of a frame that also contained rejected ones; reused across frames.
    staging: Vec<BulletInstance>,
}

impl BulletPass {
    pub(crate) fn new(
        context: &GpuContext,
        target_format: wgpu::TextureFormat,
    ) -> Result<Self, GpuError> {
        let device = context.device();
        let shader = context.capture_errors(|device| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("grimoire bullet shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("bullet.wgsl").into()),
            })
        })?;

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire bullet camera uniform"),
            size: std::mem::size_of::<BulletCameraGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let style_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire bullet style uniform"),
            size: std::mem::size_of::<BulletStyleGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        context
            .queue()
            .write_buffer(&style_buffer, 0, bytemuck::bytes_of(&style_uniform()));

        let atlas_size = wgpu::Extent3d {
            width: ATLAS_CELL * u32::from(bullet_silhouette::COUNT),
            height: ATLAS_CELL,
            depth_or_array_layers: 1,
        };
        let atlas = context.capture_errors(|device| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("grimoire bullet silhouette atlas"),
                size: atlas_size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        })?;
        context.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bake_silhouette_atlas(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas_size.width),
                rows_per_image: Some(atlas_size.height),
            },
            atlas_size,
        );
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("grimoire bullet atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("grimoire bullet layout"),
            entries: &[
                uniform_entry(0),
                uniform_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grimoire bullet bind group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: style_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grimoire bullet pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // Two draws over the same instances (`bullet.wgsl`'s header comment): every halo first,
        // then every rim and body on top, so no glow ever tints another bullet's outline.
        let halo_pipeline = create_pipeline(
            context,
            &layout,
            &shader,
            target_format,
            ("grimoire bullet halo pipeline", "vs_halo", "fs_halo"),
        )?;
        let body_pipeline = create_pipeline(
            context,
            &layout,
            &shader,
            target_format,
            ("grimoire bullet body pipeline", "vs_body", "fs_body"),
        )?;

        let max_capacity = max_instance_capacity(device.limits().max_buffer_size);
        let capacity = INITIAL_CAPACITY.clamp(1, max_capacity.max(1));
        let instance_buffer = create_instance_buffer(context, capacity)?;
        Ok(Self {
            halo_pipeline,
            body_pipeline,
            camera_buffer,
            bind_group,
            instance_buffer,
            capacity,
            staging: Vec::new(),
        })
    }

    /// Uploads the camera and every accepted bullet of `bullets`, growing the instance buffer if
    /// needed. Returns the number of instances to draw.
    pub(crate) fn prepare(
        &mut self,
        context: &GpuContext,
        view: &BulletView,
        viewport: (u32, u32),
        bullets: &[BulletInstance],
    ) -> Result<u32, GpuError> {
        let upload = accepted_bullets(bullets, &mut self.staging);
        let accepted_len = upload.len();
        let count = u32::try_from(accepted_len)
            .map_err(|_| GpuError::Validation(format!("{accepted_len} bullets exceed u32::MAX")))?;
        if count == 0 {
            return Ok(0);
        }

        let max_buffer_size = context.device().limits().max_buffer_size;
        let capacity = crate::sprite_pass::grown_capacity(
            self.capacity,
            count,
            max_instance_capacity(max_buffer_size),
        )
        .ok_or_else(|| {
            GpuError::Validation(format!(
                "{count} bullets exceed the device buffer limit of {max_buffer_size} bytes"
            ))
        })?;
        if capacity != self.capacity {
            log::debug!(
                "growing bullet instance buffer from {} to {capacity}",
                self.capacity
            );
            self.instance_buffer = create_instance_buffer(context, capacity)?;
            self.capacity = capacity;
        }

        let camera = BulletCameraGpu {
            view_proj: view.view_proj,
            axis_x: [view.axis_x[0], view.axis_x[1], view.axis_x[2], 0.0],
            axis_y: [view.axis_y[0], view.axis_y[1], view.axis_y[2], 0.0],
            viewport: [viewport.0 as f32, viewport.1 as f32, 0.0, 0.0],
        };
        let queue = context.queue();
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera));
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(upload));
        Ok(count)
    }

    /// Records the bullet draws into `pass`: every halo, then every rim and body. Returns the
    /// number of draw calls issued (`2`, or `0` without instances).
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, count: u32) -> u32 {
        if count == 0 {
            return 0;
        }
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(
            0,
            self.instance_buffer
                .slice(..u64::from(count) * INSTANCE_SIZE),
        );
        pass.set_pipeline(&self.halo_pipeline);
        pass.draw(0..QUAD_VERTICES, 0..count);
        pass.set_pipeline(&self.body_pipeline);
        pass.draw(0..QUAD_VERTICES, 0..count);
        2
    }
}

/// One bullet pipeline over the shared layout; `(label, vertex entry, fragment entry)`.
fn create_pipeline(
    context: &GpuContext,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    target_format: wgpu::TextureFormat,
    (label, vertex_entry, fragment_entry): (&str, &str, &str),
) -> Result<wgpu::RenderPipeline, GpuError> {
    context.capture_errors(|device| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(vertex_entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_SIZE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &INSTANCE_ATTRIBUTES,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // A rotation may flip the winding.
                cull_mode: None,
                ..Default::default()
            },
            // No depth test: nothing drawn before layer 6 may occlude a bullet.
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fragment_entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    })
}

fn create_instance_buffer(context: &GpuContext, capacity: u32) -> Result<wgpu::Buffer, GpuError> {
    let size = u64::from(capacity) * INSTANCE_SIZE;
    context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire bullet instances"),
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
    fn instance_attributes_match_bullet_instance_layout() {
        let offsets = [
            offset_of!(BulletInstance, position),
            offset_of!(BulletInstance, radius),
            offset_of!(BulletInstance, rotation),
            offset_of!(BulletInstance, silhouette),
            offset_of!(BulletInstance, palette_space),
        ];
        for (attribute, offset) in INSTANCE_ATTRIBUTES.iter().zip(offsets) {
            assert_eq!(attribute.offset, offset as u64);
        }
        let last = &INSTANCE_ATTRIBUTES[4];
        assert_eq!(last.offset + last.format.size(), INSTANCE_SIZE);
        // The packed words the shader unpacks: silhouette | palette << 16, palette_space | glow << 8.
        let bullet = BulletInstance {
            silhouette: 0x1234,
            palette: 0xBEEF,
            palette_space: 0x01,
            glow: 0x7F,
            flags: 0,
            ..BulletInstance::default()
        };
        let bytes = bytemuck::bytes_of(&bullet);
        let ids = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let tail = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        assert_eq!(ids & 0xFFFF, 0x1234);
        assert_eq!(ids >> 16, 0xBEEF);
        assert_eq!(tail & 0xFF, 0x01);
        assert_eq!((tail >> 8) & 0xFF, 0x7F);
    }

    #[test]
    fn uniform_sizes_match_the_shader_structs() {
        assert_eq!(std::mem::size_of::<BulletCameraGpu>(), 112);
        assert_eq!(std::mem::size_of::<BulletStyleGpu>(), 128);
    }

    #[test]
    fn shader_table_sizes_match_the_rust_tables() {
        let shader = include_str!("bullet.wgsl");
        assert!(shader.contains(&format!(
            "const PALETTE_COUNT: u32 = {}u;",
            bullet_palette::COUNT
        )));
        assert!(shader.contains(&format!(
            "const SILHOUETTE_COUNT: u32 = {}u;",
            bullet_silhouette::COUNT
        )));
        assert_eq!(PALETTE_SRGB.len(), usize::from(bullet_palette::COUNT));
    }

    #[test]
    fn srgb_conversion_matches_the_transfer_function() {
        assert_eq!(srgb_hex_to_linear(0x000000), [0.0, 0.0, 0.0]);
        assert_eq!(srgb_hex_to_linear(0xFFFFFF), [1.0, 1.0, 1.0]);
        let [r, g, b] = srgb_hex_to_linear(0x80_0A_FF);
        assert!((r - 0.215_861).abs() < 1e-5);
        assert!((g - 0.003_035).abs() < 1e-5);
        assert!((b - 1.0).abs() < 1e-6);
    }

    #[test]
    fn style_uniform_carries_the_stylebook_palette() {
        // Compared with a tolerance, not bit for bit: the table is evaluated through a different
        // path than this direct call, and an optimised build rounded the last digit differently
        // on one runner (0.4564111 against 0.45641106). This is render colour, not simulation
        // state, so no determinism rule asks for bit equality here.
        fn assert_close(actual: &[f32], expected: [f32; 3]) {
            for (a, e) in actual.iter().zip(expected) {
                assert!((a - e).abs() < 1e-6, "{actual:?} != {expected:?}");
            }
        }
        let style = style_uniform();
        assert_close(
            &style.body[usize::from(bullet_palette::HEX_MAGENTA)][..3],
            srgb_hex_to_linear(0xFF_2F_B4),
        );
        assert_close(
            &style.core[usize::from(bullet_palette::POISON_LIME)][..3],
            srgb_hex_to_linear(0xF6_FF_E0),
        );
        assert_eq!(style.rim[3], RIM_ALPHA);
    }

    #[test]
    fn silhouette_distances_have_the_documented_outlines() {
        use bullet_silhouette::{DIAMOND, ORB, RICE};
        // Orb: unit disc.
        assert!((silhouette_distance(ORB, [0.0, 0.0]) + 1.0).abs() < 1e-6);
        assert!(silhouette_distance(ORB, [1.0, 0.0]).abs() < 1e-6);
        assert!((silhouette_distance(ORB, [0.0, 2.0]) - 1.0).abs() < 1e-6);
        // Rice: spans the full radius along +X, thin across.
        assert!(silhouette_distance(RICE, [1.0, 0.0]).abs() < 1e-6);
        assert!(silhouette_distance(RICE, [0.0, RICE_RADIUS]).abs() < 1e-6);
        assert!(silhouette_distance(RICE, [0.0, 0.8]) > 0.3);
        // Diamond: tips on the axes, straight edges between them.
        assert!(silhouette_distance(DIAMOND, [1.0, 0.0]).abs() < 1e-5);
        assert!(silhouette_distance(DIAMOND, [0.0, 0.62]).abs() < 1e-5);
        assert!(silhouette_distance(DIAMOND, [0.5, 0.31]).abs() < 1e-5);
        let inradius = 0.62 / (1.0f32 + 0.62 * 0.62).sqrt();
        assert!((silhouette_distance(DIAMOND, [0.0, 0.0]) + inradius).abs() < 1e-5);
        assert!((silhouette_distance(DIAMOND, [2.0, 0.0]) - 1.0).abs() < 1e-5);
        // Out-of-table index falls back to the orb (the shader clamps the same way).
        assert_eq!(
            silhouette_distance(bullet_silhouette::COUNT, [0.3, 0.4]),
            silhouette_distance(ORB, [0.3, 0.4])
        );
    }

    fn inside_mask(atlas: &[u8], silhouette: usize) -> Vec<bool> {
        let cell = ATLAS_CELL as usize;
        let count = usize::from(bullet_silhouette::COUNT);
        // Texel value of distance 0.
        let zero = (-DISTANCE_MIN / (DISTANCE_MAX - DISTANCE_MIN) * 255.0).round() as u8;
        (0..cell)
            .flat_map(|row| {
                (0..cell).map(move |column| row * cell * count + silhouette * cell + column)
            })
            .map(|index| atlas[index] < zero)
            .collect()
    }

    #[test]
    fn every_pair_of_silhouettes_differs_clearly_in_the_atlas() {
        // PRD-0003 rule 3, structurally: no two silhouettes may be told apart only by colour. Each
        // pair must disagree on at least 10 % of the texels of their cells about inside/outside.
        let atlas = bake_silhouette_atlas();
        let masks: Vec<Vec<bool>> = (0..usize::from(bullet_silhouette::COUNT))
            .map(|silhouette| inside_mask(&atlas, silhouette))
            .collect();
        for a in 0..masks.len() {
            assert!(
                masks[a].iter().any(|&inside| inside),
                "silhouette {a} is empty"
            );
            for b in (a + 1)..masks.len() {
                let different = masks[a]
                    .iter()
                    .zip(&masks[b])
                    .filter(|(x, y)| x != y)
                    .count();
                let inside_a = masks[a].iter().filter(|&&inside| inside).count();
                assert!(
                    different * 10 >= inside_a,
                    "silhouettes {a} and {b} differ in only {different} texels"
                );
            }
        }
    }

    #[test]
    fn atlas_cell_borders_are_saturated_so_filtering_never_bleeds_a_neighbour() {
        let atlas = bake_silhouette_atlas();
        let cell = ATLAS_CELL as usize;
        let width = cell * usize::from(bullet_silhouette::COUNT);
        for silhouette in 0..usize::from(bullet_silhouette::COUNT) {
            for row in [0, cell - 1] {
                for column in 0..cell {
                    let value = atlas[row * width + silhouette * cell + column];
                    assert!(
                        value > 200,
                        "silhouette {silhouette} border texel ({column}, {row}) is {value}: too close to the outline"
                    );
                }
            }
        }
    }

    #[test]
    fn bullet_view_prefers_the_2_5d_camera_and_rejects_non_finite_ones() {
        let mut frame = StageFrame::new();
        let flat = bullet_view(&frame, 16.0 / 9.0).expect("default 2D camera is finite");
        assert_eq!(flat.axis_y, [0.0, 1.0, 0.0]);

        frame.camera_25d = Some(crate::Camera25D::default());
        let tilted = bullet_view(&frame, 16.0 / 9.0).expect("default 2.5D camera is finite");
        assert_ne!(tilted.axis_y, [0.0, 1.0, 0.0], "tilted camera up vector");
        assert!(tilted.axis_y[2] > 0.0);

        frame.camera_25d = Some(crate::Camera25D {
            tilt_degrees: f32::NAN,
            ..crate::Camera25D::default()
        });
        assert_eq!(bullet_view(&frame, 16.0 / 9.0), None);
    }
}
