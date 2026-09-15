//! GPU side of the spike: pipelines, render targets and the frame graph.
//!
//! Frame graph (identical for every look except step 2):
//! 1. World pass at `ss`x resolution: HDR colour, normal+class MRT, depth.
//! 2. Toon only: screen-space outline pass at `ss`x resolution.
//! 3. 2x2 box downsample to 1280x720 HDR.
//! 4. PostFxResolve: bloom, Khronos PBR Neutral, sRGB write into the `OffscreenTarget` (no look
//!    parameter; the per-look calibration is a light gain inside the world shading).
//! 5. Telegraphy: reserved, empty.
//! 6. Hostile bullet pass (LDR, no depth test). 7. Player marker.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use grimoire_gpu::wgpu::util::DeviceExt;
use grimoire_gpu::{ContextOptions, GpuContext, GpuError, OFFSCREEN_FORMAT, OffscreenTarget, wgpu};

use crate::bullet_pass;
use crate::camera::{self, Camera};
use crate::gpu_types::{
    BoltInstance, BulletGpu, BulletUniform, FrameUniform, LightGpu, MAX_BLOBS, MAX_LIGHTS, OutlineUniform,
    PostUniform, Vertex,
};
use crate::image_io::{Image, f16_to_f32};
use crate::materials::{self as m, hex};
use crate::scene::{PLAYER_POS, Scene};

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const OUT_WIDTH: u32 = 1280;
pub const OUT_HEIGHT: u32 = 720;
pub const BLOOM_LEVELS: usize = 4;
pub const BLOOM_THRESHOLD: f32 = 1.0;
pub const BLOOM_KNEE: f32 = 0.5;
pub const BLOOM_INTENSITY: f32 = 0.08;
/// Bullets are anchored at this plane height and face the camera.
pub const BULLET_PLANE_Z: f32 = 0.5;
pub const MARKER_RADIUS: f32 = 0.8;
pub const MARKER_WIDTH: f32 = 0.05;
/// Class id in the class MRT where nothing was drawn.
pub const CLASS_VOID: u8 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Look {
    Toon,
    Stylized,
    Realistic,
}

impl Look {
    pub const ALL: [Look; 3] = [Look::Toon, Look::Stylized, Look::Realistic];

    pub fn key(self) -> &'static str {
        match self {
            Look::Toon => "toon",
            Look::Stylized => "stylized",
            Look::Realistic => "realistic",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|look| look.key() == s)
    }

    pub fn name_de(self) -> &'static str {
        match self {
            Look::Toon => "Toon (Cel-Shading, 3 Lichtbänder, Outlines)",
            Look::Stylized => "Stilisiertes 3D (weiches Licht, Rimlight, ohne Outlines)",
            Look::Realistic => "Realistischer (GGX-Mikrofacetten, ohne Rim und Outlines)",
        }
    }

    fn fragment_entry(self) -> &'static str {
        match self {
            Look::Toon => "fs_toon",
            Look::Stylized => "fs_stylized",
            Look::Realistic => "fs_realistic",
        }
    }
}

struct Tex {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

fn create_tex(
    ctx: &GpuContext,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> Result<Tex, GpuError> {
    let texture = ctx.capture_errors(|device| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    })?;
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(Tex {
        texture,
        view,
        width,
        height,
    })
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

struct Stage<'a> {
    module: &'a wgpu::ShaderModule,
    vs: &'a str,
    fs: &'a str,
}

struct PipelineSpec<'a> {
    label: &'a str,
    layout: &'a wgpu::PipelineLayout,
    stage: Stage<'a>,
    buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    targets: &'a [Option<wgpu::ColorTargetState>],
    depth: Option<wgpu::DepthStencilState>,
    cull: Option<wgpu::Face>,
}

fn pipeline(ctx: &GpuContext, spec: PipelineSpec<'_>) -> Result<wgpu::RenderPipeline, GpuError> {
    ctx.capture_errors(|device| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(spec.label),
            layout: Some(spec.layout),
            vertex: wgpu::VertexState {
                module: spec.stage.module,
                entry_point: Some(spec.stage.vs),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: spec.buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: spec.cull,
                ..Default::default()
            },
            depth_stencil: spec.depth,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: spec.stage.module,
                entry_point: Some(spec.stage.fs),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: spec.targets,
            }),
            multiview_mask: None,
            cache: None,
        })
    })
}

fn color_attachment(view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        },
    }
}

fn fullscreen(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
    target: &wgpu::TextureView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("look-dev fullscreen pass"),
        color_attachments: &[Some(color_attachment(target, wgpu::LoadOp::Clear(wgpu::Color::BLACK)))],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32, 2 => Float32x3, 3 => Uint32];
