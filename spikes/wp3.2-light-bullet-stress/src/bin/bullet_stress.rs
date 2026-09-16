//! OF-3.3 spike measurement (plan 0002 WP3.2): billboard-impostor vs. instanced low-poly-mesh
//! bullets at 10k/20k instances under a tilted 2.5D-style camera. Measures, per repetition:
//! extraction (CPU), upload (CPU), and a *relative-only* GPU proxy (encode + submit + poll wall
//! time on the software adapter — never an absolute-millisecond judgement, see README.md).
//!
//! Every number this binary prints is a "Runner-Wert": only meaningful as measured on the CI
//! Linux runner this spike's workflow runs on, and only as a trend (engine ADR-0010) — median of
//! `REPS` repetitions plus the min/max spread, after `WARMUP` discarded warm-up repetitions.

use std::time::Instant;

use grimoire_gpu::{GpuContext, GpuError, wgpu};
use grimoire_render::procedural;
use light_bullet_stress::bullets::{
    MeshBulletInstance, SourceBullet, extract_billboard, extract_mesh, synthetic_bullets,
};
use light_bullet_stress::camera::TiltedCamera;
use light_bullet_stress::gpu::elevated_context;
use light_bullet_stress::stats::Summary;

const WARMUP: usize = 3;
const REPS: usize = 10;
const INSTANCE_COUNTS: [usize; 2] = [10_000, 20_000];
const TARGET_WIDTH: u32 = 320;
const TARGET_HEIGHT: u32 = 180;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[allow(unsafe_code)] // bytemuck derive, same rationale as grimoire_render's own GPU-upload types
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BillboardCameraUniform {
    view_proj: [[f32; 4]; 4],
    right: [f32; 4],
    up: [f32; 4],
}

#[allow(unsafe_code)]
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshCameraUniform {
    view_proj: [[f32; 4]; 4],
}

fn tilted_camera() -> TiltedCamera {
    TiltedCamera {
        target: [0.0, 0.0],
        tilt_degrees: 65.0,
        distance: 20.0,
        fov_y_degrees: 45.0,
        aspect: TARGET_WIDTH as f32 / TARGET_HEIGHT as f32,
    }
}

fn create_depth_view(context: &GpuContext) -> Result<wgpu::TextureView, GpuError> {
    let texture = context.capture_errors(|device| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("wp3.2 bullet stress depth buffer"),
            size: wgpu::Extent3d {
                width: TARGET_WIDTH,
                height: TARGET_HEIGHT,
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

fn create_uniform_buffer(
    context: &GpuContext,
    size: u64,
    label: &'static str,
) -> Result<wgpu::Buffer, GpuError> {
    context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })
}

fn create_camera_bind_group_layout(
    context: &GpuContext,
    label: &'static str,
) -> wgpu::BindGroupLayout {
    context
        .device()
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
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
        })
}

fn create_camera_bind_group(
    context: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    label: &'static str,
) -> wgpu::BindGroup {
    context
        .device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
}

fn create_instance_buffer(
    context: &GpuContext,
    size: u64,
    label: &'static str,
) -> Result<wgpu::Buffer, GpuError> {
    context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size.max(4),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })
}

/// Billboard-impostor scene: one draw call per repetition, no geometry buffer (the quad is
/// generated from the vertex index, same trick as `grimoire_render::sprite.wgsl`).
struct BillboardScene {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
}

impl BillboardScene {
    fn new(
        context: &GpuContext,
        color_format: wgpu::TextureFormat,
        capacity: usize,
    ) -> Result<Self, GpuError> {
        let device = context.device();
        let shader = context.capture_errors(|device| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("wp3.2 billboard shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("billboard.wgsl").into()),
            })
        })?;
        let bind_group_layout =
            create_camera_bind_group_layout(context, "wp3.2 billboard camera layout");
        let camera_buffer = create_uniform_buffer(
            context,
            std::mem::size_of::<BillboardCameraUniform>() as u64,
            "wp3.2 billboard camera uniform",
        )?;
        let bind_group = create_camera_bind_group(
            context,
            &bind_group_layout,
            &camera_buffer,
            "wp3.2 billboard camera bind group",
        );
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wp3.2 billboard pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let instance_attributes: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
            0 => Float32x2,
            1 => Float32,
            2 => Float32,
            3 => Uint32x2,
        ];
        let instance_size = std::mem::size_of::<grimoire_render::BulletInstance>() as u64;
        let pipeline = context.capture_errors(|device| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("wp3.2 billboard pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: instance_size,
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
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })?;
        let instance_buffer = create_instance_buffer(
            context,
            capacity as u64 * instance_size,
            "wp3.2 billboard instances",
        )?;
        Ok(Self {
            pipeline,
            camera_buffer,
            bind_group,
            instance_buffer,
        })
    }

    fn upload_camera(&self, context: &GpuContext, camera: &TiltedCamera) {
        let (right, up) = camera.right_up();
        let uniform = BillboardCameraUniform {
            view_proj: to_columns(camera.view_projection()),
            right: [right[0], right[1], right[2], 0.0],
            up: [up[0], up[1], up[2], 0.0],
        };
        context
            .queue()
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    fn draw(
        &self,
        context: &GpuContext,
        target: &grimoire_gpu::OffscreenTarget,
        depth_view: &wgpu::TextureView,
        count: u32,
    ) -> Result<wgpu::SubmissionIndex, GpuError> {
        let pipeline = &self.pipeline;
        let bind_group = &self.bind_group;
        let instance_buffer = &self.instance_buffer;
        context.capture_errors(move |device| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wp3.2 billboard encoder"),
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("wp3.2 billboard pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target.view(),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.set_vertex_buffer(0, instance_buffer.slice(..));
                pass.draw(0..6, 0..count);
            }
            context.queue().submit([encoder.finish()])
        })
    }
}

