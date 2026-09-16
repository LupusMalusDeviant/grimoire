//! Light-culling spike measurement (plan 0002 WP3.2): CPU-froxel cluster assignment vs.
//! compute-shader clustering, both on the frozen 16x9x24 froxel grid
//! (`grimoire_render::cluster_layout`) at 256 lights (the "High" budget). Measures, per
//! repetition: the CPU-froxel path's assignment cost and its full three-buffer upload; the
//! compute path's (much smaller) light-only upload and its *relative-only* GPU dispatch time
//! (see README.md — never an absolute-millisecond judgement).
//!
//! Every number is a "Runner-Wert": median of `REPS` repetitions plus min/max spread, after
//! `WARMUP` discarded repetitions, measured on this spike's CI Linux runner only (engine
//! ADR-0010: a shared-runner wall-clock number is a trend, not a budget proof).

use std::time::Instant;

use grimoire_gpu::{GpuContext, GpuError, wgpu};
use grimoire_render::cluster_layout::{
    CLUSTER_GRID_X, CLUSTER_GRID_Y, CLUSTER_GRID_Z, LIGHT_BUDGET_HIGH, cluster_table_bytes,
    light_index_list_worst_case_bytes, light_list_bytes,
};
use light_bullet_stress::compute_cluster::{ComputeClusterPass, Grid};
use light_bullet_stress::gpu::elevated_context;
use light_bullet_stress::lights::{cpu_assign_clusters, synthetic_lights};
use light_bullet_stress::stats::Summary;

const WARMUP: usize = 3;
const REPS: usize = 10;
const LIGHT_COUNT: usize = LIGHT_BUDGET_HIGH; // 256, "256 Lichtern" per the plan text

fn create_storage_buffer(
    context: &GpuContext,
    size: u64,
    label: &'static str,
) -> Result<wgpu::Buffer, GpuError> {
    context.capture_errors(|device| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size.max(4),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    })
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