const BOLT_ATTRIBUTES: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32, 2 => Float32x2, 3 => Float32, 4 => Float32, 5 => Float32x3, 6 => Float32
];
const BULLET_ATTRIBUTES: [wgpu::VertexAttribute; 6] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32, 2 => Float32, 3 => Uint32, 4 => Uint32, 5 => Float32];

/// Scene data on the GPU for one variant.
pub struct SceneGpu {
    opaque_vertices: wgpu::Buffer,
    opaque_indices: wgpu::Buffer,
    opaque_index_count: u32,
    emissive_vertices: wgpu::Buffer,
    emissive_indices: wgpu::Buffer,
    emissive_index_count: u32,
    bolts: wgpu::Buffer,
    bolt_count: u32,
    frame_buffer: wgpu::Buffer,
    _light_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    bullets: wgpu::Buffer,
    bullet_count: u32,
    bullet_uniform: wgpu::Buffer,
    bullet_group: wgpu::BindGroup,
    frame: FrameUniform,
}

/// Render targets of one supersampling factor.
pub struct Targets {
    pub ss: u32,
    hdr: Tex,
    nc: Tex,
    depth: Tex,
    outline: Tex,
    down: Tex,
    bloom: Vec<Tex>,
    up: Vec<Tex>,
    pub out: OffscreenTarget,
}

/// Class MRT decoded at output resolution (one sample per output pixel).
pub struct ClassMask {
    pub width: u32,
    pub height: u32,
    pub class: Vec<u8>,
    /// Figure index + 1, 0 = no figure.
    pub figure: Vec<u8>,
    /// Floor pixels inside the ritual decal or a blob shadow.
    pub excluded: Vec<bool>,
}

impl ClassMask {
    /// Lossless RGBA8 encoding: R = class id, G = figure index + 1, B = 255 on excluded floor pixels.
    pub fn to_image(&self) -> Image {
        let mut image = Image::new(self.width, self.height);
        for (i, pixel) in image.rgba.chunks_exact_mut(4).enumerate() {
            pixel.copy_from_slice(&[self.class[i], self.figure[i], if self.excluded[i] { 255 } else { 0 }, 255]);
        }
        image
    }

    pub fn from_image(image: &Image) -> Self {
        let pixels = image.rgba.chunks_exact(4);
        Self {
            width: image.width,
            height: image.height,
            class: pixels.clone().map(|p| p[0]).collect(),
            figure: pixels.clone().map(|p| p[1]).collect(),
            excluded: pixels.map(|p| p[2] > 127).collect(),
        }
    }
}

/// Khronos PBR Neutral on the CPU (reference for the test of [`pbr_neutral_inverse`]).
#[cfg(test)]
fn pbr_neutral(color: [f32; 3]) -> [f32; 3] {
    const START_COMPRESSION: f32 = 0.8 - 0.04;
    const DESATURATION: f32 = 0.15;
    let x = color[0].min(color[1]).min(color[2]);
    let offset = if x < 0.08 { x - 6.25 * x * x } else { 0.04 };
    let c = color.map(|v| v - offset);
    let peak = c[0].max(c[1]).max(c[2]);
    if peak < START_COMPRESSION {
        return c;
    }
    let d = 1.0 - START_COMPRESSION;
    let new_peak = 1.0 - d * d / (peak + d - START_COMPRESSION);
    let g = 1.0 - 1.0 / (DESATURATION * (peak - new_peak) + 1.0);
    c.map(|v| {
        let scaled = v * new_peak / peak;
        scaled + (new_peak - scaled) * g
    })
}

/// Linear HDR colour that PBR Neutral maps to `target`, valid below its compression start (the
/// dark void colour). The toe removes `x - 6.25 x^2` for a minimum channel `x < 0.08`.
pub fn pbr_neutral_inverse(target: [f32; 3]) -> [f32; 3] {
    let t_min = target[0].min(target[1]).min(target[2]).max(0.0);
    let offset = if t_min < 0.04 { (t_min / 6.25).sqrt() - t_min } else { 0.04 };
    target.map(|v| v + offset)
}

