//! Plan 0002 WP6.4: the render test scene `overlay` — the stats overlay with fixed profile values
//! over a simple stage, rendered offscreen on the software adapter.
//!
//! The overlay shows eight scopes: `sim` and `collide` over their budgets (red bars), the others
//! within (green), `app` without a budget (no bar) and `gpu` as an estimate (amber `~GPU`).
//!
//! - [`overlay_scene_marks_over_budget_rows_red`] runs with every `cargo test`: it checks colours
//!   and placement, independent of any reference image (skipped without an adapter, failing
//!   instead with `GRIMOIRE_REQUIRE_GPU_ADAPTER=1`).
//! - [`overlay_scene`] (`#[ignore]`, run by the CI step for the WP2.8 render test scenes) compares
//!   the image against `tests/snapshots/<platform>/overlay.png` with the tolerance metric and
//!   blocking rule of `grimoire_render`'s `tests/snapshot_scenes.rs` (a mismatch or a missing
//!   reference fails on Windows and Linux and only warns on macOS), and prints the same
//!   `grimoire-snapshot-*` lines, so `.github/scripts/report-snapshot-diff.sh` reports it too.
//!   `GRIMOIRE_SNAPSHOT_UPDATE=1` writes this platform's reference instead.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use grimoire::FrameStats;
use grimoire::adapters::debug::{
    Profiler, ProfilerBudgets, SCOPE_EXTRACT, SCOPE_FRAME, SCOPE_GPU, SCOPE_RENDER, SCOPE_SIM,
    StatsOverlay,
};
use grimoire::render::{
    RenderError, RenderStats, Renderer, RendererConfig, SpriteInstance, StageFrame, WgpuRenderer,
    shape,
};

// The PNG codec and tolerance metric of the WP2.8 scenes, shared rather than copied so both
// harnesses judge images the same way. That module's own tests name temporary files after the
// system time, which the facade's determinism lints forbid for simulation code; it is test support
// outside any simulation, so the lint is lifted for this module only.
#[allow(clippy::disallowed_methods)]
#[path = "../../grimoire_render/tests/support/mod.rs"]
mod snapshot_support;
use snapshot_support::Image;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const MS: Duration = Duration::from_millis(1);

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

/// The overlay with the scene's fixed values, visible.
fn scene_overlay() -> StatsOverlay {
    let mut profiler = Profiler::new(ProfilerBudgets::default());
    profiler.begin_frame(0);
    profiler.record("sigil", MS * 6 / 10);
    profiler.record("collide", MS * 19 / 10);
    profiler.record("app", MS * 3 / 10);
    profiler.record(SCOPE_SIM, MS * 48 / 10);
    profiler.record(SCOPE_EXTRACT, MS * 3 / 10);
    profiler.record(SCOPE_RENDER, MS * 21 / 10);
    profiler.record_estimate(SCOPE_GPU, MS * 65 / 10);
    profiler.record(SCOPE_FRAME, MS * 142 / 10);
    let stats = FrameStats {
        frame: 0,
        sim_tick: 1,
        ticks_this_frame: 1,
        alpha: 0.0,
        frame_time: MS * 142 / 10,
        fps: 70.4,
        dropped_time: Duration::ZERO,
        render: RenderStats::default(),
    };
    let mut overlay = StatsOverlay::new();
    overlay.record(&stats, Some(profiler.profile()));
    overlay.set_visible(true);
    overlay
}

/// Renders the scene, or `None` without an adapter.
fn render_scene() -> Option<Image> {
    let config = RendererConfig {
        vsync: false,
        initial_sprite_capacity: 256,
        allow_software_fallback: true,
    };
    let mut renderer = match WgpuRenderer::new_offscreen(WIDTH, HEIGHT, config) {
        Ok(renderer) => renderer,
        Err(RenderError::NoAdapter) => return None,
        Err(error) => panic!("offscreen renderer creation failed: {error}"),
    };
    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.08, 0.1, 0.16, 1.0];
    // World content under the overlay, so its translucent panel is visible as such.
    for (index, color) in [[0.8, 0.3, 0.1, 1.0], [0.2, 0.5, 0.9, 1.0]]
        .into_iter()
        .enumerate()
    {
        frame.base.sprites.push(SpriteInstance {
            position: [-40.0 + 45.0 * index as f32, 5.0],
            half_size: [30.0, 30.0],
            rotation: 0.0,
            shape: shape::CIRCLE,
            color,
        });
    }
    let mut overlay = scene_overlay();
    let drawn = overlay.draw(
        &frame.base.camera,
        [WIDTH as f32, HEIGHT as f32],
        &mut frame.debug_sprites,
    );
    assert!(drawn > 100, "the scene overlay has {drawn} sprites");
    renderer.render_stage(&frame).expect("renders");
    let rgba = renderer.read_offscreen_rgba().expect("reads back");
    Some(Image::from_offscreen(WIDTH, HEIGHT, rgba))
}

fn render_or_skip() -> Option<Image> {
    let image = render_scene();
    if image.is_none() {
        assert!(
            !adapter_required(),
            "no GPU adapter found although GRIMOIRE_REQUIRE_GPU_ADAPTER=1"
        );
        let _ = writeln!(
            std::io::stdout(),
            "\n::warning title=GPU test skipped::no GPU adapter found; grimoire overlay_scene rendered nothing"
        );
    }
    image
}

/// Pixels of `image` inside row band `rows` (inclusive start, exclusive end) matching `test`.
fn count_in_rows(image: &Image, rows: std::ops::Range<u32>, test: impl Fn(&[u8]) -> bool) -> usize {
    rows.flat_map(|y| (0..image.width).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let start = ((y * image.width + x) * 4) as usize;
            test(&image.rgba[start..start + 4])
        })
        .count()
}

