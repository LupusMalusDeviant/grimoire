//! Plan 0002 WP5.7, the M2 showcase: the scene of the example `sigil_curtain`
//! (`examples/sigil_curtain/scene.rs`, included below with `#[path]`) through the real main loop.
//!
//! - [`the_curtain_scene_runs_headlessly_and_hands_its_bullets_over`] drives the loop with the
//!   `NullRenderer` and checks the pattern fills the stage without dropping a spawn;
//! - [`the_curtain_scene_renders_offscreen`] renders a few small frames on the GPU (skipped without
//!   an adapter, failing instead with `GRIMOIRE_REQUIRE_GPU_ADAPTER=1`);
//! - [`render_sigil_curtain_png_sequence`] (`#[ignore]`, run explicitly) writes the showcase frame
//!   sequence as numbered PNGs, which the CI job `sigil-curtain-gif` assembles into the GIF:
//!   `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire --test sigil_curtain_showcase --locked --
//!   --ignored --nocapture render_sigil_curtain_png_sequence`. Frames go to
//!   `GRIMOIRE_SIGIL_CURTAIN_GIF_OUT_DIR` (default `target/wp57-sigil-curtain`, ignored by git).
//!
//! The GPU tests share one device at a time through [`GPU_SERIAL`] (see `tests/asset_hook.rs`).

mod support;

#[path = "../examples/sigil_curtain/scene.rs"]
mod scene;

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use grimoire::prelude::*;
use grimoire::render::RenderError;
use grimoire::{GrimoireError, OffscreenRun};
use scene::CurtainFrame;
use support::Image;

/// One frame of a 24 frames-per-second GIF: 2.5 ticks at 60 Hz, so every other frame lands between
/// two ticks and is interpolated.
const GIF_FRAME: Duration = Duration::from_nanos(41_666_667);

static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_serial() -> std::sync::MutexGuard<'static, ()> {
    GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn adapter_required() -> bool {
    matches!(
        std::env::var("GRIMOIRE_REQUIRE_GPU_ADAPTER")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true")
    )
}

#[test]
fn the_curtain_scene_runs_headlessly_and_hands_its_bullets_over() {
    let (app, last_frame) = scene::app(WindowConfig::default());
    let report = app
        .run_headless_frames(240, GIF_FRAME)
        .expect("the headless loop runs");
    assert_eq!(report.frames, 240);
    assert_eq!(report.final_tick, 600, "2.5 ticks per frame");

    let frame: CurtainFrame = last_frame.get();
    assert!(
        frame.bullets.extracted > 400,
        "the curtain fills the stage ({frame:?})"
    );
    assert_eq!(
        frame.bullets.extracted, frame.live,
        "every live bullet reaches the bullet pass"
    );
    assert_eq!(frame.bullets.unmapped_visual, 0);
    assert_eq!(frame.dropped_spawns, 0);
}

/// Runs `frames` frames of the scene offscreen at `width` × `height` and hands frame `n` with
/// `n >= skip` to `capture`. `None` without an adapter.
fn run_offscreen(
    width: u32,
    height: u32,
    frames: u64,
    skip: u64,
    capture: &mut dyn FnMut(u64, &[u8], CurtainFrame),
) -> Option<LoopSummary> {
    let (app, last_frame) = scene::app(WindowConfig::default());
    let mut run = OffscreenRun::new(width, height, frames, GIF_FRAME);
    run.capture_every = 1;
    let result = app.run_offscreen(run, &mut |_, _| {}, &mut |frame, rgba| {
        if frame >= skip {
            capture(frame - skip, rgba, last_frame.get());
        }
    });
    match result {
        Ok(report) => Some(LoopSummary {
            frames: report.frames,
            final_tick: report.final_tick,
        }),
        Err(GrimoireError::Render(RenderError::NoAdapter)) => {
            assert!(
                !adapter_required(),
                "no GPU adapter found although GRIMOIRE_REQUIRE_GPU_ADAPTER=1"
            );
            let _ = writeln!(
                std::io::stdout(),
                "\n::warning title=GPU test skipped::no GPU adapter found; grimoire sigil_curtain_showcase rendered nothing"
            );
            None
        }
        Err(error) => panic!("offscreen run failed: {error}"),
    }
}

struct LoopSummary {
    frames: u64,
    final_tick: u64,
}

/// Number of pixels whose colour differs by more than `threshold` in any channel.
fn changed_pixels(a: &[u8], b: &[u8], threshold: u8) -> usize {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(pa, pb)| {
            pa.iter()
                .zip(pb.iter())
                .any(|(x, y)| x.abs_diff(*y) > threshold)
        })
        .count()
}

#[test]
fn the_curtain_scene_renders_offscreen() {
    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 90;
    let _serial = gpu_serial();
    let mut images: Vec<(u64, Vec<u8>, CurtainFrame)> = Vec::new();
    let Some(summary) = run_offscreen(WIDTH, HEIGHT, 49, 0, &mut |frame, rgba, counters| {
        if frame == 0 || frame == 48 {
            images.push((frame, rgba.to_vec(), counters));
        }
    }) else {
        return;
    };
    assert_eq!(summary.frames, 49);
    assert_eq!(summary.final_tick, 122);
    let [(_, first, early), (_, last, late)] = images.as_slice() else {
        panic!("frames 0 and 48 are captured");
    };
    assert!(early.bullets.extracted < 20, "{early:?}");
    assert!(late.bullets.extracted > 200, "{late:?}");
    assert_eq!(late.dropped_spawns, 0);

    // The stage stands still; what changes between the frames are the bullets and their light.
    let changed = changed_pixels(first, last, 40);
    assert!(
        changed > 150,
        "only {changed} of {} pixels changed with {} bullets on the stage",
        WIDTH * HEIGHT,
        late.bullets.extracted
    );
}

fn out_dir() -> PathBuf {
    std::env::var_os("GRIMOIRE_SIGIL_CURTAIN_GIF_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target").join("wp57-sigil-curtain"))
}

#[test]
#[ignore = "writes a PNG frame sequence; run explicitly (see this file's module docs)"]
fn render_sigil_curtain_png_sequence() {
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 360;
    /// Frames before the first written one, so the curtain already reaches the pillars (120 ticks).
    const WARM_UP_FRAMES: u64 = 48;
    /// Three seconds at 24 frames per second.
    const FRAMES: u64 = 72;

    let _serial = gpu_serial();
    let dir = out_dir();
    std::fs::create_dir_all(&dir).expect("output directory");
    let summary = run_offscreen(
        WIDTH,
        HEIGHT,
        WARM_UP_FRAMES + FRAMES,
        WARM_UP_FRAMES,
        &mut |frame, rgba, counters| {
            let image = Image::from_offscreen(WIDTH, HEIGHT, rgba.to_vec());
            let path = dir.join(format!("frame_{frame:04}.png"));
            image.write_png(&path).expect("write PNG");
            assert_eq!(counters.dropped_spawns, 0);
            let _ = writeln!(
                std::io::stdout(),
                "frame {frame:02}: bullets {} (live {}) -> {}",
                counters.bullets.extracted,
                counters.live,
                path.display()
            );
        },
    )
    .expect("the PNG sequence needs a GPU or software adapter");
    assert_eq!(summary.frames, WARM_UP_FRAMES + FRAMES);
}