fn print_summary(label: &str, summary: &Summary) {
    println!(
        "wp32-light: metric={label} median_us={:.2} min_us={:.2} max_us={:.2} samples={}",
        summary.median_us, summary.min_us, summary.max_us, summary.samples
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let context = elevated_context()?;
    println!("{}", context.adapter_report_line());

    let lights = synthetic_lights(LIGHT_COUNT, 0x11_6807);
    let light_bytes = light_list_bytes(LIGHT_COUNT) as u64;
    let table_bytes = cluster_table_bytes() as u64;
    let index_bytes = light_index_list_worst_case_bytes(LIGHT_COUNT) as u64;

    // --- CPU-froxel path: assignment on the CPU, then all three buffers uploaded from it -------
    let light_buffer = create_storage_buffer(&context, light_bytes, "wp3.2 cpu-froxel lights")?;
    let cluster_table_buffer =
        create_storage_buffer(&context, table_bytes, "wp3.2 cpu-froxel cluster table")?;
    let index_list_buffer =
        create_storage_buffer(&context, index_bytes, "wp3.2 cpu-froxel index list")?;

    let mut cpu_assign_us = Vec::with_capacity(REPS);
    let mut cpu_upload_us = Vec::with_capacity(REPS);
    for rep in 0..(WARMUP + REPS) {
        let t0 = Instant::now();
        let assignment = cpu_assign_clusters(&lights, LIGHT_BUDGET_HIGH);
        let assign_elapsed = t0.elapsed();

        let t1 = Instant::now();
        context
            .queue()
            .write_buffer(&light_buffer, 0, bytemuck::cast_slice(&lights));
        context.queue().write_buffer(
            &cluster_table_buffer,
            0,
            bytemuck::cast_slice(&assignment.cluster_table),
        );
        context.queue().write_buffer(
            &index_list_buffer,
            0,
            bytemuck::cast_slice(&assignment.index_list),
        );
        let upload_elapsed = t1.elapsed();

        if rep >= WARMUP {
            cpu_assign_us.push(assign_elapsed.as_secs_f64() * 1e6);
            cpu_upload_us.push(upload_elapsed.as_secs_f64() * 1e6);
        }
    }
    let cpu_assign_summary = Summary::from_micros(cpu_assign_us);
    let cpu_upload_summary = Summary::from_micros(cpu_upload_us);

    // --- Compute-clustering path: only the (small) light buffer is ever uploaded from the CPU --
    let grid = Grid {
        x: CLUSTER_GRID_X,
        y: CLUSTER_GRID_Y,
        z: CLUSTER_GRID_Z,
    };
    let compute_pass = ComputeClusterPass::new(
        &context,
        grid,
        u32::try_from(LIGHT_BUDGET_HIGH).expect("light budget fits u32"),
        u32::try_from(LIGHT_COUNT).expect("light count fits u32"),
    )?;

    let mut compute_upload_us = Vec::with_capacity(REPS);
    let mut compute_gpu_us = Vec::with_capacity(REPS);
    for rep in 0..(WARMUP + REPS) {
        let t0 = Instant::now();
        compute_pass.upload_lights(&context, &lights);
        let upload_elapsed = t0.elapsed();

        let t1 = Instant::now();
        let submission = compute_pass.dispatch(&context)?;
        poll_wait(&context, submission)?;
        let gpu_elapsed = t1.elapsed();

        if rep >= WARMUP {
            compute_upload_us.push(upload_elapsed.as_secs_f64() * 1e6);
            compute_gpu_us.push(gpu_elapsed.as_secs_f64() * 1e6);
        }
    }
    let compute_upload_summary = Summary::from_micros(compute_upload_us);
    let compute_gpu_summary = Summary::from_micros(compute_gpu_us);

    println!(
        "## WP3.2 Licht-Culling: CPU-Froxel-Zuordnung vs. Compute-Clustering (Runner-Werte, Median aus {REPS} Wiederholungen nach {WARMUP} Aufwaermlaeufen, 256 Lichter, Raster 16x9x24)"
    );
    println!();
    print_summary("cpu_froxel_assignment", &cpu_assign_summary);
    print_summary("cpu_froxel_upload_all_three_buffers", &cpu_upload_summary);
    print_summary("compute_upload_light_buffer_only", &compute_upload_summary);
    print_summary("compute_gpu_relative_dispatch", &compute_gpu_summary);
    println!();
    println!(
        "| Weg | CPU-Zuordnung Median (us) | CPU-Zuordnung Spanne (us) | Upload Median (us) | Upload Spanne (us) | GPU relativ Median (us) | GPU relativ Spanne (us) |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|");
    println!(
        "| CPU-Froxel | {:.2} | {:.2}-{:.2} | {:.2} | {:.2}-{:.2} | n/a (kein Compute-Dispatch) | n/a |",
        cpu_assign_summary.median_us,
        cpu_assign_summary.min_us,
        cpu_assign_summary.max_us,
        cpu_upload_summary.median_us,
        cpu_upload_summary.min_us,
        cpu_upload_summary.max_us,
    );
    println!(
        "| Compute-Clustering | ~0 (auf der GPU, siehe GPU relativ) | n/a | {:.2} | {:.2}-{:.2} | {:.2} | {:.2}-{:.2} |",
        compute_upload_summary.median_us,
        compute_upload_summary.min_us,
        compute_upload_summary.max_us,
        compute_gpu_summary.median_us,
        compute_gpu_summary.min_us,
        compute_gpu_summary.max_us,
    );
    println!();
    println!(
        "### Speicher- und Upload-Aufwand (High-Budget, 256 Lichter, aus grimoire_render::cluster_layout)"
    );
    println!();
    println!("| Puffer | Bytes | CPU-Froxel laedt hoch | Compute-Clustering laedt hoch |");
    println!("|---|---:|---|---|");
    println!("| Lichtliste | {light_bytes} | ja | ja |");
    println!("| Cluster-Tabelle | {table_bytes} | ja | nein (schreibt die GPU) |");
    println!("| Index-Liste (worst case) | {index_bytes} | ja | nein (schreibt die GPU) |");
    println!(
        "| Summe je Frame | {} | {} | {light_bytes} |",
        light_bytes + table_bytes + index_bytes,
        light_bytes + table_bytes + index_bytes,
    );

    Ok(())
}
