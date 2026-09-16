//! Plan 0002 WP2.8 milestone showcase (M1): renders the merged stage (procedural meshes, PBR
//! materials, lights, the key-light shadow map, the tilted camera — the same cast as the
//! `pbr_stage` example, `crates/grimoire_render/examples/pbr_stage.rs`) offscreen, headlessly, as a
//! numbered PNG sequence. `.github/workflows/ci.yml`'s `showcase-gif` job assembles the sequence
//! into an animated GIF with `ffmpeg` and uploads it as a build artifact — CI has no window system,
//! so this is the only way to produce the showcase there at all (the `pbr_stage` example itself is
//! only ever built in CI, never run, exactly because it needs a real window).
//!
//! **Cost:** deliberately small — 320x180 and [`FRAME_COUNT`] frames on the software
//! adapter, one OS (`showcase-gif`'s `ubuntu-latest` runs it exactly once, since the GIF is a
//! showcase artifact, not a per-platform correctness check — that is what `snapshot_scenes.rs`'s
//! per-platform references are for), and **skipped on pull requests** (only a push to `main` or
//! `workflow_dispatch` runs it): a milestone GIF is worth refreshing on every merge, not worth
//! paying for on every review iteration of a pull request. Unlike `snapshot_scenes.rs`'s
//! committed references, these frames are a build artifact CI uploads and expires, never a
//! checked-in file, so the extra pixels cost CI storage/time, not permanent repository size, and
//! 320x180 is worth it here for a more watchable result.
//!
//! Ignored like every other WP2.8/WP2.6 offscreen showcase test in this crate; run explicitly:
//! `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire_render --test pbr_stage_showcase --locked
//! -- --ignored --nocapture render_pbr_stage_png_sequence`. Writes numbered PNGs under
//! `GRIMOIRE_PBR_STAGE_GIF_OUT_DIR` (default `target/wp28-pbr-stage-gif`); like
//! `tests/wp26_shadow_showcase.rs`, the caller is responsible for choosing a directory outside the
//! repository if the frames should not be committed (they never are: `target/` is already ignored
//! by the workspace root `.gitignore`, and the default lands there).

use std::path::PathBuf;

use grimoire_render::procedural::{altar_block, capsule_actor, floor_tile_grid, octagonal_pillar};
use grimoire_render::{
    AmbientLight, Camera25D, DirectionalLight, MaterialHandle, MeshHandle, MeshInstance,
    PbrMaterial, PointLight, RenderError, Renderer, RendererConfig, ShadowConfig, ShadowMode,
    StageFrame, WgpuRenderer,
};

#[path = "support/mod.rs"]
mod support;
use support::Image;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

/// Frame count of the rendered loop (see this file's "Cost" doc section): at the CI job's assembled
/// frame rate (`showcase-gif`, 24 fps) this is a 2-second loop, long enough to show the camera tilt
/// animation complete a full cycle.
const FRAME_COUNT: u32 = 48;

fn out_dir() -> PathBuf {
    std::env::var_os("GRIMOIRE_PBR_STAGE_GIF_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target").join("wp28-pbr-stage-gif"))
}

fn offscreen_renderer() -> Option<WgpuRenderer> {
    let config = RendererConfig {
        vsync: false,
        initial_sprite_capacity: 16,
        allow_software_fallback: true,
    };
    match WgpuRenderer::new_offscreen(WIDTH, HEIGHT, config) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::NoAdapter) => None,
        Err(error) => panic!("offscreen renderer creation failed: {error}"),
    }
}

fn translation(t: [f32; 3]) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [t[0], t[1], t[2], 1.0],
    ]
}

fn mesh_instance(
    mesh: MeshHandle,
    material: MaterialHandle,
    transform: [[f32; 4]; 4],
) -> MeshInstance {
    let mut instance = MeshInstance::default();
    instance.mesh = mesh;
    instance.material = material;
    instance.transform = transform;
    instance
}

fn material(base_color: [f32; 4], metallic: f32, roughness: f32) -> PbrMaterial {
    let mut material = PbrMaterial::default();
    material.base_color_factor = base_color;
    material.metallic_factor = metallic;
    material.roughness_factor = roughness;
    material
}

fn torch(position: [f32; 3]) -> PointLight {
    let mut light = PointLight::default();
    light.position = position;
    light.color = [0.85, 0.55, 0.32];
    light.intensity = 6.0;
    light.range = 15.0;
    light
}

const PILLAR_COUNT: u32 = 8;
const PILLAR_RADIUS: f32 = 13.0;

