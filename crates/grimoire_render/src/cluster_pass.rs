//! Clustered forward+ light assignment (plan 0002 WP3.4, engine ADR-0015 "compute clustering"):
//! wires `cluster_layout`'s three-buffer shape (WP3.1, engine ADR-0013) into a real GPU compute
//! pass instead of a CPU froxel loop. Not part of the crate's public API (engine ADR-0002: `wgpu`
//! stays invisible outside this crate and `grimoire_gpu`).
//!
//! # Why compute, not CPU
//!
//! Engine ADR-0015 measured CPU-side froxel assignment at 256 lights overrunning the WP3.3
//! combined render-CPU budget by 15-18% on the CI runner, while the compute path stayed under 5%
//! of the same budget (the CPU only uploads the small light list; the cluster table and index list
//! are never round-tripped through the CPU). [`ClusterPass::dispatch`] follows that shape: it
//! uploads lights and a small params uniform, encodes and submits the compute pass, and returns
//! immediately — no blocking wait on the dispatch itself. The GPU enforces the write (this pass) /
//! read (`mesh.wgsl`'s fragment shader, group 4) ordering itself because both are submitted to the
//! same queue in program order; nothing here ever calls `device.poll(Wait)` to force that
//! dependency, so the near-zero CPU cost ADR-0015 measured is what a real frame actually pays.
//!
//! # Where the frame-visible counters come from
//!
//! [`crate::StageStats`]'s new `clusters_with_lights`/`light_cluster_index_entries` counters (WP3.4)
//! need CPU-visible data the compute shader only produces on the GPU. Reading it back with a
//! blocking wait would reintroduce exactly the cost ADR-0015 rejected (its "Einschränkung" notes
//! the measured compute time is a *blocking* wall-clock number, pessimistic versus a pipelined
//! renderer) — so [`ClusterPass`] instead copies the (small, budget-independent, 27,648-byte)
//! cluster table into a persistent staging buffer every frame and maps it with `map_async`,
//! polling non-blockingly (`PollType::Poll`) on the *next* call to see whether it resolved. The
//! counters [`ClusterPass::dispatch`] returns therefore lag the actual GPU state by roughly one
//! frame under normal load (more under GPU contention, since a still-pending read is not restarted
//! until it resolves) — a known, documented limitation, not a bug; nothing in the render contract
//! promises same-frame freshness for a diagnostic counter, and every render test scene's exact
//! pixel output is unaffected (they never read these counters).

use std::sync::mpsc::{Receiver, TryRecvError};

use grimoire_gpu::{GpuContext, GpuError, wgpu};

use crate::cluster_layout::{self, CLUSTER_COUNT, GpuClusterLightRange, GpuPointLight};
use crate::gpu_timer::PassTimestamps;

const WORKGROUP_SIZE: u32 = 64;

/// Camera basis and projection scalars [`ClusterPass::dispatch`] needs to bucket a light (or,
/// mirrored in `mesh.wgsl`, a shaded fragment) into a froxel, without rebuilding a full 4x4
/// view-projection matrix. Built from the same orthonormal basis
/// [`crate::stage3d::view_projection`] uses (`stage3d::cluster_camera_basis`), so the compute
/// assignment and the fragment shader's lookup agree on every camera frame by construction, never
/// by keeping two derivations in sync by hand.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClusterCameraParams {
    /// World-space eye point.
    pub eye: [f32; 3],
    /// Camera-local "right" axis (unit length).
    pub right: [f32; 3],
    /// Camera-local "up" axis (unit length).
    pub up: [f32; 3],
    /// Camera-local "forward" axis (unit length; positive forward-distance direction).
    pub forward: [f32; 3],
    /// `1 / tan(fov_y / 2) / aspect`: scales a camera-local `x` offset at forward-distance `w` to
    /// NDC `x` (`ndc_x = right_offset * f_over_aspect / w`).
    pub f_over_aspect: f32,
    /// `1 / tan(fov_y / 2)`: same as [`ClusterCameraParams::f_over_aspect`] for the `y` axis.
    pub f: f32,
    /// Near clip plane (matches `mesh_pass::NEAR_PLANE`).
    pub near: f32,
    /// Far clip plane (matches `mesh_pass::FAR_PLANE`).
    pub far: f32,
}