/// Instanced low-poly-mesh scene: one icosphere (`procedural::icosphere(0, 1.0)`, the base
/// icosahedron, 12 vertices / 20 triangles), instanced with a per-bullet `MeshBulletInstance`.
struct MeshScene {
    pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    instance_buffer: wgpu::Buffer,
}

impl MeshScene {
    fn new(
        context: &GpuContext,
        color_format: wgpu::TextureFormat,
        capacity: usize,
    ) -> Result<Self, GpuError> {
        let device = context.device();
        let shader = context.capture_errors(|device| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("wp3.2 mesh shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("mesh.wgsl").into()),
            })
        })?;
        let bind_group_layout =
            create_camera_bind_group_layout(context, "wp3.2 mesh camera layout");
        let camera_buffer = create_uniform_buffer(
            context,
            std::mem::size_of::<MeshCameraUniform>() as u64,
            "wp3.2 mesh camera uniform",
        )?;
        let bind_group = create_camera_bind_group(
            context,
            &bind_group_layout,
            &camera_buffer,
            "wp3.2 mesh camera bind group",
        );
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wp3.2 mesh pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let vertex_attributes: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x3,
        ];
        let instance_attributes: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            2 => Float32x3,
            3 => Float32,
            4 => Float32,
            5 => Uint32,
            6 => Uint32,
        ];
        let instance_size = std::mem::size_of::<MeshBulletInstance>() as u64;
        let pipeline = context.capture_errors(|device| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("wp3.2 mesh pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<grimoire_render::MeshVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &vertex_attributes,
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: instance_size,
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
                    module: &shader,
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
        })?;

        let mesh_data = procedural::icosphere(0, 1.0);
        let vertex_bytes: &[u8] = bytemuck::cast_slice(mesh_data.vertices.as_slice());
        let index_bytes: &[u8] = bytemuck::cast_slice(mesh_data.indices.as_slice());
        let vertex_buffer = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wp3.2 mesh vertices"),
                size: vertex_bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;
        let index_buffer = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wp3.2 mesh indices"),
                size: index_bytes.len() as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;
        context
            .queue()
            .write_buffer(&vertex_buffer, 0, vertex_bytes);
        context.queue().write_buffer(&index_buffer, 0, index_bytes);

        let instance_buffer = create_instance_buffer(
            context,
            capacity as u64 * instance_size,
            "wp3.2 mesh instances",
        )?;

        Ok(Self {
            pipeline,
            camera_buffer,
            bind_group,
            vertex_buffer,
            index_buffer,
            index_count: u32::try_from(mesh_data.indices.len()).unwrap_or(u32::MAX),
            instance_buffer,
        })
    }

    fn upload_camera(&self, context: &GpuContext, camera: &TiltedCamera) {
        let uniform = MeshCameraUniform {
            view_proj: to_columns(camera.view_projection()),
        };
        context
            .queue()
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    fn draw(
        &self,
        context: &GpuContext,
        target: &grimoire_gpu::OffscreenTarget,
        depth_view: &wgpu::TextureView,
        count: u32,
    ) -> Result<wgpu::SubmissionIndex, GpuError> {
        let pipeline = &self.pipeline;
        let bind_group = &self.bind_group;
        let vertex_buffer = &self.vertex_buffer;
        let index_buffer = &self.index_buffer;
        let index_count = self.index_count;
        let instance_buffer = &self.instance_buffer;
        context.capture_errors(move |device| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wp3.2 mesh encoder"),
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("wp3.2 mesh pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target.view(),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.set_vertex_buffer(1, instance_buffer.slice(..));
                pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..index_count, 0, 0..count);
            }
            context.queue().submit([encoder.finish()])
        })
    }
}