pub struct Renderer {
    ctx: GpuContext,
    frame_layout: wgpu::BindGroupLayout,
    world: Vec<wgpu::RenderPipeline>,
    emissive: wgpu::RenderPipeline,
    bolt_pipeline: wgpu::RenderPipeline,
    outline_layout: wgpu::BindGroupLayout,
    outline: wgpu::RenderPipeline,
    outline_buffer: wgpu::Buffer,
    post_layout: wgpu::BindGroupLayout,
    downsample: wgpu::RenderPipeline,
    prefilter: wgpu::RenderPipeline,
    bloom_down: wgpu::RenderPipeline,
    bloom_up: wgpu::RenderPipeline,
    resolve: wgpu::RenderPipeline,
    post_buffer: wgpu::Buffer,
    bullet_layout: wgpu::BindGroupLayout,
    glow: wgpu::RenderPipeline,
    body: wgpu::RenderPipeline,
    marker: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    material_buffer: wgpu::Buffer,
    dummy: Tex,
}

impl Renderer {
    pub fn new() -> Result<Self, GpuError> {
        let ctx = GpuContext::new_offscreen(ContextOptions {
            high_performance: true,
            allow_software_fallback: false,
            vsync: false,
        })?;
        let device = ctx.device();
        let shader = |label: &str, source: &'static str| {
            ctx.capture_errors(|device| {
                device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(label),
                    source: wgpu::ShaderSource::Wgsl(source.into()),
                })
            })
        };
        let world_module = shader("look-dev world", include_str!("shaders/world.wgsl"))?;
        let outline_module = shader("look-dev outline", include_str!("shaders/outline.wgsl"))?;
        let post_module = shader("look-dev post", include_str!("shaders/post.wgsl"))?;
        let bullet_module = shader("look-dev bullets", include_str!("shaders/bullets.wgsl"))?;

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("look-dev frame layout"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                uniform_entry(1, wgpu::ShaderStages::FRAGMENT),
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let frame_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("look-dev world layout"),
            bind_group_layouts: &[Some(&frame_layout)],
            immediate_size: 0,
        });

        let hdr_target = Some(wgpu::ColorTargetState {
            format: HDR_FORMAT,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        });
        let world_targets = [hdr_target.clone(), hdr_target.clone()];
        let depth_write = wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let depth_test = wgpu::DepthStencilState {
            depth_write_enabled: Some(false),
            ..depth_write.clone()
        };
        let mesh_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &VERTEX_ATTRIBUTES,
        })];

        let mut world = Vec::new();
        for look in Look::ALL {
            let label = format!("look-dev world {}", look.key());
            world.push(pipeline(
                &ctx,
                PipelineSpec {
                    label: &label,
                    layout: &frame_pipeline_layout,
                    stage: Stage {
                        module: &world_module,
                        vs: "vs_world",
                        fs: look.fragment_entry(),
                    },
                    buffers: &mesh_buffers,
                    targets: &world_targets,
                    depth: Some(depth_write.clone()),
                    cull: Some(wgpu::Face::Back),
                },
            )?);
        }
        let emissive = pipeline(
            &ctx,
            PipelineSpec {
                label: "look-dev emissive",
                layout: &frame_pipeline_layout,
                stage: Stage {
                    module: &world_module,
                    vs: "vs_world",
                    fs: "fs_emissive",
                },
                buffers: &mesh_buffers,
                targets: &world_targets,
                depth: Some(depth_write.clone()),
                cull: Some(wgpu::Face::Back),
            },
        )?;
        let bolt_targets = [
            Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::empty(),
            }),
        ];
        let bolt_pipeline = pipeline(
            &ctx,
            PipelineSpec {
                label: "look-dev emissive billboards",
                layout: &frame_pipeline_layout,
                stage: Stage {
                    module: &world_module,
                    vs: "vs_bolt",
                    fs: "fs_emissive_bolt",
                },
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<BoltInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &BOLT_ATTRIBUTES,
                })],
                targets: &bolt_targets,
                depth: Some(depth_test),
                cull: None,
            },
        )?;

        let outline_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("look-dev outline layout"),
            entries: &[
                texture_entry(0, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(1, wgpu::TextureSampleType::Depth),
                texture_entry(2, wgpu::TextureSampleType::Float { filterable: false }),
                uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let outline_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("look-dev outline pipeline layout"),
            bind_group_layouts: &[Some(&outline_layout)],
            immediate_size: 0,
        });
        let outline = pipeline(
            &ctx,
            PipelineSpec {
                label: "look-dev outline",
                layout: &outline_pipeline_layout,
                stage: Stage {
                    module: &outline_module,
                    vs: "vs_fullscreen",
                    fs: "fs_outline",
                },
                buffers: &[],
                targets: &[hdr_target.clone()],
                depth: None,
                cull: None,
            },
        )?;

        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("look-dev post layout"),
            entries: &[
                texture_entry(0, wgpu::TextureSampleType::Float { filterable: true }),
                texture_entry(1, wgpu::TextureSampleType::Float { filterable: true }),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(3, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let post_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("look-dev post pipeline layout"),
            bind_group_layouts: &[Some(&post_layout)],
            immediate_size: 0,
        });
        let post_pipeline = |label: &str, fs: &str, format: wgpu::TextureFormat| {
            pipeline(
                &ctx,
                PipelineSpec {
                    label,
                    layout: &post_pipeline_layout,
                    stage: Stage {
                        module: &post_module,
                        vs: "vs_fullscreen",
                        fs,
                    },
                    buffers: &[],
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    depth: None,
                    cull: None,
                },
            )
        };
        let downsample = post_pipeline("look-dev downsample", "fs_downsample", HDR_FORMAT)?;
        let prefilter = post_pipeline("look-dev bloom prefilter", "fs_prefilter", HDR_FORMAT)?;
        let bloom_down = post_pipeline("look-dev bloom down", "fs_down", HDR_FORMAT)?;
        let bloom_up = post_pipeline("look-dev bloom up", "fs_up", HDR_FORMAT)?;
        let resolve = post_pipeline("look-dev resolve", "fs_resolve", OFFSCREEN_FORMAT)?;

        let bullet_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("look-dev bullet layout"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let bullet_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("look-dev bullet pipeline layout"),
            bind_group_layouts: &[Some(&bullet_layout)],
            immediate_size: 0,
        });
        let bullet_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BulletGpu>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &BULLET_ATTRIBUTES,
        })];
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        };
        let ldr = |blend: wgpu::BlendState| {
            [Some(wgpu::ColorTargetState {
                format: OFFSCREEN_FORMAT,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })]
        };
        let glow = pipeline(
            &ctx,
            PipelineSpec {
                label: "look-dev bullet glow",
                layout: &bullet_pipeline_layout,
                stage: Stage {
                    module: &bullet_module,
                    vs: "vs_glow",
                    fs: "fs_glow",
                },
                buffers: &bullet_buffers,
                targets: &ldr(additive),
                depth: None,
                cull: None,
            },
        )?;
        let body = pipeline(
            &ctx,
            PipelineSpec {
                label: "look-dev bullet body",
                layout: &bullet_pipeline_layout,
                stage: Stage {
                    module: &bullet_module,
                    vs: "vs_body",
                    fs: "fs_body",
                },
                buffers: &bullet_buffers,
                targets: &ldr(wgpu::BlendState::ALPHA_BLENDING),
                depth: None,
                cull: None,
            },
        )?;
        let marker = pipeline(
            &ctx,
            PipelineSpec {
                label: "look-dev player marker",
                layout: &bullet_pipeline_layout,
                stage: Stage {
                    module: &bullet_module,
                    vs: "vs_marker",
                    fs: "fs_marker",
                },
                buffers: &[],
                targets: &ldr(wgpu::BlendState::ALPHA_BLENDING),
                depth: None,
                cull: None,
            },
        )?;

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("look-dev linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let materials = m::material_table();
        let rims = m::rim_table();
        let mut material_bytes = Vec::new();
        material_bytes.extend_from_slice(bytemuck::cast_slice(&materials[..]));
        material_bytes.extend_from_slice(bytemuck::cast_slice(&rims[..]));
        let material_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("look-dev materials"),
            contents: &material_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let uniform_buffer = |label: &str, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let outline_buffer = uniform_buffer("look-dev outline params", std::mem::size_of::<OutlineUniform>());
        let post_buffer = uniform_buffer("look-dev post params", std::mem::size_of::<PostUniform>());
        let dummy = create_tex(&ctx, "look-dev dummy", 1, 1, HDR_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING)?;

        Ok(Self {
            ctx,
            frame_layout,
            world,
            emissive,
            bolt_pipeline,
            outline_layout,
            outline,
            outline_buffer,
            post_layout,
            downsample,
            prefilter,
            bloom_down,
            bloom_up,
            resolve,
            post_buffer,
            bullet_layout,
            glow,
            body,
            marker,
            sampler,
            material_buffer,
            dummy,
        })
    }

    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    pub fn adapter_summary(&self) -> String {
        let info = self.ctx.adapter_info();
        format!("{} ({:?}, {})", info.name, info.device_type, self.ctx.backend_name())
    }

    pub fn is_cpu_adapter(&self) -> bool {
        self.ctx.adapter_info().device_type == wgpu::DeviceType::Cpu
    }

    pub fn upload_scene(&self, scene: &Scene, camera: &Camera) -> Result<SceneGpu, GpuError> {
        assert!(scene.lights.len() <= MAX_LIGHTS, "at most {MAX_LIGHTS} lights fit the uniform array");
        let device = self.ctx.device();
        let init = |label: &str, contents: &[u8], usage: wgpu::BufferUsages| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let frame = frame_uniform(scene, camera);
        let mut lights = [LightGpu::default(); MAX_LIGHTS];
        lights[..scene.lights.len()].copy_from_slice(&scene.lights);
        let frame_buffer = init(
            "look-dev frame",
            bytemuck::bytes_of(&frame),
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let light_buffer = init("look-dev lights", bytemuck::cast_slice(&lights[..]), wgpu::BufferUsages::UNIFORM);
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("look-dev frame group"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.material_buffer.as_entire_binding(),
                },
            ],
        });
        let bolt_data: Vec<BoltInstance> = if scene.bolts.is_empty() {
            vec![BoltInstance::default()]
        } else {
            scene.bolts.clone()
        };
        let (accepted, _) = bullet_pass::accept(&scene.bullets);
        let bullet_data = if accepted.is_empty() {
            vec![BulletGpu::default()]
        } else {
            accepted.clone()
        };
        let bullet_uniform = init(
            "look-dev bullet params",
            bytemuck::bytes_of(&bullet_uniform(camera)),
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let bullet_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("look-dev bullet group"),
            layout: &self.bullet_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: bullet_uniform.as_entire_binding(),
            }],
        });
        let count = |n: usize| u32::try_from(n).expect("count fits u32");
        Ok(SceneGpu {
            opaque_vertices: init(
                "look-dev opaque vertices",
                bytemuck::cast_slice(&scene.opaque.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            opaque_indices: init(
                "look-dev opaque indices",
                bytemuck::cast_slice(&scene.opaque.indices),
                wgpu::BufferUsages::INDEX,
            ),
            opaque_index_count: count(scene.opaque.indices.len()),
            emissive_vertices: init(
                "look-dev emissive vertices",
                bytemuck::cast_slice(&scene.emissive.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            emissive_indices: init(
                "look-dev emissive indices",
                bytemuck::cast_slice(&scene.emissive.indices),
                wgpu::BufferUsages::INDEX,
            ),
            emissive_index_count: count(scene.emissive.indices.len()),
            bolts: init("look-dev bolts", bytemuck::cast_slice(&bolt_data), wgpu::BufferUsages::VERTEX),
            bolt_count: count(scene.bolts.len()),
            frame_buffer,
            _light_buffer: light_buffer,
            frame_group,
            bullets: init("look-dev bullets", bytemuck::cast_slice(&bullet_data), wgpu::BufferUsages::VERTEX),
            bullet_count: count(accepted.len()),
            bullet_uniform,
            bullet_group,
            frame,
        })
    }

    /// Replaces the camera of an uploaded scene (sway sequence).
    pub fn set_camera(&self, scene: &mut SceneGpu, camera: &Camera) {
        scene.frame.view_proj = camera.view_proj;
        scene.frame.view = camera.view;
        scene.frame.eye = camera.eye;
        scene.frame.cam_right = camera.right;
        scene.frame.cam_up = camera.up;
        let queue = self.ctx.queue();
        queue.write_buffer(&scene.frame_buffer, 0, bytemuck::bytes_of(&scene.frame));
        queue.write_buffer(&scene.bullet_uniform, 0, bytemuck::bytes_of(&bullet_uniform(camera)));
    }

    pub fn targets(&self, ss: u32) -> Result<Targets, GpuError> {
        use wgpu::TextureUsages as U;
        let (w, h) = (OUT_WIDTH * ss, OUT_HEIGHT * ss);
        let rt = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING;
        let ctx = &self.ctx;
        let mut bloom = Vec::new();
        let mut up = Vec::new();
        for level in 0..BLOOM_LEVELS {
            let (lw, lh) = (OUT_WIDTH >> (level + 1), OUT_HEIGHT >> (level + 1));
            bloom.push(create_tex(ctx, "look-dev bloom down", lw, lh, HDR_FORMAT, rt)?);
            if level + 1 < BLOOM_LEVELS {
                up.push(create_tex(ctx, "look-dev bloom up", lw, lh, HDR_FORMAT, rt)?);
            }
        }
        Ok(Targets {
            ss,
            hdr: create_tex(ctx, "look-dev hdr", w, h, HDR_FORMAT, rt)?,
            nc: create_tex(ctx, "look-dev normal class", w, h, HDR_FORMAT, rt | U::COPY_SRC)?,
            depth: create_tex(ctx, "look-dev depth", w, h, DEPTH_FORMAT, rt)?,
            outline: create_tex(ctx, "look-dev outline", w, h, HDR_FORMAT, rt)?,
            down: create_tex(ctx, "look-dev downsampled", OUT_WIDTH, OUT_HEIGHT, HDR_FORMAT, rt)?,
            bloom,
            up,
            out: OffscreenTarget::new(ctx, OUT_WIDTH, OUT_HEIGHT)?,
        })
    }

    fn encode(
        &self,
        label: &str,
        record: impl FnOnce(&mut wgpu::CommandEncoder),
    ) -> Result<wgpu::CommandBuffer, GpuError> {
        self.ctx.capture_errors(|device| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
            record(&mut encoder);
            encoder.finish()
        })
    }

    /// Wall time of queue submit plus a blocking poll until the work has finished.
    fn submit_wait(&self, commands: wgpu::CommandBuffer) -> Result<Duration, GpuError> {
        let start = Instant::now();
        let index = self.ctx.queue().submit([commands]);
        self.ctx
            .device()
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: None,
            })
            .map_err(|error| GpuError::Validation(error.to_string()))?;
        Ok(start.elapsed())
    }

    /// Step 1: world layer. `gain` is the look's calibrated light gain; the shader multiplies the
    /// light-derived terms by `LIGHT_INTENSITY_FACTOR * gain`. The void clear colour is the tonemap
    /// inverse of #06070A, so the background resolves to it in every look (before bloom).
    pub fn world_pass(&self, look: Look, scene: &SceneGpu, t: &Targets, gain: f32) -> Result<Duration, GpuError> {
        let mut frame = scene.frame;
        frame.light_gain = m::LIGHT_INTENSITY_FACTOR * gain;
        self.ctx
            .queue()
            .write_buffer(&scene.frame_buffer, 0, bytemuck::bytes_of(&frame));
        let void = pbr_neutral_inverse(hex(m::VOID_HEX));
        let clear = wgpu::Color {
            r: f64::from(void[0]),
            g: f64::from(void[1]),
            b: f64::from(void[2]),
            a: 1.0,
        };
        let void_class = wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: f64::from(CLASS_VOID),
        };
        let commands = self.encode("look-dev world", |encoder| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("look-dev world pass"),
                color_attachments: &[
                    Some(color_attachment(&t.hdr.view, wgpu::LoadOp::Clear(clear))),
                    Some(color_attachment(&t.nc.view, wgpu::LoadOp::Clear(void_class))),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &scene.frame_group, &[]);
            pass.set_pipeline(&self.world[look as usize]);
            pass.set_vertex_buffer(0, scene.opaque_vertices.slice(..));
            pass.set_index_buffer(scene.opaque_indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..scene.opaque_index_count, 0, 0..1);
            pass.set_pipeline(&self.emissive);
            pass.set_vertex_buffer(0, scene.emissive_vertices.slice(..));
            pass.set_index_buffer(scene.emissive_indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..scene.emissive_index_count, 0, 0..1);
            if scene.bolt_count > 0 {
                pass.set_pipeline(&self.bolt_pipeline);
                pass.set_vertex_buffer(0, scene.bolts.slice(..));
                pass.draw(0..6, 0..scene.bolt_count);
            }
        })?;
        self.submit_wait(commands)
    }

    /// Step 2 (toon only): screen-space outlines on the supersampled targets.
    pub fn outline_pass(&self, t: &Targets) -> Result<Duration, GpuError> {
        // 2x SSAA: taps -1/+2 = 3 SS px (about 1.5 px final). 1x: taps -1/+1 = 2 px.
        let (tap_lo, tap_hi) = if t.ss >= 2 { (-1, 2) } else { (-1, 1) };
        let params = OutlineUniform {
            near: camera::NEAR,
            far: camera::FAR,
            tap_lo,
            tap_hi,
        };
        self.ctx
            .queue()
            .write_buffer(&self.outline_buffer, 0, bytemuck::bytes_of(&params));
        let group = self.ctx.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("look-dev outline group"),
            layout: &self.outline_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&t.hdr.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&t.depth.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&t.nc.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.outline_buffer.as_entire_binding(),
                },
            ],
        });
        let commands = self.encode("look-dev outline", |encoder| {
            fullscreen(encoder, &self.outline, &group, &t.outline.view);
        })?;
        self.submit_wait(commands)
    }

    fn post_group(&self, src: &Tex, add: &Tex) -> wgpu::BindGroup {
        self.ctx.device().create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("look-dev post group"),
            layout: &self.post_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&src.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&add.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.post_buffer.as_entire_binding(),
                },
            ],
        })
    }

    /// Steps 3 and 4: downsample, bloom chain and resolve into `t.out`. No look parameter enters
    /// the stack; `look` only selects the outline output as the source for toon.
    pub fn post_pass(&self, look: Look, t: &Targets) -> Result<Duration, GpuError> {
        let params = PostUniform {
            bloom_intensity: BLOOM_INTENSITY,
            threshold: BLOOM_THRESHOLD,
            knee: BLOOM_KNEE,
            ss_factor: t.ss,
            bloom_levels: BLOOM_LEVELS as f32,
            _pad: [0.0; 3],
        };
        self.ctx.queue().write_buffer(&self.post_buffer, 0, bytemuck::bytes_of(&params));
        let src = if look == Look::Toon { &t.outline } else { &t.hdr };
        let d = &self.dummy;
        let steps = [
            (&self.downsample, self.post_group(src, d), &t.down.view),
            (&self.prefilter, self.post_group(&t.down, d), &t.bloom[0].view),
            (&self.bloom_down, self.post_group(&t.bloom[0], d), &t.bloom[1].view),
            (&self.bloom_down, self.post_group(&t.bloom[1], d), &t.bloom[2].view),
            (&self.bloom_down, self.post_group(&t.bloom[2], d), &t.bloom[3].view),
            (&self.bloom_up, self.post_group(&t.bloom[3], &t.bloom[2]), &t.up[2].view),
            (&self.bloom_up, self.post_group(&t.up[2], &t.bloom[1]), &t.up[1].view),
            (&self.bloom_up, self.post_group(&t.up[1], &t.bloom[0]), &t.up[0].view),
            (&self.resolve, self.post_group(&t.down, &t.up[0]), t.out.view()),
        ];
        let commands = self.encode("look-dev post", |encoder| {
            for (pipeline, group, target) in &steps {
                fullscreen(encoder, pipeline, group, target);
            }
        })?;
        self.submit_wait(commands)
    }

    /// Steps 5 to 7: (empty telegraphy slot), hostile bullets with their own glow, player marker.
    pub fn bullet_pass(&self, scene: &SceneGpu, target: &OffscreenTarget, clear: bool) -> Result<Duration, GpuError> {
        let load = if clear {
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
        } else {
            wgpu::LoadOp::Load
        };
        let commands = self.encode("look-dev bullets", |encoder| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("look-dev bullet pass"),
                color_attachments: &[Some(color_attachment(target.view(), load))],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &scene.bullet_group, &[]);
            if scene.bullet_count > 0 {
                pass.set_vertex_buffer(0, scene.bullets.slice(..));
                // All glow halos first, so no halo ever tints a body.
                pass.set_pipeline(&self.glow);
                pass.draw(0..6, 0..scene.bullet_count);
                pass.set_pipeline(&self.body);
                pass.draw(0..6, 0..scene.bullet_count);
            }
            pass.set_pipeline(&self.marker);
            pass.draw(0..6, 0..1);
        })?;
        self.submit_wait(commands)
    }

    pub fn read(&self, target: &OffscreenTarget) -> Result<Image, GpuError> {
        Ok(Image::from_rgba(target.width(), target.height(), target.read_rgba(&self.ctx)?))
    }

    fn read_texture(&self, tex: &Tex, bytes_per_pixel: u32) -> Result<Vec<u8>, GpuError> {
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let unpadded = tex.width * bytes_per_pixel;
        let padded = unpadded.div_ceil(align) * align;
        let staging = self.ctx.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("look-dev texture read-back"),
                size: u64::from(padded) * u64::from(tex.height),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;
        let commands = self.encode("look-dev read-back", |encoder| {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(tex.height),
                    },
                },
                wgpu::Extent3d {
                    width: tex.width,
                    height: tex.height,
                    depth_or_array_layers: 1,
                },
            );
        })?;
        self.submit_wait(commands)?;
        let (sender, receiver) = mpsc::channel();
        staging.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        self.ctx
            .device()
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|error| GpuError::Readback(error.to_string()))?;
        receiver
            .recv()
            .map_err(|error| GpuError::Readback(error.to_string()))?
            .map_err(|error| GpuError::Readback(error.to_string()))?;
        let data = {
            let mapped = staging
                .get_mapped_range(..)
                .map_err(|error| GpuError::Readback(error.to_string()))?;
            let mut out = Vec::with_capacity(unpadded as usize * tex.height as usize);
            for row in mapped.chunks(padded as usize).take(tex.height as usize) {
                out.extend_from_slice(&row[..unpadded as usize]);
            }
            out
        };
        staging.unmap();
        Ok(data)
    }

    /// Decodes the class MRT of the last world pass, one (top-left) sample per output pixel.
    pub fn read_class_mask(&self, t: &Targets) -> Result<ClassMask, GpuError> {
        let bytes = self.read_texture(&t.nc, 8)?;
        let (w, h) = (OUT_WIDTH, OUT_HEIGHT);
        let n = w as usize * h as usize;
        let mut mask = ClassMask {
            width: w,
            height: h,
            class: vec![CLASS_VOID; n],
            figure: vec![0; n],
            excluded: vec![false; n],
        };
        for y in 0..h {
            for x in 0..w {
                let o = ((y * t.ss) as usize * t.nc.width as usize + (x * t.ss) as usize) * 8;
                let a = f16_to_f32(u16::from_le_bytes([bytes[o + 6], bytes[o + 7]]));
                let class = (a + 0.001).floor().clamp(0.0, 15.0);
                let frac = a - class;
                let i = y as usize * w as usize + x as usize;
                mask.class[i] = class as u8;
                if class == 0.0 {
                    mask.excluded[i] = frac > 0.25;
                } else {
                    mask.figure[i] = (frac * 16.0).round().clamp(0.0, 15.0) as u8;
                }
            }
        }
        Ok(mask)
    }
}