impl ClusterCameraParams {
    /// A safe, finite fallback used when there is no usable camera this frame (no camera, or a
    /// non-finite one — `mesh_pass.rs` already treats that the same as "no camera" for drawing).
    /// Every field stays finite and `far > near > 0`, so the compute shader's `log`/division never
    /// sees a non-finite or zero input; which cluster a (nonexistent) light lands in does not
    /// matter when nothing is drawn.
    pub(crate) fn degenerate(near: f32, far: f32) -> Self {
        Self {
            eye: [0.0; 3],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            forward: [0.0, 1.0, 0.0],
            f_over_aspect: 1.0,
            f: 1.0,
            near,
            far,
        }
    }
}

mod gpu_types {
    // bytemuck's derive macros expand to `unsafe impl` blocks, like elsewhere in this crate.
    #![allow(unsafe_code)]

    /// Uniform the compute shader (`cluster.wgsl`) reads: [`super::ClusterCameraParams`] flattened
    /// to GPU-friendly vectors, plus the light count actually uploaded this frame and the
    /// configured light budget (buffer stride, matching [`crate::cluster_layout`]'s fixed-stride
    /// index list).
    #[repr(C)]
    #[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    pub(super) struct ClusterParamsGpu {
        pub eye: [f32; 4],
        pub right: [f32; 4],
        pub up: [f32; 4],
        pub forward: [f32; 4],
        /// `x` = `f_over_aspect`, `y` = `f`, `z` = `near`, `w` = `far`.
        pub proj: [f32; 4],
        pub light_count: u32,
        pub light_budget: u32,
        pub _pad0: u32,
        pub _pad1: u32,
    }
}
use gpu_types::ClusterParamsGpu;

fn params_gpu(
    camera: &ClusterCameraParams,
    light_count: u32,
    light_budget: u32,
) -> ClusterParamsGpu {
    ClusterParamsGpu {
        eye: [camera.eye[0], camera.eye[1], camera.eye[2], 0.0],
        right: [camera.right[0], camera.right[1], camera.right[2], 0.0],
        up: [camera.up[0], camera.up[1], camera.up[2], 0.0],
        forward: [camera.forward[0], camera.forward[1], camera.forward[2], 0.0],
        proj: [camera.f_over_aspect, camera.f, camera.near, camera.far],
        light_count,
        light_budget,
        _pad0: 0,
        _pad1: 0,
    }
}

/// Frame-visible measurements of one [`ClusterPass::dispatch`] call, surfaced on
/// [`crate::StageStats`] (WP3.4). See this module's doc comment for why these lag the actual GPU
/// state by roughly a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ClusterFrameStats {
    /// Of [`cluster_layout::CLUSTER_COUNT`] froxels, how many had at least one light assigned, as
    /// of the most recently completed readback (see this module's doc comment).
    pub clusters_with_lights: u32,
    /// Total light-cluster assignments across every froxel (sum of `count` over the cluster
    /// table), as of the most recently completed readback. Always
    /// `<= cluster_layout::light_index_list_worst_case_len(light_budget)`, usually far below it —
    /// real scenes overlap much less than the worst case that sizes the index list buffer (engine
    /// ADR-0013's module doc comment).
    pub light_cluster_index_entries: u32,
}

/// Non-blocking readback state of the (small, budget-independent) cluster-table staging buffer —
/// see this module's doc comment for why this is asynchronous rather than a `device.poll(Wait)`.
enum StatsReadback {
    /// No copy is in flight; the next [`ClusterPass::dispatch`] starts one.
    Idle,
    /// A copy was started; `Receiver` resolves once `map_async`'s callback runs.
    Pending(Receiver<Result<(), wgpu::BufferAsyncError>>),
}

