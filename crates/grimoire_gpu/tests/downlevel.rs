//! WP3.1 downlevel probe (plan 0002 WP3.1): measures the *real* capabilities of the selected
//! adapter instead of assuming them, so the light/cluster data layout plan 0002 WP3.4 needs
//! (clustered forward+, storage buffers in the fragment stage and in compute) rests on numbers,
//! not hope.
//!
//! Two independent things happen here:
//!
//! 1. **Capability report** ([`capability_report_prints_the_measured_limits`]): prints the
//!    adapter's actual features, downlevel flags and limits
//!    ([`GpuContext::capability_report_lines`]) — queried straight from the adapter, so it is
//!    accurate even for an adapter too limited to build the elevated context the two functional
//!    proofs below need.
//! 2. **Functional proof, not just a capability query** ([`fragment_stage_reads_a_storage_buffer...`],
//!    [`compute_pass_writes_a_storage_buffer...`]): two minimal pipelines that must actually run —
//!    a fragment shader reading a storage buffer and writing the result to a render target, and a
//!    compute pass writing a storage buffer whose content is read back and checked against fixed
//!    expected values. Both need a device requested with the adapter's own limits (not
//!    [`GpuContext::new_offscreen`]'s conservative WebGL2 baseline, which allows neither storage
//!    buffers nor compute at all — see the `grimoire-gpu-limits`/`grimoire-gpu-downlevel` lines
//!    this file prints for what the difference actually is on each runner).
//!
//! Same skip/report conventions as `grimoire_render/tests/offscreen.rs` (WP2.1): without any GPU
//! adapter at all, tests skip and are counted (`grimoire-gpu-downlevel-tests-skipped: <n>`) unless
//! `GRIMOIRE_REQUIRE_GPU_ADAPTER=1`, which turns the skip into a failure. Once an adapter exists, a
//! *missing capability* (no `COMPUTE_SHADERS`/`FRAGMENT_STORAGE` downlevel flag) is also a skip,
//! counted the same way and printed with its specific reason — never silently passed over. A
//! distinct line prefix (`grimoire-gpu-downlevel-*`, not `grimoire-gpu-adapter`/
//! `grimoire-gpu-tests-skipped`) keeps this binary's counters from being folded into WP2.1's,
//! which would otherwise undercount whichever of the two binaries skips less
//! (`.github/scripts/report-gpu-downlevel.sh` reads these lines on their own).

use std::io::Write;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use grimoire_gpu::{ContextOptions, GpuContext, GpuError, OffscreenTarget, wgpu};
use wgpu::util::DeviceExt;

/// Environment variable that turns a missing capability into a test failure instead of a skip.
const ENV_REQUIRE_GPU_ADAPTER: &str = "GRIMOIRE_REQUIRE_GPU_ADAPTER";

fn required_from_env() -> bool {
    matches!(
        std::env::var(ENV_REQUIRE_GPU_ADAPTER)
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true")
    )
}

/// Total tests in this binary that skipped for lack of an adapter or a required capability
/// (WP3.1). A running total, like `grimoire-gpu-tests-skipped` in `offscreen.rs`, so whichever
/// test happens to run last still reports the true count.
static SKIPPED_TESTS: AtomicUsize = AtomicUsize::new(0);

/// Fails the calling test if the adapter/capability is required, otherwise announces the skip
/// (with `reason`) as a GitHub Actions warning and counts it.
fn skip(reason: &str) {
    assert!(
        !required_from_env(),
        "skipped ({reason}) although {ENV_REQUIRE_GPU_ADAPTER}=1: the downlevel tests would pass \
         without proving anything"
    );
    // libtest captures print!/eprint! of passing tests, but not direct writes to the stream (same
    // rationale as offscreen.rs).
    let _ = writeln!(
        std::io::stdout(),
        "\n::warning title=WP3.1 downlevel test skipped::{reason}"
    );
    let total = SKIPPED_TESTS.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = writeln!(
        std::io::stdout(),
        "grimoire-gpu-downlevel-tests-skipped: {total}"
    );
}