fn to_columns(matrix: [f32; 16]) -> [[f32; 4]; 4] {
    [
        [matrix[0], matrix[1], matrix[2], matrix[3]],
        [matrix[4], matrix[5], matrix[6], matrix[7]],
        [matrix[8], matrix[9], matrix[10], matrix[11]],
        [matrix[12], matrix[13], matrix[14], matrix[15]],
    ]
}

fn poll_wait(context: &GpuContext, submission: wgpu::SubmissionIndex) -> Result<(), GpuError> {
    context
        .device()
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .map_err(|error| GpuError::Validation(error.to_string()))?;
    Ok(())
}

struct PhaseResult {
    extract: Summary,
    upload: Summary,
    gpu_relative: Summary,
}

fn measure_billboard(
    context: &GpuContext,
    scene: &BillboardScene,
    target: &grimoire_gpu::OffscreenTarget,
    depth_view: &wgpu::TextureView,
    camera: &TiltedCamera,
    source: &[SourceBullet],
) -> Result<PhaseResult, GpuError> {
    scene.upload_camera(context, camera);
    let mut instances = Vec::with_capacity(source.len());
    let count = u32::try_from(source.len()).expect("instance counts stay far below u32::MAX");
    let mut extract_us = Vec::with_capacity(REPS);
    let mut upload_us = Vec::with_capacity(REPS);
    let mut gpu_us = Vec::with_capacity(REPS);
    for rep in 0..(WARMUP + REPS) {
        let t0 = Instant::now();
        extract_billboard(source, &mut instances);
        let extract_elapsed = t0.elapsed();

        let t1 = Instant::now();
        context.queue().write_buffer(
            &scene.instance_buffer,
            0,
            bytemuck::cast_slice(instances.as_slice()),
        );
        let upload_elapsed = t1.elapsed();

        let t2 = Instant::now();
        let submission = scene.draw(context, target, depth_view, count)?;
        poll_wait(context, submission)?;
        let gpu_elapsed = t2.elapsed();

        if rep >= WARMUP {
            extract_us.push(extract_elapsed.as_secs_f64() * 1e6);
            upload_us.push(upload_elapsed.as_secs_f64() * 1e6);
            gpu_us.push(gpu_elapsed.as_secs_f64() * 1e6);
        }
    }
    Ok(PhaseResult {
        extract: Summary::from_micros(extract_us),
        upload: Summary::from_micros(upload_us),
        gpu_relative: Summary::from_micros(gpu_us),
    })
}

fn measure_mesh(
    context: &GpuContext,
    scene: &MeshScene,
    target: &grimoire_gpu::OffscreenTarget,
    depth_view: &wgpu::TextureView,
    camera: &TiltedCamera,
    source: &[SourceBullet],
) -> Result<PhaseResult, GpuError> {
    scene.upload_camera(context, camera);
    let mut instances = Vec::with_capacity(source.len());
    let count = u32::try_from(source.len()).expect("instance counts stay far below u32::MAX");
    let mut extract_us = Vec::with_capacity(REPS);
    let mut upload_us = Vec::with_capacity(REPS);
    let mut gpu_us = Vec::with_capacity(REPS);
    for rep in 0..(WARMUP + REPS) {
        let t0 = Instant::now();
        extract_mesh(source, &mut instances);
        let extract_elapsed = t0.elapsed();

        let t1 = Instant::now();
        context.queue().write_buffer(
            &scene.instance_buffer,
            0,
            bytemuck::cast_slice(instances.as_slice()),
        );
        let upload_elapsed = t1.elapsed();

        let t2 = Instant::now();
        let submission = scene.draw(context, target, depth_view, count)?;
        poll_wait(context, submission)?;
        let gpu_elapsed = t2.elapsed();

        if rep >= WARMUP {
            extract_us.push(extract_elapsed.as_secs_f64() * 1e6);
            upload_us.push(upload_elapsed.as_secs_f64() * 1e6);
            gpu_us.push(gpu_elapsed.as_secs_f64() * 1e6);
        }
    }
    Ok(PhaseResult {
        extract: Summary::from_micros(extract_us),
        upload: Summary::from_micros(upload_us),
        gpu_relative: Summary::from_micros(gpu_us),
    })
}