fn frame_uniform(scene: &Scene, camera: &Camera) -> FrameUniform {
    let mut blobs = [[0.0; 4]; MAX_BLOBS];
    for (slot, figure) in blobs.iter_mut().zip(&scene.figures) {
        *slot = [figure.position[0], figure.position[1], 1.2 * figure.footprint, 0.0];
    }
    FrameUniform {
        view_proj: camera.view_proj,
        view: camera.view,
        eye: camera.eye,
        light_count: u32::try_from(scene.lights.len()).expect("light count fits u32"),
        key_dir: camera::normalize(camera::scale(m::MOON_DIRECTION, -1.0)),
        key_intensity: m::MOON_INTENSITY,
        key_color: hex(m::MOON_HEX),
        ambient_intensity: m::AMBIENT_INTENSITY,
        sky: hex(m::SKY_HEX),
        decal_emission: scene.decal_emission,
        ground: hex(m::GROUND_HEX),
        blob_count: scene.figures.len().min(MAX_BLOBS) as u32,
        cam_right: camera.right,
        near: camera::NEAR,
        cam_up: camera.up,
        far: camera::FAR,
        decal_color: hex(m::DECAL_HEX),
        floor_salt: scene.floor_salt,
        blobs,
        // Replaced per look in `world_pass`.
        light_gain: m::LIGHT_INTENSITY_FACTOR,
        _pad: [0.0; 3],
    }
}

