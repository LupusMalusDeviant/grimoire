//! GPU time per frame through timestamp queries (plan 0002 WP6.3, PRD-0002 FR-12: "GPU über
//! Timestamp-Queries, sonst gekennzeichneter Fallback").
//!
//! Every pass of [`crate::WgpuRenderer::render_stage`] writes a timestamp at its beginning and at
//! its end into one query set. After the frame's last pass, one extra command buffer resolves the
//! written queries and copies them into a mappable staging buffer, whose `map_async` completes
//! some frames later. [`GpuTimer::begin_frame`] of a later frame picks the result up without ever
//! waiting for the GPU (`PollType::Poll`, the rule engine ADR-0015 set for the cluster stats), so
//! the reported time lags the drawn frame by roughly one to three frames.
//!
//! The reported value is the **sum of the pass durations**: time the GPU spent executing the
//! frame's render and compute passes. Queue uploads (`write_buffer`), the resolve itself, idle time
//! between two submissions and presentation are not part of it.
//!
//! While a readback is still pending, the following frames write no timestamps at all (one
//! staging buffer, never copied into while mapped); the last completed value keeps being reported.
//! Without `wgpu::Features::TIMESTAMP_QUERY` on the device (for example the macOS CI runner's
//! paravirtual Metal adapter) the timer is disabled and reports `None`, and the facade records a
//! marked estimate instead (contract §9.7).

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use grimoire_gpu::{GpuContext, wgpu};

/// Most passes timed per frame: shadow map, cluster compute, meshes, world sprites, bullets,
/// marker sprites and debug sprites, plus one spare. A pass beyond it simply runs untimed.
const MAX_TIMED_PASSES: u32 = 8;

/// Two queries (beginning and end) per timed pass.
const QUERY_COUNT: u32 = MAX_TIMED_PASSES * 2;

/// Size of one resolved timestamp.
const QUERY_BYTES: u64 = 8;

/// The query indices one pass writes to.
pub(crate) struct PassTimestamps {
    query_set: wgpu::QuerySet,
    beginning: u32,
    end: u32,
}

impl PassTimestamps {
    /// Timestamp writes for a render pass descriptor.
    pub(crate) fn render(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(self.beginning),
            end_of_pass_write_index: Some(self.end),
        }
    }

    /// Timestamp writes for a compute pass descriptor.
    pub(crate) fn compute(&self) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(self.beginning),
            end_of_pass_write_index: Some(self.end),
        }
    }
}

/// Outcome channel of the staging buffer's `map_async`.
type MapResult = Receiver<Result<(), wgpu::BufferAsyncError>>;

enum Readback {
    Idle,
    /// A resolve of `passes` timed passes was submitted and is being mapped.
    Pending {
        receiver: MapResult,
        passes: u32,
    },
}

/// GPU resources of an enabled timer.
struct Enabled {
    query_set: wgpu::QuerySet,
    resolve_buffer: wgpu::Buffer,
    staging_buffer: wgpu::Buffer,
    /// Nanoseconds per timestamp tick (`Queue::get_timestamp_period`).
    period_ns: f64,
    /// Whether the current frame writes timestamps.
    measuring: bool,
    /// Timed passes handed out in the current frame.
    passes: u32,
    readback: Readback,
    last: Option<Duration>,
}

/// Per-frame GPU pass timing; disabled when the device has no timestamp queries.
pub(crate) struct GpuTimer {
    enabled: Option<Enabled>,
}