fn print_row(variant: &str, count: usize, result: &PhaseResult) {
    println!(
        "wp32-bullet: variant={variant} instances={count} phase=extraction median_us={:.2} min_us={:.2} max_us={:.2} samples={}",
        result.extract.median_us,
        result.extract.min_us,
        result.extract.max_us,
        result.extract.samples
    );
    println!(
        "wp32-bullet: variant={variant} instances={count} phase=upload median_us={:.2} min_us={:.2} max_us={:.2} samples={}",
        result.upload.median_us, result.upload.min_us, result.upload.max_us, result.upload.samples
    );
    println!(
        "wp32-bullet: variant={variant} instances={count} phase=gpu_relative median_us={:.2} min_us={:.2} max_us={:.2} samples={}",
        result.gpu_relative.median_us,
        result.gpu_relative.min_us,
        result.gpu_relative.max_us,
        result.gpu_relative.samples
    );
    println!(
        "| {variant} | {count} | {:.2} | {:.2}-{:.2} | {:.2} | {:.2}-{:.2} | {:.2} | {:.2}-{:.2} |",
        result.extract.median_us,
        result.extract.min_us,
        result.extract.max_us,
        result.upload.median_us,
        result.upload.min_us,
        result.upload.max_us,
        result.gpu_relative.median_us,
        result.gpu_relative.min_us,
        result.gpu_relative.max_us,
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = elevated_context()?;
    println!("{}", context.adapter_report_line());
    println!(
        "## WP3.2 Bullet-Darstellung: Billboard-Impostor vs. instanzierte Low-Poly-Meshes (Runner-Werte, Median aus {REPS} Wiederholungen nach {WARMUP} Aufwaermlaeufen, Software-Adapter)"
    );
    println!();
    println!(
        "| Variante | Instanzen | Extraktion Median (us) | Extraktion Spanne (us) | Upload Median (us) | Upload Spanne (us) | GPU relativ Median (us) | GPU relativ Spanne (us) |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");

    let target = grimoire_gpu::OffscreenTarget::new(&context, TARGET_WIDTH, TARGET_HEIGHT)?;
    let depth_view = create_depth_view(&context)?;
    let camera = tilted_camera();
    let color_format = target.format();

    let max_capacity = *INSTANCE_COUNTS.iter().max().expect("non-empty");
    let billboard_scene = BillboardScene::new(&context, color_format, max_capacity)?;
    let mesh_scene = MeshScene::new(&context, color_format, max_capacity)?;

    for &count in &INSTANCE_COUNTS {
        let source = synthetic_bullets(count, 0xB0110E5);
        let billboard = measure_billboard(
            &context,
            &billboard_scene,
            &target,
            &depth_view,
            &camera,
            &source,
        )?;
        print_row("billboard", count, &billboard);
        let mesh = measure_mesh(
            &context,
            &mesh_scene,
            &target,
            &depth_view,
            &camera,
            &source,
        )?;
        print_row("mesh", count, &mesh);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BillboardScene, MeshScene, TARGET_HEIGHT, TARGET_WIDTH, create_depth_view,
        elevated_context, measure_billboard, measure_mesh, synthetic_bullets, tilted_camera,
    };

    /// Correctness-only smoke test, not a measurement: confirms both pipelines actually draw a
    /// tiny batch without a `GpuError` before this spike's CI workflow ever spends a full
    /// 10k/20k-instance CI run on it. Runs locally with `GRIMOIRE_GPU_ADAPTER=software` like every
    /// other GPU test in this workspace (crate-vertraege.md §6); skips (does not fail) without an
    /// adapter at all.
    fn tiny_source() -> Vec<light_bullet_stress::bullets::SourceBullet> {
        synthetic_bullets(4, 42)
    }

    #[test]
    fn billboard_scene_draws_a_tiny_batch_without_error() {
        let Ok(context) = elevated_context() else {
            eprintln!(
                "\n::warning title=WP3.2 bullet stress smoke test skipped::no GPU adapter available"
            );
            return;
        };
        let target = grimoire_gpu::OffscreenTarget::new(&context, TARGET_WIDTH, TARGET_HEIGHT)
            .expect("offscreen target");
        let depth_view = create_depth_view(&context).expect("depth view");
        let camera = tilted_camera();
        let scene = BillboardScene::new(&context, target.format(), 4).expect("billboard scene");
        let source = tiny_source();
        let result = measure_billboard(&context, &scene, &target, &depth_view, &camera, &source);
        assert!(result.is_ok(), "{:?}", result.err());
    }

    #[test]
    fn mesh_scene_draws_a_tiny_batch_without_error() {
        let Ok(context) = elevated_context() else {
            eprintln!(
                "\n::warning title=WP3.2 bullet stress smoke test skipped::no GPU adapter available"
            );
            return;
        };
        let target = grimoire_gpu::OffscreenTarget::new(&context, TARGET_WIDTH, TARGET_HEIGHT)
            .expect("offscreen target");
        let depth_view = create_depth_view(&context).expect("depth view");
        let camera = tilted_camera();
        let scene = MeshScene::new(&context, target.format(), 4).expect("mesh scene");
        let source = tiny_source();
        let result = measure_mesh(&context, &scene, &target, &depth_view, &camera, &source);
        assert!(result.is_ok(), "{:?}", result.err());
    }
}