/// Owns the compute pipeline and the three [`cluster_layout`] storage buffers (sized once, for a
/// fixed light budget, at construction — WP3.4 does not support changing the budget of a live
/// renderer, matching how `RendererConfig`'s other construction parameters work today), plus the
/// read-only bind group `mesh_pass.rs`'s fragment pipelines bind at group 4 to consume the result.
pub(crate) struct ClusterPass {
    light_budget: u32,
    pipeline: wgpu::ComputePipeline,
    compute_bind_group: wgpu::BindGroup,
    /// Group 4 of `mesh.wgsl`: read-only view of the same three buffers the compute pass writes.
    fragment_bind_group_layout: wgpu::BindGroupLayout,
    fragment_bind_group: wgpu::BindGroup,
    light_buffer: wgpu::Buffer,
    cluster_table_buffer: wgpu::Buffer,
    /// Bound into `compute_bind_group` at construction; otherwise never read from Rust in
    /// production (the fragment shader reads it itself via `fragment_bind_group`) — kept as a
    /// field only so `#[cfg(test)]` correctness tests can read it back directly.
    #[allow(
        dead_code,
        reason = "keeps the GPU buffer alive and lets #[cfg(test)] tests read it back; never read directly outside tests"
    )]
    index_list_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    stats_staging: wgpu::Buffer,
    stats_readback: StatsReadback,
    stats_last: ClusterFrameStats,
}