/// Same cast as `pbr_stage.rs`'s arena (floor, pillars, an altar, a few "figures", torches, a key
/// light), parameterised by `t` in `0.0..1.0` (one full loop) so the camera tilt oscillates exactly
/// like the interactive example's `sin(time * 0.3)`, just driven by a frame index instead of wall
/// time.
fn arena_frame(renderer: &mut WgpuRenderer, t: f32) -> StageFrame {
    let floor = renderer
        .register_mesh(floor_tile_grid(20, 2.0))
        .expect("valid mesh");
    let pillar = renderer
        .register_mesh(octagonal_pillar(0.6, 6.0))
        .expect("valid mesh");
    let altar = renderer
        .register_mesh(altar_block(3.0, 2.0, 1.2))
        .expect("valid mesh");
    let figure = renderer
        .register_mesh(capsule_actor(0.6, 1.8, 12, 3))
        .expect("valid mesh");

    let mut camera = Camera25D::default();
    camera.target = [0.0, 2.0];
    camera.tilt_degrees = 67.5 + 7.5 * (t * std::f32::consts::TAU).sin();
    camera.fov_y_degrees = 50.0;
    camera.distance = 34.0;

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.02, 0.024, 0.036, 1.0];
    frame.camera_25d = Some(camera);

    let mut key_light = DirectionalLight::default();
    key_light.direction = [0.35, 0.5, -0.8];
    key_light.color = [0.60, 0.69, 0.85];
    key_light.intensity = 5.5;
    frame.key_light = Some(key_light);
    frame.ambient = AmbientLight::Hemisphere {
        sky_color: [0.16, 0.18, 0.24],
        ground_color: [0.06, 0.055, 0.06],
        intensity: 1.0,
    };
    frame.shadow_config = {
        let mut config = ShadowConfig::default();
        config.mode = ShadowMode::KeyLight;
        config
    };

    frame
        .materials
        .push(material([0.20, 0.21, 0.24, 1.0], 0.0, 0.85)); // 0: floor/pillars
    frame
        .materials
        .push(material([0.11, 0.09, 0.095, 1.0], 0.0, 0.6)); // 1: altar
    frame
        .materials
        .push(material([0.30, 0.27, 0.25, 1.0], 0.0, 0.7)); // 2: figures

    frame.meshes.push(mesh_instance(
        floor,
        MaterialHandle(0),
        translation([0.0, 0.0, 0.0]),
    ));
    frame.meshes.push(mesh_instance(
        altar,
        MaterialHandle(1),
        translation([0.0, 0.0, 0.6]),
    ));

    for i in 0..PILLAR_COUNT {
        let angle = i as f32 / PILLAR_COUNT as f32 * std::f32::consts::TAU;
        let (x, y) = (PILLAR_RADIUS * angle.cos(), PILLAR_RADIUS * angle.sin());
        frame.meshes.push(mesh_instance(
            pillar,
            MaterialHandle(0),
            translation([x, y, 3.0]),
        ));
        frame.point_lights.push(torch([x * 0.82, y * 0.82, 4.2]));
    }

    for &[x, y] in &[
        [-4.0, 6.0],
        [4.5, 4.0],
        [-2.5, -5.0],
        [3.0, 10.0],
        [1.0, -6.5],
    ] {
        frame.meshes.push(mesh_instance(
            figure,
            MaterialHandle(2),
            translation([x, y, 1.5]),
        ));
    }

    frame
}

/// Produces the WP2.8 milestone-showcase PNG sequence (plan 0002 WP2.8 step 4): `frame_0000.png`
/// through `frame_{FRAME_COUNT - 1:04}.png` under [`out_dir`], for `.github/workflows/ci.yml`'s
/// `showcase-gif` job to assemble into a GIF with `ffmpeg`.
#[test]
#[ignore = "WP2.8 milestone showcase: run explicitly (see this file's module doc comment)"]
fn render_pbr_stage_png_sequence() {
    let Some(mut renderer) = offscreen_renderer() else {
        eprintln!("no GPU adapter available; skipping the WP2.8 pbr_stage showcase");
        return;
    };
    let dir = out_dir();
    std::fs::create_dir_all(&dir).expect("create output directory");

    for index in 0..FRAME_COUNT {
        let t = index as f32 / FRAME_COUNT as f32;
        let frame = arena_frame(&mut renderer, t);
        renderer.render_stage(&frame).expect("render_stage");
        let rgba = renderer.read_offscreen_rgba().expect("read-back");
        let image = Image::from_offscreen(WIDTH, HEIGHT, rgba);
        let path = dir.join(format!("frame_{index:04}.png"));
        image.write_png(&path).expect("write frame PNG");
    }

    println!(
        "grimoire-pbr-stage-showcase: frames={FRAME_COUNT} size={WIDTH}x{HEIGHT} platform={} dir={}",
        support::platform_dir(),
        dir.display()
    );
}