static REPORTED: Once = Once::new();

/// Prints [`GpuContext::capability_report_lines`] once per test binary, the first time a test
/// actually obtains an adapter — regardless of whether that adapter can run the functional-proof
/// pipelines below.
fn report_capabilities_once(context: &GpuContext) {
    REPORTED.call_once(|| {
        for line in context.capability_report_lines() {
            let _ = writeln!(std::io::stdout(), "\n{line}");
        }
    });
}

/// Creates an offscreen context requesting the adapter's own limits (not
/// [`GpuContext::new_offscreen`]'s conservative baseline), so storage buffers and compute are
/// available whenever the adapter itself claims to support them. Reports capabilities once an
/// adapter is found; returns `None` (after skipping and counting) if there is none.
fn elevated_context() -> Option<GpuContext> {
    let options = ContextOptions {
        allow_software_fallback: true,
        ..ContextOptions::default()
    };
    match GpuContext::new_offscreen_with_limits(options, wgpu::Features::empty(), |limits| limits) {
        Ok(context) => {
            report_capabilities_once(&context);
            Some(context)
        }
        Err(GpuError::NoAdapter(_)) => {
            skip("no GPU adapter found");
            None
        }
        Err(error) => panic!("elevated offscreen context creation failed: {error}"),
    }
}

/// Copies `size` bytes out of `buffer` (usage must include `COPY_SRC`) to the CPU. Blocks until
/// the copy has finished, mirroring [`grimoire_gpu::OffscreenTarget::read_rgba`]'s approach.
fn read_buffer(context: &GpuContext, buffer: &wgpu::Buffer, size: u64) -> Vec<u8> {
    let device = context.device();
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wp3.1 downlevel read-back"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("wp3.1 downlevel read-back encoder"),
    });
    encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    let submission = context.queue().submit([encoder.finish()]);

    let (sender, receiver) = mpsc::channel();
    staging.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .expect("poll for the read-back copy");
    receiver
        .recv()
        .expect("map_async callback ran")
        .expect("buffer mapped for reading");
    let data = staging.get_mapped_range(..).expect("mapped range").to_vec();
    staging.unmap();
    data
}

#[test]
fn capability_report_prints_the_measured_limits() {
    // Reports even if the two functional-proof tests below skip: an adapter that cannot run them
    // still has real, measurable limits worth seeing in the job summary.
    let Some(_context) = elevated_context() else {
        return;
    };
}

// --- Fragment stage reads a storage buffer (plan 0002 WP3.1, item 2) --------------------------

/// One RGBA colour per output pixel, read-only: exactly the access pattern the clustered
/// forward+ pass (plan 0002 WP3.4) needs for its light and cluster tables in the fragment stage.
///
/// The vertex stage draws a full-screen triangle (no vertex buffer): it covers clip space
/// `[-1, 3]` on both axes, so every pixel of the small render target below is inside it.
const FRAGMENT_STORAGE_SHADER: &str = r#"
@group(0) @binding(0)
var<storage, read> palette: array<vec4<f32>, 4>;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let index = u32(position.x);
    return palette[index];
}
"#;

fn expected_palette() -> [[f32; 4]; 4] {
    // Chosen so sRGB encoding is exact at every channel (0.0 and 1.0 both round-trip exactly
    // through the transfer function): a mismatch can only be a real logic error, never rounding.
    [
        [1.0, 0.0, 0.0, 1.0], // red
        [0.0, 1.0, 0.0, 1.0], // green
        [0.0, 0.0, 1.0, 1.0], // blue
        [1.0, 1.0, 1.0, 1.0], // white
    ]
}

fn palette_bytes(palette: &[[f32; 4]; 4]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 * 4 * 4);
    for color in palette {
        for component in color {
            bytes.extend_from_slice(&component.to_le_bytes());
        }
    }
    bytes
}

