//! GPU compute-clustering path (`light_cluster.wgsl`): the light-culling spike's second variant,
//! running the identical `O(clusters x lights)` assignment algorithm as
//! [`crate::lights::cpu_assign_clusters`] on the GPU instead of the CPU. Lives in the library (not
//! `src/bin/light_cluster.rs`) so its correctness can be checked by a `cargo test` at a tiny,
//! fast grid size, independent of the stress-scale numbers the binary measures.

use grimoire_gpu::{GpuContext, GpuError, wgpu};
use grimoire_render::cluster_layout::{GpuClusterLightRange, GpuPointLight};

#[allow(unsafe_code)] // bytemuck derive, same rationale as elsewhere in this spike
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    light_count: u32,
    light_budget: u32,
    grid_x: u32,
    grid_y: u32,
    grid_z: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

/// Grid dimensions the compute pass clusters into. Mirrors `grimoire_render::cluster_layout`'s
/// `CLUSTER_GRID_{X,Y,Z}` for the stress binary's own run; a correctness test uses a much smaller
/// grid instead so it stays fast under plain `cargo test`.
#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

impl Grid {
    #[must_use]
    pub const fn cluster_count(&self) -> usize {
        (self.x * self.y * self.z) as usize
    }
}

const WORKGROUP_SIZE: u32 = 64;

/// Owns the GPU-side buffers and pipeline for one compute-clustering run: a read-only light
/// buffer (sized for `max_lights`), and the same fixed-stride cluster-table/index-list buffers
/// [`crate::lights::cpu_assign_clusters`] fills on the CPU (sized for `grid` and `light_budget`).
pub struct ComputeClusterPass {
    grid: Grid,
    light_budget: u32,
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    light_buffer: wgpu::Buffer,
    cluster_table_buffer: wgpu::Buffer,
    index_list_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
}

impl ComputeClusterPass {
    /// # Errors
    /// Whatever buffer/pipeline creation on `context` returns.
    pub fn new(
        context: &GpuContext,
        grid: Grid,
        light_budget: u32,
        max_lights: u32,
    ) -> Result<Self, GpuError> {
        let device = context.device();
        let shader = context.capture_errors(|device| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("wp3.2 light cluster compute shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("light_cluster.wgsl").into()),
            })
        })?;

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wp3.2 light cluster bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wp3.2 light cluster pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = context.capture_errors(|device| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("wp3.2 light cluster pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("cs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        })?;

        let light_buffer = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wp3.2 light cluster lights"),
                size: u64::from(max_lights) * size_of::<GpuPointLight>() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;
        let cluster_count = grid.cluster_count() as u64;
        let cluster_table_buffer = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wp3.2 light cluster table"),
                size: cluster_count * size_of::<GpuClusterLightRange>() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        })?;
        let index_list_buffer = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wp3.2 light cluster index list"),
                size: cluster_count * u64::from(light_budget) * size_of::<u32>() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        })?;
        let params_buffer = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("wp3.2 light cluster params"),
                size: size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wp3.2 light cluster bind group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: cluster_table_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: index_list_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        Ok(Self {
            grid,
            light_budget,
            pipeline,
            bind_group,
            light_buffer,
            cluster_table_buffer,
            index_list_buffer,
            params_buffer,
        })
    }

    /// Uploads `lights` (must fit within `max_lights` from [`ComputeClusterPass::new`]) and the
    /// (constant, but cheap to rewrite) params uniform. This is the compute path's entire
    /// "upload" cost: unlike the CPU path, the cluster table and index list are never written
    /// from the CPU at all.
    pub fn upload_lights(&self, context: &GpuContext, lights: &[GpuPointLight]) {
        context
            .queue()
            .write_buffer(&self.light_buffer, 0, bytemuck::cast_slice(lights));
        let params = Params {
            light_count: u32::try_from(lights.len()).expect("light count fits u32"),
            light_budget: self.light_budget,
            grid_x: self.grid.x,
            grid_y: self.grid.y,
            grid_z: self.grid.z,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        };
        context
            .queue()
            .write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));
    }

    /// Dispatches one invocation per cluster (`ceil(cluster_count / 64)` workgroups) and submits.
    ///
    /// # Errors
    /// Whatever `GpuContext::capture_errors` reports for the encode/submit.
    pub fn dispatch(&self, context: &GpuContext) -> Result<wgpu::SubmissionIndex, GpuError> {
        let workgroups = (self.grid.cluster_count() as u32).div_ceil(WORKGROUP_SIZE);
        let pipeline = &self.pipeline;
        let bind_group = &self.bind_group;
        context.capture_errors(move |device| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wp3.2 light cluster encoder"),
            });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("wp3.2 light cluster pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.dispatch_workgroups(workgroups, 1, 1);
            }
            context.queue().submit([encoder.finish()])
        })
    }

    /// Reads the cluster table and index list back to the CPU (correctness tests only — the
    /// stress binary never pays this cost, matching a real clustered forward+ pass, which reads
    /// these buffers from the fragment shader, never back on the CPU).
    ///
    /// # Errors
    /// [`GpuError::Readback`] if mapping fails.
    pub fn read_back(
        &self,
        context: &GpuContext,
    ) -> Result<(Vec<GpuClusterLightRange>, Vec<u32>), GpuError> {
        let cluster_table = read_buffer(
            context,
            &self.cluster_table_buffer,
            self.grid.cluster_count() as u64 * size_of::<GpuClusterLightRange>() as u64,
        )?;
        let index_list = read_buffer(
            context,
            &self.index_list_buffer,
            self.grid.cluster_count() as u64
                * u64::from(self.light_budget)
                * size_of::<u32>() as u64,
        )?;
        Ok((
            bytemuck::cast_slice(&cluster_table).to_vec(),
            bytemuck::cast_slice(&index_list).to_vec(),
        ))
    }
}