fn bullet_uniform(camera: &Camera) -> BulletUniform {
    let rgba = |h: u32, a: f32| {
        let c = hex(h);
        [c[0], c[1], c[2], a]
    };
    let palette = &m::HOSTILE_PALETTE;
    BulletUniform {
        view_proj: camera.view_proj,
        cam_right: camera.right,
        plane_z: BULLET_PLANE_Z,
        cam_up: camera.up,
        rim_px: m::BULLET_RIM_PX,
        body: [rgba(palette[0].body_hex, 1.0), rgba(palette[1].body_hex, 1.0)],
        core: [rgba(palette[0].core_hex, 1.0), rgba(palette[1].core_hex, 1.0)],
        rim: rgba(m::BULLET_RIM_HEX, m::BULLET_RIM_ALPHA),
        marker: rgba(m::MARKER_HEX, m::MARKER_ALPHA),
        marker_ring: [PLAYER_POS[0], PLAYER_POS[1], MARKER_RADIUS, MARKER_WIDTH],
        _pad: [0.0; 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn void_colour_survives_the_tonemap() {
        let target = hex(m::VOID_HEX);
        let back = pbr_neutral(pbr_neutral_inverse(target));
        for c in 0..3 {
            assert!((back[c] - target[c]).abs() < 1e-6, "{back:?} vs {target:?}");
        }
        let grey = [0.3, 0.3, 0.3];
        let back = pbr_neutral(pbr_neutral_inverse(grey));
        assert!((back[0] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn class_mask_round_trips_through_rgba() {
        let mask = ClassMask {
            width: 3,
            height: 1,
            class: vec![0, 3, CLASS_VOID],
            figure: vec![0, 9, 0],
            excluded: vec![true, false, false],
        };
        let back = ClassMask::from_image(&mask.to_image());
        assert_eq!(back.class, mask.class);
        assert_eq!(back.figure, mask.figure);
        assert_eq!(back.excluded, mask.excluded);
    }
}