#[test]
fn fragment_stage_reads_a_storage_buffer_and_writes_the_render_target() {
    let Some(context) = elevated_context() else {
        return;
    };
    let downlevel = context.adapter().get_downlevel_capabilities();
    if !downlevel
        .flags
        .contains(wgpu::DownlevelFlags::FRAGMENT_STORAGE)
    {
        skip(
            "adapter lacks the FRAGMENT_STORAGE downlevel flag (no storage buffers in the fragment stage)",
        );
        return;
    }

    let device = context.device();
    let palette = expected_palette();
    let storage_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("wp3.1 fragment storage palette"),
        contents: &palette_bytes(&palette),
        usage: wgpu::BufferUsages::STORAGE,
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("wp3.1 fragment storage layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("wp3.1 fragment storage bind group"),
        layout: &bind_group_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: storage_buffer.as_entire_binding(),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("wp3.1 fragment storage pipeline layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let shader = context
        .capture_errors(|device| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("wp3.1 fragment storage shader"),
                source: wgpu::ShaderSource::Wgsl(FRAGMENT_STORAGE_SHADER.into()),
            })
        })
        .expect("shader compiles");
    let pipeline = context
        .capture_errors(|device| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("wp3.1 fragment storage pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
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
                        format: grimoire_gpu::OFFSCREEN_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
        .expect("pipeline creation");

    let target = OffscreenTarget::new(&context, 4, 1).expect("4x1 offscreen target");
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("wp3.1 fragment storage encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("wp3.1 fragment storage pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    context.queue().submit([encoder.finish()]);

    let image = target.read_rgba(&context).expect("read-back");
    for (index, expected) in palette.iter().enumerate() {
        let pixel = &image[index * 4..index * 4 + 4];
        let expected_srgb: [u8; 4] = [
            (expected[0] * 255.0).round() as u8,
            (expected[1] * 255.0).round() as u8,
            (expected[2] * 255.0).round() as u8,
            (expected[3] * 255.0).round() as u8,
        ];
        assert_eq!(
            pixel, expected_srgb,
            "pixel {index}: fragment-stage storage-buffer read did not reach the render target"
        );
    }
}

// --- Compute pass writes a storage buffer, read back and checked (plan 0002 WP3.1, item 2) -----

const COMPUTE_STORAGE_WGSL: &str = r#"
@group(0) @binding(0)
var<storage, read_write> output: array<u32, 4>;

@compute @workgroup_size(4)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    output[id.x] = id.x * 2u + 1u;
}
"#;

#[test]
fn compute_pass_writes_a_storage_buffer_and_the_readback_matches() {
    let Some(context) = elevated_context() else {
        return;
    };
    let downlevel = context.adapter().get_downlevel_capabilities();
    if !downlevel
        .flags
        .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
    {
        skip("adapter lacks the COMPUTE_SHADERS downlevel flag");
        return;
    }

    let device = context.device();
    const ELEMENTS: u64 = 4;
    let buffer_size = ELEMENTS * u64::from(u32::BITS / 8);
    let storage_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wp3.1 compute storage output"),
        size: buffer_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("wp3.1 compute storage layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("wp3.1 compute storage bind group"),
        layout: &bind_group_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: storage_buffer.as_entire_binding(),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("wp3.1 compute storage pipeline layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let shader = context
        .capture_errors(|device| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("wp3.1 compute storage shader"),
                source: wgpu::ShaderSource::Wgsl(COMPUTE_STORAGE_WGSL.into()),
            })
        })
        .expect("shader compiles");
    let pipeline = context
        .capture_errors(|device| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("wp3.1 compute storage pipeline"),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        })
        .expect("compute pipeline creation");

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("wp3.1 compute storage encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("wp3.1 compute storage pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    context.queue().submit([encoder.finish()]);

    let bytes = read_buffer(&context, &storage_buffer, buffer_size);
    let (chunks, _) = bytes.as_chunks::<4>();
    let values: Vec<u32> = chunks.iter().copied().map(u32::from_le_bytes).collect();
    assert_eq!(
        values,
        vec![1, 3, 5, 7],
        "compute-written storage buffer did not read back the expected values"
    );
}