fn read_buffer(
    context: &GpuContext,
    buffer: &wgpu::Buffer,
    size: u64,
) -> Result<Vec<u8>, GpuError> {
    let device = context.device();
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wp3.2 light cluster read-back"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("wp3.2 light cluster read-back encoder"),
    });
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    let submission = context.queue().submit([encoder.finish()]);

    let (sender, receiver) = std::sync::mpsc::channel();
    staging.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .map_err(|error| GpuError::Readback(error.to_string()))?;
    receiver
        .recv()
        .map_err(|error| GpuError::Readback(error.to_string()))?
        .map_err(|error| GpuError::Readback(error.to_string()))?;
    let data = staging
        .get_mapped_range(..)
        .map_err(|error| GpuError::Readback(error.to_string()))?
        .to_vec();
    staging.unmap();
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::{ComputeClusterPass, Grid};
    use crate::gpu::elevated_context;
    use crate::lights::cpu_assign_clusters;
    use grimoire_render::cluster_layout::GpuPointLight;

    /// Tiny grid (`2x2x2 = 8` clusters), 4 lights, budget 4 -- fast enough for plain `cargo test`
    /// with `GRIMOIRE_GPU_ADAPTER=software` (the crate-vertraege.md-sanctioned local test path),
    /// unlike the stress binary's full 16x9x24/256-light run. Skips (not fails) without an
    /// adapter, same convention as `grimoire_gpu`/`grimoire_render`'s own GPU tests.
    #[test]
    fn compute_path_matches_the_cpu_path_on_a_tiny_grid() {
        let Ok(context) = elevated_context() else {
            eprintln!(
                "\n::warning title=WP3.2 compute cluster test skipped::no GPU adapter available"
            );
            return;
        };

        let grid = Grid { x: 2, y: 2, z: 2 };
        let light_budget = 4u32;
        let lights = vec![
            GpuPointLight {
                position: [1.0, 1.0, 1.0],
                range: 2.5,
                color: [1.0, 0.0, 0.0],
                intensity: 1.0,
            },
            GpuPointLight {
                position: [0.0, 0.0, 0.0],
                range: 0.1,
                color: [0.0, 1.0, 0.0],
                intensity: 1.0,
            },
        ];

        let pass = ComputeClusterPass::new(&context, grid, light_budget, 16)
            .expect("pass creation on an elevated software adapter succeeds");
        pass.upload_lights(&context, &lights);
        let submission = pass.dispatch(&context).expect("dispatch succeeds");
        context
            .device()
            .poll(grimoire_gpu::wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .expect("poll succeeds");
        let (gpu_table, gpu_index_list) = pass.read_back(&context).expect("read-back succeeds");

        // Reference: the exact same algorithm, run on the CPU, against the same tiny grid --
        // `cpu_assign_clusters` itself is hard-coded to the real 16x9x24 grid, so this test
        // re-derives the expected per-cluster counts directly instead (still the same overlap
        // test as `light_cluster.wgsl`'s `cs_main` and `cpu_assign_clusters`).
        const FROXEL_HALF_DIAGONAL: f32 = 0.866_025_4;
        for z in 0..grid.z {
            for y in 0..grid.y {
                for x in 0..grid.x {
                    let cluster = ((z * grid.y + y) * grid.x + x) as usize;
                    let centre = [x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5];
                    let expected_count = lights
                        .iter()
                        .filter(|light| {
                            let d = [
                                light.position[0] - centre[0],
                                light.position[1] - centre[1],
                                light.position[2] - centre[2],
                            ];
                            let dist_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                            let reach = light.range + FROXEL_HALF_DIAGONAL;
                            dist_sq <= reach * reach
                        })
                        .count() as u32;
                    assert_eq!(
                        gpu_table[cluster].count, expected_count,
                        "cluster {cluster} ({x},{y},{z})"
                    );
                    assert_eq!(
                        gpu_table[cluster].offset as usize,
                        cluster * light_budget as usize
                    );
                }
            }
        }
        // Every recorded index must point at a real light.
        for (cluster, range) in gpu_table.iter().enumerate() {
            for slot in &gpu_index_list[range.offset as usize..][..range.count as usize] {
                assert!(
                    (*slot as usize) < lights.len(),
                    "cluster {cluster}: bad index {slot}"
                );
            }
        }

        // Cross-check against the library's own CPU path is only meaningful at the real grid size
        // (`cpu_assign_clusters` hard-codes CLUSTER_GRID_*); confirm that path at least runs and
        // produces a fully covered table, as a smoke test that the two paths use compatible types.
        let cpu_result = cpu_assign_clusters(&lights, light_budget as usize);
        assert_eq!(
            cpu_result.cluster_table.len(),
            grimoire_render::cluster_layout::CLUSTER_COUNT
        );
    }
}