impl GpuTimer {
    /// Creates the timer for `context`'s device: enabled exactly when the device was created with
    /// `wgpu::Features::TIMESTAMP_QUERY` ([`GpuContext::supports_timestamp_queries`]).
    pub(crate) fn new(context: &GpuContext) -> Self {
        if !context.supports_timestamp_queries() {
            return Self { enabled: None };
        }
        let device = context.device();
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("grimoire gpu timer queries"),
            ty: wgpu::QueryType::Timestamp,
            count: QUERY_COUNT,
        });
        let size = u64::from(QUERY_COUNT) * QUERY_BYTES;
        let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire gpu timer resolve"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grimoire gpu timer staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            enabled: Some(Enabled {
                query_set,
                resolve_buffer,
                staging_buffer,
                period_ns: f64::from(context.queue().get_timestamp_period()),
                measuring: false,
                passes: 0,
                readback: Readback::Idle,
                last: None,
            }),
        }
    }

    /// Whether this timer measures at all.
    #[cfg(test)]
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled.is_some()
    }

    /// Picks up a completed readback without waiting and decides whether this frame writes
    /// timestamps (only while no readback is pending).
    pub(crate) fn begin_frame(&mut self, context: &GpuContext) {
        let Some(timer) = &mut self.enabled else {
            return;
        };
        timer.pump(context);
        timer.passes = 0;
        timer.measuring = matches!(timer.readback, Readback::Idle);
    }

    /// Query indices for the next pass of this frame, or `None` if this frame is not measured, the
    /// timer is disabled or [`MAX_TIMED_PASSES`] passes were already handed out. Call it only
    /// directly before the pass is actually recorded: every handed-out pair is resolved.
    pub(crate) fn next_pass(&mut self) -> Option<PassTimestamps> {
        let timer = self.enabled.as_mut()?;
        if !timer.measuring || timer.passes >= MAX_TIMED_PASSES {
            return None;
        }
        let beginning = timer.passes * 2;
        timer.passes += 1;
        Some(PassTimestamps {
            query_set: timer.query_set.clone(),
            beginning,
            end: beginning + 1,
        })
    }

    /// Submits the resolve of this frame's timestamps and starts the non-blocking readback. Call
    /// after the frame's last pass was submitted; does nothing if no pass was timed.
    pub(crate) fn end_frame(&mut self, context: &GpuContext) {
        let Some(timer) = &mut self.enabled else {
            return;
        };
        if !timer.measuring || timer.passes == 0 {
            return;
        }
        timer.measuring = false;
        let passes = timer.passes;
        let bytes = u64::from(passes * 2) * QUERY_BYTES;
        let submitted = context.capture_errors(|device| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("grimoire gpu timer resolve encoder"),
            });
            encoder.resolve_query_set(&timer.query_set, 0..passes * 2, &timer.resolve_buffer, 0);
            encoder.copy_buffer_to_buffer(
                &timer.resolve_buffer,
                0,
                &timer.staging_buffer,
                0,
                bytes,
            );
            context.queue().submit([encoder.finish()]);
        });
        if let Err(error) = submitted {
            // A diagnostic value: keep reporting the last good one rather than failing the frame.
            log::warn!("GPU timestamp resolve failed, skipping this measurement: {error}");
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        timer
            .staging_buffer
            .slice(..bytes)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        timer.readback = Readback::Pending { receiver, passes };
    }

    /// GPU time of the most recently completed measurement, `None` while disabled or before the
    /// first measurement completed.
    pub(crate) fn last(&self) -> Option<Duration> {
        self.enabled.as_ref().and_then(|timer| timer.last)
    }
}

impl Enabled {
    /// Non-blocking check of a pending readback; updates `last` once it resolved.
    fn pump(&mut self, context: &GpuContext) {
        let Readback::Pending { receiver, passes } = &self.readback else {
            return;
        };
        let passes = *passes;
        let _ = context.device().poll(wgpu::PollType::Poll);
        let outcome = receiver.try_recv();
        match outcome {
            Ok(Ok(())) => {
                let bytes = u64::from(passes * 2) * QUERY_BYTES;
                if let Ok(view) = self.staging_buffer.get_mapped_range(..bytes) {
                    // Read value by value: the mapped bytes need not be aligned for `u64`.
                    let stamps: Vec<u64> = view
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|chunk| u64::from_ne_bytes(*chunk))
                        .collect();
                    drop(view);
                    self.last = Some(sum_pass_durations(&stamps, self.period_ns));
                }
                self.staging_buffer.unmap();
                self.readback = Readback::Idle;
            }
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => self.readback = Readback::Idle,
            Err(TryRecvError::Empty) => {}
        }
    }
}