impl ClusterPass {
    /// # Errors
    /// Whatever buffer/pipeline creation on `context` returns (for example out of memory).
    pub(crate) fn new(context: &GpuContext, light_budget: usize) -> Result<Self, GpuError> {
        let device = context.device();
        let shader = context.capture_errors(|device| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("grimoire cluster compute shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("cluster.wgsl").into()),
            })
        })?;

        let compute_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("grimoire cluster compute layout"),
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
            label: Some("grimoire cluster compute pipeline layout"),
            bind_group_layouts: &[Some(&compute_bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = context.capture_errors(|device| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("grimoire cluster compute pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("cs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        })?;

        let light_buffer = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("grimoire cluster lights"),
                size: cluster_layout::light_list_bytes(light_budget) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;
        let cluster_table_buffer = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("grimoire cluster table"),
                size: cluster_layout::cluster_table_bytes() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        })?;
        let index_list_buffer = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("grimoire cluster index list"),
                size: cluster_layout::light_index_list_worst_case_bytes(light_budget) as u64,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        })?;
        let params_buffer = context.capture_errors(|device| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("grimoire cluster params"),
                size: std::mem::size_of::<ClusterParamsGpu>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;

        let compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grimoire cluster compute bind group"),
            layout: &compute_bind_group_layout,
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

        // Group 4 of `mesh.wgsl`: the same three buffers the compute pass above owns, bound
        // read-only for the fragment stage. A separate layout/bind group (rather than reusing
        // `compute_bind_group_layout`) because the visibility flags and the missing params entry
        // differ; the underlying buffers are identical.
        let fragment_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("grimoire cluster fragment layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let fragment_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grimoire cluster fragment bind group"),
            layout: &fragment_bind_group_layout,
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
            ],
        });

        let stats_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire cluster stats staging"),
            size: cluster_layout::cluster_table_bytes() as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            light_budget: u32::try_from(light_budget).unwrap_or(u32::MAX),
            pipeline,
            compute_bind_group,
            fragment_bind_group_layout,
            fragment_bind_group,
            light_buffer,
            cluster_table_buffer,
            index_list_buffer,
            params_buffer,
            stats_staging,
            stats_readback: StatsReadback::Idle,
            stats_last: ClusterFrameStats::default(),
        })
    }

    /// Group 4's layout, for `mesh_pass.rs`'s rigid and skinned pipeline layouts (both share
    /// `mesh.wgsl`'s `fs_main`, so both need it at the same group index).
    pub(crate) fn fragment_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.fragment_bind_group_layout
    }

    /// Group 4's bind group, bound identically regardless of which mesh pipeline (rigid or
    /// skinned) is drawing — both read the same cluster assignment for the frame.
    pub(crate) fn fragment_bind_group(&self) -> &wgpu::BindGroup {
        &self.fragment_bind_group
    }

    /// Uploads `lights` (must be `<= light_budget` from [`ClusterPass::new`], already clamped and
    /// bullet-cap-adjusted by the caller — `mesh_pass::build_light_list`) and `camera`'s basis,
    /// then dispatches one compute invocation per cluster
    /// (`ceil(cluster_layout::CLUSTER_COUNT / 64)` workgroups). Encodes and submits its own command
    /// buffer and returns immediately: no blocking wait on the dispatch (see this module's doc
    /// comment on why that stays true even though this pass writes buffers `mesh.wgsl`'s fragment
    /// shader reads later the same frame — ordinary queue submission order already guarantees
    /// that).
    ///
    /// Also advances the non-blocking cluster-table stats readback (see this module's doc comment)
    /// and returns whatever [`ClusterFrameStats`] it currently knows (roughly one frame behind
    /// under normal load).
    ///
    /// # Errors
    /// Whatever `GpuContext::capture_errors` reports for the encode/submit.
    pub(crate) fn dispatch(
        &mut self,
        context: &GpuContext,
        lights: &[GpuPointLight],
        camera: &ClusterCameraParams,
        timestamps: Option<PassTimestamps>,
    ) -> Result<ClusterFrameStats, GpuError> {
        debug_assert!(
            lights.len() <= self.light_budget as usize,
            "caller must clamp to the configured light budget before calling dispatch"
        );
        context
            .queue()
            .write_buffer(&self.light_buffer, 0, bytemuck::cast_slice(lights));
        let params = params_gpu(
            camera,
            u32::try_from(lights.len()).unwrap_or(self.light_budget),
            self.light_budget,
        );
        context
            .queue()
            .write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&params));

        // Pump any previous readback *before* deciding whether to start a new one this frame: a
        // buffer cannot be copied into while a `map_async` on it is still outstanding, so a
        // still-pending readback means this frame skips refreshing the stats mirror entirely
        // (`dispatch` still runs the real compute assignment every frame regardless — only the
        // diagnostic counters go stale a little longer under contention, see this module's doc
        // comment).
        self.pump_stats(context);
        let want_stats_copy = matches!(self.stats_readback, StatsReadback::Idle);

        let workgroups = (CLUSTER_COUNT as u32).div_ceil(WORKGROUP_SIZE);
        let pipeline = &self.pipeline;
        let compute_bind_group = &self.compute_bind_group;
        let cluster_table_buffer = &self.cluster_table_buffer;
        let stats_staging = &self.stats_staging;
        let cluster_table_bytes = cluster_layout::cluster_table_bytes() as u64;
        let _submission = context.capture_errors(move |device| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("grimoire cluster compute encoder"),
            });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("grimoire cluster compute pass"),
                    timestamp_writes: timestamps.as_ref().map(PassTimestamps::compute),
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, compute_bind_group, &[]);
                pass.dispatch_workgroups(workgroups, 1, 1);
            }
            if want_stats_copy {
                // Feeds only the non-blocking stats readback below, never the fragment shader's
                // own read of `cluster_table_buffer` (that one needs no copy at all — the mesh
                // pass binds the buffer itself at group 4, see this module's doc comment on
                // submission ordering).
                encoder.copy_buffer_to_buffer(
                    cluster_table_buffer,
                    0,
                    stats_staging,
                    0,
                    cluster_table_bytes,
                );
            }
            context.queue().submit([encoder.finish()])
        })?;

        if want_stats_copy {
            self.start_stats_readback();
        }
        Ok(self.stats_last)
    }

    /// Non-blocking poll of a previously started stats readback (see this module's doc comment);
    /// updates [`ClusterPass::stats_last`] if it has resolved since the last call.
    fn pump_stats(&mut self, context: &GpuContext) {
        let StatsReadback::Pending(receiver) = &self.stats_readback else {
            return;
        };
        // Never blocks: `PollType::Poll` only checks for already-completed work.
        let _ = context.device().poll(wgpu::PollType::Poll);
        // Read the outcome before matching on it so the borrow of `self.stats_readback` behind
        // `receiver` ends here — the arms below need to assign back into `self.stats_readback`.
        let outcome = receiver.try_recv();
        match outcome {
            Ok(Ok(())) => {
                if let Ok(view) = self.stats_staging.get_mapped_range(..) {
                    let entries: &[GpuClusterLightRange] = bytemuck::cast_slice(&view);
                    let mut clusters_with_lights = 0u32;
                    let mut light_cluster_index_entries = 0u32;
                    for entry in entries {
                        if entry.count > 0 {
                            clusters_with_lights += 1;
                        }
                        light_cluster_index_entries += entry.count;
                    }
                    drop(view);
                    self.stats_last = ClusterFrameStats {
                        clusters_with_lights,
                        light_cluster_index_entries,
                    };
                }
                self.stats_staging.unmap();
                self.stats_readback = StatsReadback::Idle;
            }
            Ok(Err(_)) => {
                // Mapping failed (for example a lost device); drop this attempt and keep
                // reporting the last known-good stats rather than propagating an error for a
                // diagnostic counter.
                self.stats_readback = StatsReadback::Idle;
            }
            Err(TryRecvError::Empty) => {
                // Still in flight; try again next frame. `stats_last` (returned by `dispatch`)
                // keeps reporting the previous value.
            }
            Err(TryRecvError::Disconnected) => {
                self.stats_readback = StatsReadback::Idle;
            }
        }
    }

    /// Starts a new non-blocking `map_async` on `stats_staging`, which [`ClusterPass::dispatch`]
    /// just refreshed via `copy_buffer_to_buffer`. Only called when no readback is already pending
    /// (`ClusterPass::dispatch`'s caller checks `StatsReadback::Idle`), since a buffer cannot be
    /// mapped again while a previous map is still outstanding.
    fn start_stats_readback(&mut self) {
        let (sender, receiver) = std::sync::mpsc::channel();
        self.stats_staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.stats_readback = StatsReadback::Pending(receiver);
    }

    /// Test-only blocking readback of the cluster table and index list — unlike
    /// [`ClusterPass::dispatch`]'s own non-blocking stats mirror (this module's doc comment
    /// explains why production code never does this), a test needs a deterministic, immediate
    /// result far more than it needs to avoid a `device.poll(Wait)` a test only pays once.
    #[cfg(test)]
    fn read_back_for_test(&self, context: &GpuContext) -> (Vec<GpuClusterLightRange>, Vec<u32>) {
        let cluster_table = Self::read_buffer_for_test(
            context,
            &self.cluster_table_buffer,
            cluster_layout::cluster_table_bytes() as u64,
        );
        let index_list = Self::read_buffer_for_test(
            context,
            &self.index_list_buffer,
            cluster_layout::light_index_list_worst_case_bytes(self.light_budget as usize) as u64,
        );
        (
            bytemuck::cast_slice(&cluster_table).to_vec(),
            bytemuck::cast_slice(&index_list).to_vec(),
        )
    }

    #[cfg(test)]
    fn read_buffer_for_test(context: &GpuContext, buffer: &wgpu::Buffer, size: u64) -> Vec<u8> {
        let device = context.device();
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire cluster test read-back"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("grimoire cluster test read-back encoder"),
        });
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
        context.queue().submit([encoder.finish()]);

        let (sender, receiver) = std::sync::mpsc::channel();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        // `submission_index: None` waits for the most recent submission at the time of this poll
        // (this function's own copy, just submitted above) rather than a specific stashed index —
        // simpler and sufficient for a test that submits nothing concurrently.
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll succeeds");
        receiver
            .recv()
            .expect("map_async callback fires")
            .expect("mapping succeeds");
        let data = staging
            .get_mapped_range(..)
            .expect("mapped range is available")
            .to_vec();
        staging.unmap();
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grimoire_gpu::ContextOptions;

    /// Mirrors the crate's other GPU tests (for example `cluster_layout.rs`'s worst-case test and
    /// `tests/offscreen.rs`'s `try_offscreen_renderer`): skips, rather than fails, when no adapter
    /// is available, with the same greppable `::warning::` annotation convention.
    fn test_context() -> Option<GpuContext> {
        match GpuContext::new_offscreen(ContextOptions::default()) {
            Ok(context) => Some(context),
            Err(error) => {
                eprintln!(
                    "\n::warning title=WP3.4 cluster pass test skipped::no GPU adapter available ({error})"
                );
                None
            }
        }
    }

    fn stand_in_camera() -> ClusterCameraParams {
        // Looking along +Y, right = +X, up = +Z (the same convention `stage3d::camera_basis`
        // uses), a 90-degree vertical FOV (`f = 1 / tan(45deg) = 1`), a 16:9-ish aspect, and a
        // near/far range wide enough for 24 exponential depth slices to matter.
        ClusterCameraParams {
            eye: [0.0, 0.0, 0.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            forward: [0.0, 1.0, 0.0],
            f_over_aspect: 0.5625,
            f: 1.0,
            near: 1.0,
            far: 100.0,
        }
    }

    fn light(position: [f32; 3], range: f32) -> GpuPointLight {
        GpuPointLight {
            position,
            range,
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
        }
    }

    #[test]
    fn dispatch_assigns_a_near_center_light_and_excludes_an_out_of_frustum_light() {
        let Some(context) = test_context() else {
            return;
        };
        let mut pass = ClusterPass::new(&context, cluster_layout::LIGHT_BUDGET_LOW)
            .expect("pass creation on a software adapter succeeds");
        let camera = stand_in_camera();
        // Light 0: just past the near plane, close to the view centre — must land in some
        // cluster. Light 1: behind the camera (negative forward distance) — the sphere-vs-AABB
        // test's `clamp(lw, w_near, w_far)` still finds a *nearest* point for any real number, so
        // this is not proven unreachable by construction; it is far enough behind (50 units, a
        // tiny 0.5-unit range) that it cannot be within `range` of any cluster's near slab
        // (`w_near` for slice 0 equals `near = 1.0`), which is what this test actually checks.
        let lights = [light([0.05, 1.5, 0.05], 0.5), light([0.0, -50.0, 0.0], 0.5)];
        pass.dispatch(&context, &lights, &camera, None)
            .expect("dispatch succeeds");
        // `read_back_for_test` submits its own copy (necessarily ordered after the compute
        // dispatch above, same queue) and waits on *that* submission specifically — correct and
        // sufficient without a separate, broader wait here first.
        let (table, index_list) = pass.read_back_for_test(&context);

        let total_count: u32 = table.iter().map(|range| range.count).sum();
        assert!(
            total_count >= 1,
            "the near-centre light must be assigned to at least one cluster"
        );
        for range in &table {
            for &index in &index_list[range.offset as usize..][..range.count as usize] {
                assert_eq!(
                    index, 0,
                    "the far-behind-camera light must never be recorded"
                );
            }
        }
    }

    #[test]
    fn dispatch_processes_the_full_high_budget_of_256_lights_without_error() {
        let Some(context) = test_context() else {
            return;
        };
        let mut pass = ClusterPass::new(&context, cluster_layout::LIGHT_BUDGET_HIGH)
            .expect("pass creation on a software adapter succeeds");
        let camera = stand_in_camera();
        // 256 lights spread out along the view axis and to both sides, all within range of at
        // least the fragment directly in front of them.
        let lights: Vec<GpuPointLight> = (0..cluster_layout::LIGHT_BUDGET_HIGH)
            .map(|i| {
                let t = i as f32;
                light([(t % 16.0) - 8.0, 2.0 + t * 0.3, (t % 9.0) - 4.0], 3.0)
            })
            .collect();
        pass.dispatch(&context, &lights, &camera, None)
            .expect("dispatch of 256 lights succeeds");
        let (table, index_list) = pass.read_back_for_test(&context);

        let total_count: u32 = table.iter().map(|range| range.count).sum();
        assert!(
            total_count > 0,
            "at least one of 256 spread-out lights must land in at least one cluster"
        );
        for range in &table {
            assert!(
                range.count as usize <= cluster_layout::LIGHT_BUDGET_HIGH,
                "a cluster's count must never exceed the configured light budget"
            );
            for &index in &index_list[range.offset as usize..][..range.count as usize] {
                assert!(
                    (index as usize) < lights.len(),
                    "every recorded index must point at a real uploaded light"
                );
            }
        }
    }
}