fn is_red(pixel: &[u8]) -> bool {
    pixel[0] > 200 && pixel[1] < 120 && pixel[2] < 120
}

fn is_green(pixel: &[u8]) -> bool {
    pixel[1] > 190 && pixel[0] < 170 && pixel[2] < 190
}

/// Pixel row band of overlay row `index` (0 = header) at scale 1: margin 2, padding 2, 9 per row.
fn row_band(index: u32) -> std::ops::Range<u32> {
    let top = 4 + index * 9;
    top..top + 7
}

#[test]
fn overlay_scene_marks_over_budget_rows_red() {
    let _serial = gpu_serial();
    let Some(image) = render_or_skip() else {
        return;
    };
    // Row order: header, frame, sim, sigil, collide, app, extract, render, gpu.
    let expectations = [
        ("frame", 1, false),
        ("sim", 2, true),
        ("sigil", 3, false),
        ("collide", 4, true),
        ("extract", 6, false),
        ("render", 7, false),
        ("gpu", 8, false),
    ];
    for (name, row, over) in expectations {
        let red = count_in_rows(&image, row_band(row), is_red);
        let green = count_in_rows(&image, row_band(row), is_green);
        if over {
            assert!(red >= 30, "{name}: {red} red pixels");
            assert_eq!(green, 0, "{name}: no green fill over budget");
        } else {
            assert!(green >= 5, "{name}: {green} green pixels");
            assert_eq!(red, 0, "{name}: no red within budget");
        }
    }
    // `app` has no budget and so no bar at all.
    let app_bar = count_in_rows(&image, row_band(5), |pixel| {
        is_red(pixel) || is_green(pixel)
    });
    assert_eq!(app_bar, 0, "app draws no bar");
    // The amber `~GPU` name marks the estimate.
    let amber = count_in_rows(&image, row_band(8), |pixel| {
        pixel[0] > 230 && (140..230).contains(&pixel[1]) && pixel[2] < 120
    });
    assert!(amber >= 10, "gpu name drawn in amber: {amber} pixels");
    // Below the panel, the stage shows through untouched by the overlay.
    let below = count_in_rows(&image, 88..90, |pixel| is_red(pixel) || is_green(pixel));
    assert_eq!(below, 0);
}

fn reference_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(snapshot_support::platform_dir())
        .join("overlay.png")
}

fn candidate_dir() -> PathBuf {
    std::env::var_os("GRIMOIRE_SNAPSHOT_CANDIDATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("target").join("wp28-snapshots"))
}

#[test]
#[ignore = "WP2.8-style render test scene; the CI scene step runs it with --ignored"]
fn overlay_scene() {
    let _serial = gpu_serial();
    let Some(image) = render_scene() else {
        eprintln!("no GPU adapter available; skipping the overlay scene");
        return;
    };
    let platform = snapshot_support::platform_dir();
    let path = reference_path();
    let mut stdout = std::io::stdout();
    if matches!(
        std::env::var("GRIMOIRE_SNAPSHOT_UPDATE").ok().as_deref(),
        Some("1" | "true")
    ) {
        image.write_png(&path).expect("writes the reference");
        let _ = writeln!(
            stdout,
            "grimoire-snapshot-updated: name=overlay path={}",
            path.display()
        );
        return;
    }
    let blocks = snapshot_support::blocks_on_mismatch(platform);
    let level = if blocks { "error" } else { "warning" };
    let candidate = candidate_dir().join(platform).join("overlay.png");
    if !path.exists() {
        image.write_png(&candidate).expect("writes the candidate");
        let _ = writeln!(
            stdout,
            "grimoire-snapshot-no-reference: name=overlay platform={platform} candidate={}\n\
             ::{level} title=Render test scene has no reference::scene \"overlay\" on {platform} \
             has no committed reference image; a candidate was written to {}",
            candidate.display(),
            candidate.display()
        );
        assert!(
            !blocks,
            "scene \"overlay\" has no reference on {platform}, where rendering blocks: review the \
             candidate (CI artifact snapshot-candidates-<runner>) and commit it as \
             tests/snapshots/{platform}/overlay.png"
        );
        return;
    }
    let reference = Image::read_png(&path).expect("reads the reference");
    let metric = snapshot_support::compare(&reference, &image)
        .expect("reference and rendered image have the same size");
    let _ = writeln!(
        stdout,
        "grimoire-snapshot-diff: name=overlay platform={platform} mean_abs_diff={:.3} \
         max_abs_diff={} mean_tolerance={:.3} max_tolerance={} within_tolerance={}",
        metric.mean_abs_diff,
        metric.max_abs_diff,
        snapshot_support::MEAN_ABS_DIFF_TOLERANCE,
        snapshot_support::MAX_ABS_DIFF_TOLERANCE,
        metric.within_tolerance()
    );
    if !metric.within_tolerance() {
        let _ = image.write_png(&candidate);
        let comparison = candidate_dir()
            .join(platform)
            .join("overlay_reference_vs_candidate.png");
        let _ = Image::beside(&[&reference, &image]).write_png(&comparison);
        let _ = writeln!(
            stdout,
            "\n::{level} title=Render test scene mismatch::scene \"overlay\" on {platform} exceeds \
             tolerance; candidate written to {}",
            candidate.display()
        );
        assert!(
            !blocks,
            "scene \"overlay\" on {platform} mismatches its reference beyond tolerance: \
             mean_abs_diff={:.3} (tolerance {:.3}), max_abs_diff={} (tolerance {}); rendering on \
             {platform} blocks",
            metric.mean_abs_diff,
            snapshot_support::MEAN_ABS_DIFF_TOLERANCE,
            metric.max_abs_diff,
            snapshot_support::MAX_ABS_DIFF_TOLERANCE
        );
    }
}