/// Sum of `end − beginning` over `(beginning, end)` timestamp pairs, converted with `period_ns`
/// nanoseconds per tick. A pair whose end lies before its beginning counts as zero.
fn sum_pass_durations(stamps: &[u64], period_ns: f64) -> Duration {
    let ticks: u64 = stamps
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[beginning, end]| end.saturating_sub(*beginning))
        .fold(0, u64::saturating_add);
    // `as` saturates for out-of-range floats; precision loss beyond 2^53 ticks is irrelevant.
    Duration::from_nanos((ticks as f64 * period_ns).round() as u64)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;
    use crate::{Renderer, RendererConfig, SpriteInstance, StageFrame, WgpuRenderer};

    #[test]
    fn pass_durations_sum_and_convert_with_the_period() {
        let stamps = [100, 150, 200, 260, 300, 290];
        assert_eq!(sum_pass_durations(&stamps, 1.0), Duration::from_nanos(110));
        assert_eq!(sum_pass_durations(&stamps, 2.5), Duration::from_nanos(275));
        assert_eq!(sum_pass_durations(&[], 1.0), Duration::ZERO);
        assert_eq!(
            sum_pass_durations(&[0, u64::MAX, 0, u64::MAX], 1.0),
            Duration::from_nanos(u64::MAX)
        );
    }

    #[test]
    fn render_stage_reports_gpu_time_once_a_readback_completed() {
        let mut renderer = match WgpuRenderer::new_offscreen(64, 36, RendererConfig::default()) {
            Ok(renderer) => renderer,
            Err(error) => {
                // Direct stdout writes reach the CI log although the test passes (libtest captures
                // only the print macros), like the adapter lines of `tests/offscreen.rs`.
                let _ = writeln!(
                    std::io::stdout(),
                    "\n::warning title=WP6.3 GPU timer test skipped::no GPU adapter available ({error})"
                );
                return;
            }
        };
        let mut frame = StageFrame::new();
        frame.base.sprites.push(SpriteInstance {
            half_size: [0.5, 0.5],
            color: [1.0, 0.5, 0.0, 1.0],
            ..SpriteInstance::default()
        });
        let first = renderer.render_stage(&frame).expect("renders");
        assert_eq!(
            first.gpu_time, None,
            "no readback can have completed before the first frame"
        );
        if !renderer.gpu_timer_enabled() {
            let _ = writeln!(
                std::io::stdout(),
                "\ngrimoire-gpu-timer: timestamp_queries=false gpu_time=none {}",
                renderer.adapter_report_line()
            );
            for _ in 0..3 {
                let stats = renderer.render_stage(&frame).expect("renders");
                assert_eq!(stats.gpu_time, None);
            }
            return;
        }
        // The readback never blocks, so give the (software) GPU a moment between frames.
        let mut measured = None;
        for _ in 0..2_000 {
            let stats = renderer.render_stage(&frame).expect("renders");
            if stats.gpu_time.is_some() {
                measured = stats.gpu_time;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let measured = measured.expect("a timestamp readback completes within 2,000 frames");
        let _ = writeln!(
            std::io::stdout(),
            "\ngrimoire-gpu-timer: timestamp_queries=true gpu_time_ns={} frame=64x36 {}",
            measured.as_nanos(),
            renderer.adapter_report_line()
        );
        assert!(
            measured < Duration::from_secs(10),
            "implausible GPU time {measured:?}"
        );
        // A zero-size target skips the frame and reports no GPU time.
        renderer.resize(0, 0);
        let skipped = renderer
            .render_stage(&frame)
            .expect("a skipped frame is no error");
        assert_eq!(skipped.gpu_time, None);
    }
}
