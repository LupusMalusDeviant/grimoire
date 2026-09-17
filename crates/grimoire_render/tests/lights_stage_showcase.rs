//! Plan 0002 WP3.7 showcase (M2): renders the `lights_stage` example's scene offscreen, headlessly,
//! as a numbered PNG sequence for the CI GIF job (`.github/workflows/ci.yml`, `showcase-gif`), which
//! assembles it with `ffmpeg` and uploads it as the artifact `lights_stage-showcase-gif`.
//!
//! The scene is the example's own `scene.rs`, included here with `#[path]`, so the GIF shows exactly
//! what `cargo run -p grimoire_render --example lights_stage` draws.
//!
//! - [`lights_stage_scene_fills_the_high_light_budget`] runs with every `cargo test` on the
//!   adapter at hand (skipped without one): one frame, checking that the scene really uses the
//!   High budget, draws every bullet and the marker, and derives bullet-cloud lights.
//! - [`render_lights_stage_png_sequence`] (`#[ignore]`, run by the GIF job):
//!   `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire_render --test lights_stage_showcase
//!   --locked -- --ignored --nocapture render_lights_stage_png_sequence`. Writes
//!   `frame_0000.png` … under `GRIMOIRE_LIGHTS_STAGE_GIF_OUT_DIR` (default
//!   `target/wp37-lights-stage-gif`, ignored by git).
//!
//! Cost like the `pbr_stage` showcase: 320x180, [`FRAME_COUNT`] frames, one OS, never on pull
//! requests.

#[path = "../examples/lights_stage/scene.rs"]
mod scene;

#[path = "support/mod.rs"]
mod support;

use std::io::Write as _;
use std::path::PathBuf;

use grimoire_render::{RenderError, Renderer, RendererConfig, StageFrame, WgpuRenderer};
use support::Image;

/// Most bullet-cloud lights the renderer derives per frame (contract §6, "Geschoss-Lichter aus dem
/// Bullet-Kanal": at most eight).
const BULLET_CLOUD_LIGHTS_MAX: u32 = 8;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

/// Frames of the loop: 72 at the GIF job's 24 fps is a three-second loop.
const FRAME_COUNT: u32 = 72;

/// One GPU device at a time in this binary (see `tests/offscreen.rs`).
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu_serial() -> std::sync::MutexGuard<'static, ()> {
    GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn out_dir() -> PathBuf {
    std::env::var_os("GRIMOIRE_LIGHTS_STAGE_GIF_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target").join("wp37-lights-stage-gif"))
}

fn offscreen_renderer() -> Option<WgpuRenderer> {
    let config = scene::renderer_config(RendererConfig {
        vsync: false,
        initial_sprite_capacity: 16,
        allow_software_fallback: true,
    });
    match WgpuRenderer::new_offscreen_staged(WIDTH, HEIGHT, config) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::NoAdapter) => None,
        Err(error) => panic!("offscreen renderer creation failed: {error}"),
    }
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
fn lights_stage_scene_fills_the_high_light_budget() {
    let _serial = gpu_serial();
    let Some(mut renderer) = offscreen_renderer() else {
        assert!(
            !adapter_required(),
            "no GPU adapter found although GRIMOIRE_REQUIRE_GPU_ADAPTER=1"
        );
        let _ = writeln!(
            std::io::stdout(),
            "\n::warning title=GPU test skipped::no GPU adapter found; lights_stage showcase rendered nothing"
        );
        return;
    };
    let meshes = scene::register_meshes(&mut renderer);
    let mut frame = StageFrame::new();
    scene::build_frame(&meshes, 0.25, &mut frame);
    let stats = renderer.render_stage(&frame).expect("render_stage");

    let own_lights = scene::CANDLES + scene::ORBIT_RINGS * scene::ORBIT_LIGHTS_PER_RING;
    assert_eq!(frame.point_lights.len() as u32, own_lights);
    assert_eq!(
        stats.bullets_drawn,
        scene::BULLET_ARMS * scene::BULLETS_PER_ARM
    );
    assert!(
        stats.bullet_point_lights_drawn > 0
            && stats.bullet_point_lights_drawn <= BULLET_CLOUD_LIGHTS_MAX,
        "{} bullet-cloud lights",
        stats.bullet_point_lights_drawn
    );
    assert_eq!(
        stats.point_lights_drawn,
        own_lights + stats.bullet_point_lights_drawn
    );
    assert_eq!(
        stats.point_lights_over_budget, 0,
        "the High budget holds every light"
    );
    assert_eq!(stats.base.sprites_drawn, 1, "the player marker");
}

/// Writes the showcase PNG sequence for the GIF job; see the module documentation.
#[test]
#[ignore = "WP3.7 showcase: run explicitly (see this file's module doc comment)"]
fn render_lights_stage_png_sequence() {
    let _serial = gpu_serial();
    let Some(mut renderer) = offscreen_renderer() else {
        eprintln!("no GPU adapter available; skipping the WP3.7 lights_stage showcase");
        return;
    };
    let dir = out_dir();
    std::fs::create_dir_all(&dir).expect("create output directory");
    let meshes = scene::register_meshes(&mut renderer);
    let mut frame = StageFrame::new();
    let mut lights = 0;
    for index in 0..FRAME_COUNT {
        scene::build_frame(&meshes, index as f32 / FRAME_COUNT as f32, &mut frame);
        let stats = renderer.render_stage(&frame).expect("render_stage");
        lights = lights.max(stats.point_lights_drawn);
        let rgba = renderer.read_offscreen_rgba().expect("read-back");
        Image::from_offscreen(WIDTH, HEIGHT, rgba)
            .write_png(&dir.join(format!("frame_{index:04}.png")))
            .expect("write frame PNG");
    }
    println!(
        "grimoire-lights-stage-showcase: frames={FRAME_COUNT} size={WIDTH}x{HEIGHT} max_point_lights={lights} platform={} dir={}",
        support::platform_dir(),
        dir.display()
    );
}
