//! Plan 0002 WP5.3 (Sigil → Render extraction adapter) and the WP3.5 bullet pass, end to end.
//!
//! Every test runs a real, `sigilc`-compiled Sigil unit (`tests/fixtures/*.sigil`, compiled bytes
//! checked in next to them and kept current by `grimoire_sigilc`'s `tests/unit_fixtures.rs`)
//! through the interpreter of `grimoire_sigil`, then extracts the pool with
//! `grimoire::adapters::sigil_render`:
//!
//! - headless tests check the adapter itself — interpolation, visual mapping, rotation, radius,
//!   unmapped visuals, the plugin hook and that extraction never touches simulation state — and
//!   that `NullRenderer` counts what the adapter hands it;
//! - [`bullets_from_a_sigil_unit_render_over_a_lit_stage`] renders one frame of that pool over a
//!   lit 2.5D stage on the GPU (skipped without an adapter, failing instead with
//!   `GRIMOIRE_REQUIRE_GPU_ADAPTER=1`);
//! - [`render_sigil_bullet_png_sequence`] (`#[ignore]`, run explicitly) writes a short frame
//!   sequence of the same scene as numbered PNGs, interpolating between ticks with a fractional
//!   `alpha` like the main loop does:
//!   `GRIMOIRE_GPU_ADAPTER=software cargo test -p grimoire --test sigil_render --locked --
//!   --ignored --nocapture render_sigil_bullet_png_sequence`. Frames go to
//!   `GRIMOIRE_SIGIL_BULLETS_OUT_DIR` (default `target/wp35-sigil-bullets`, ignored by git).

mod support;

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use grimoire::GamePlugin;
use grimoire::adapters::sigil_render::{
    BulletExtractionStats, SigilRenderPlugin, extract_bullets, extract_pool, map_visual,
};
use grimoire::core::Vec2;
use grimoire::core::math::dmath;
use grimoire::render::procedural::{altar_block, floor_tile_grid, octagonal_pillar};
use grimoire::render::{
    AmbientLight, BULLET_PASS_PALETTE_SPACE, Camera25D, DirectionalLight, MaterialHandle,
    MeshHandle, MeshInstance, NullRenderer, PbrMaterial, PointLight, RenderError, RenderLayer,
    Renderer, RendererConfig, ShadowConfig, ShadowMode, StageFrame, WgpuRenderer,
};
use grimoire::sigil::{
    BehaviorRegistry, BehaviorRegistryBuilder, BulletPool, Emitter, SigilConfig, SigilContent,
    SigilLibrary, SigilUnit, install,
};
use grimoire::sim::{Simulation, TickInput};
use support::Image;

const SHOWCASE_UNIT: &[u8] = include_bytes!("fixtures/bullet_showcase_unit_v1.bin");
const UNMAPPED_UNIT: &[u8] = include_bytes!("fixtures/unmapped_visual_unit_v1.bin");

/// Ground point every emitter of a test fires from.
const ORIGIN: Vec2 = Vec2::new(0.0, 1.5);

/// A simulation with `unit_bytes` installed and one `Emitter` entity per emitter of the unit, all
/// at [`ORIGIN`], started at tick 0.
fn simulation_with(unit_bytes: &[u8]) -> Simulation {
    let unit = SigilUnit::from_bytes(unit_bytes).expect("checked-in fixture decodes");
    let unit_id = unit.id();
    let emitter_count = unit.emitter_count();
    let registry: Arc<BehaviorRegistry> = BehaviorRegistryBuilder::new(1).build();
    let library = SigilLibrary::new(vec![unit], registry.clone()).expect("library builds");
    let mut sim = Simulation::new(0x5EED_B011_E75E_ED00);
    install(
        &mut sim,
        library,
        registry,
        SigilConfig::new(4096, Vec2::new(-40.0, -40.0), Vec2::new(40.0, 40.0)),
    )
    .expect("install succeeds");
    for emitter in 0..emitter_count {
        sim.world_mut().spawn((Emitter {
            unit: unit_id,
            emitter,
            origin: ORIGIN,
            rotation: 0.0,
            started_at: 0,
        },));
    }
    sim
}

fn run_ticks(sim: &mut Simulation, ticks: u32) {
    for _ in 0..ticks {
        sim.step(TickInput::default());
    }
}

fn pool(sim: &Simulation) -> &BulletPool {
    sim.world().resource::<BulletPool>().expect("installed")
}

#[test]
fn extraction_interpolates_between_the_previous_and_the_current_tick() {
    let mut sim = simulation_with(SHOWCASE_UNIT);
    run_ticks(&mut sim, 45);
    let live = pool(&sim).len() as usize;
    assert!(
        live > 50,
        "the showcase unit has fired by now ({live} live)"
    );

    for alpha in [0.0, 0.25, 1.0] {
        let mut out = Vec::new();
        let stats = extract_bullets(sim.world(), alpha, &mut out);
        assert_eq!(stats.extracted as usize, live);
        assert_eq!(stats.unmapped_visual, 0);
        for (instance, bullet) in out.iter().zip(pool(&sim).iter()) {
            let previous = bullet.previous_position();
            let current = bullet.position();
            let expected = previous + (current - previous) * alpha;
            assert_eq!(instance.position, expected.to_array(), "alpha {alpha}");
        }
    }

    // At least one bullet actually moved during the last tick, so the three alphas differ.
    let moved = pool(&sim)
        .iter()
        .any(|bullet| bullet.position() != bullet.previous_position());
    assert!(moved);
}

#[test]
fn extraction_clamps_alpha_and_shows_the_current_tick_for_a_non_finite_one() {
    let mut sim = simulation_with(SHOWCASE_UNIT);
    run_ticks(&mut sim, 30);
    let extract = |alpha: f32| {
        let mut out = Vec::new();
        extract_bullets(sim.world(), alpha, &mut out);
        out
    };
    assert_eq!(extract(-3.0), extract(0.0));
    assert_eq!(extract(7.0), extract(1.0));
    assert_eq!(extract(f32::NAN), extract(1.0));
}

#[test]
fn extraction_maps_visual_radius_rotation_and_palette_space_from_the_unit() {
    let mut sim = simulation_with(SHOWCASE_UNIT);
    run_ticks(&mut sim, 60);
    let content = sim
        .world()
        .resource::<SigilContent>()
        .expect("installed")
        .clone();
    let mut out = Vec::new();
    let stats = extract_pool(pool(&sim), &content, 1.0, &mut out);
    assert_eq!(stats.extracted as usize, out.len());

    let columns = pool(&sim).columns();
    let mut silhouettes = std::collections::BTreeSet::new();
    for (instance, bullet) in out.iter().zip(pool(&sim).iter()) {
        let unit = &content.library().units()[usize::from(bullet.unit_index())];
        let bullet_type = unit.bullet_types()[usize::from(bullet.bullet_type())];
        let mapped = map_visual(bullet_type.visual).expect("showcase visuals all map");
        assert_eq!(instance.silhouette, mapped.silhouette);
        assert_eq!(instance.palette, mapped.palette);
        assert_eq!(instance.glow, bullet_type.visual.glow);
        assert_eq!(instance.radius, bullet_type.radius);
        assert_eq!(
            instance.rotation,
            columns.angle[bullet.id().index() as usize],
            "rotation is the flight direction"
        );
        assert_eq!(instance.palette_space, BULLET_PASS_PALETTE_SPACE);
        assert_eq!(instance.flags, 0);
        silhouettes.insert(instance.silhouette);
    }
    assert_eq!(
        silhouettes.len(),
        3,
        "all three silhouettes of the showcase unit are on screen"
    );

    // The renderer accepts every one of them.
    let mut stage = StageFrame::new();
    stage.bullets = out;
    let render_stats = NullRenderer::default()
        .render_stage(&stage)
        .expect("null renderer never fails");
    assert_eq!(render_stats.bullets_drawn, stats.extracted);
    assert_eq!(render_stats.bullets_rejected_invalid, 0);
    assert_eq!(render_stats.bullets_rejected_palette_space, 0);
}

#[test]
fn unmappable_visuals_are_counted_and_not_handed_to_the_renderer() {
    let mut sim = simulation_with(UNMAPPED_UNIT);
    run_ticks(&mut sim, 3);
    assert_eq!(pool(&sim).len(), 16, "four rings of four");
    let mut out = Vec::new();
    let stats = extract_bullets(sim.world(), 0.5, &mut out);
    assert_eq!(stats.extracted, 8, "orb and rice map");
    assert_eq!(
        stats.unmapped_visual, 8,
        "shard (palette 2) and wisp (silhouette 3) do not"
    );
    assert_eq!(out.len(), 8);
}

#[test]
fn extraction_reads_the_world_without_changing_any_state() {
    let mut reference = simulation_with(SHOWCASE_UNIT);
    let mut extracted = simulation_with(SHOWCASE_UNIT);
    let mut out = Vec::new();
    for _ in 0..40 {
        reference.step(TickInput::default());
        extracted.step(TickInput::default());
        out.clear();
        extract_bullets(extracted.world(), 0.5, &mut out);
        assert_eq!(extracted.state_hash(), reference.state_hash());
    }
}

#[test]
fn the_plugin_fills_the_stage_frame_and_keeps_its_counters() {
    let mut sim = simulation_with(SHOWCASE_UNIT);
    run_ticks(&mut sim, 50);
    let mut plugin = SigilRenderPlugin::new();
    assert_eq!(plugin.last_stats(), BulletExtractionStats::default());

    let mut stage = StageFrame::new();
    plugin.extract_stage(sim.world(), 0.5, &mut stage);
    assert_eq!(stage.bullets.len(), pool(&sim).len() as usize);
    assert_eq!(plugin.last_stats().extracted, pool(&sim).len());

    let stats = NullRenderer::default()
        .render_stage(&stage)
        .expect("null renderer never fails");
    assert_eq!(stats.bullets_drawn, pool(&sim).len());
    assert!(
        stats.bullet_point_lights_drawn >= 1,
        "the glowing showcase bullets yield bullet-cloud lights"
    );
}

// --- The GPU part: a real pool over a lit stage ---------------------------------------------------

fn adapter_required() -> bool {
    matches!(
        std::env::var("GRIMOIRE_REQUIRE_GPU_ADAPTER")
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true")
    )
}

fn offscreen_renderer(width: u32, height: u32) -> Option<WgpuRenderer> {
    let config = RendererConfig {
        vsync: false,
        initial_sprite_capacity: 16,
        allow_software_fallback: true,
    };
    match WgpuRenderer::new_offscreen(width, height, config) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::NoAdapter) => {
            assert!(
                !adapter_required(),
                "no GPU adapter found although GRIMOIRE_REQUIRE_GPU_ADAPTER=1"
            );
            let _ = writeln!(
                std::io::stdout(),
                "::warning title=GPU test skipped::no GPU adapter found; grimoire sigil_render rendered nothing"
            );
            None
        }
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

fn mesh_instance(mesh: MeshHandle, material: u32, transform: [[f32; 4]; 4]) -> MeshInstance {
    let mut instance = MeshInstance::default();
    instance.mesh = mesh;
    instance.material = MaterialHandle(material);
    instance.transform = transform;
    instance
}

fn material(base_color: [f32; 4], roughness: f32) -> PbrMaterial {
    let mut material = PbrMaterial::default();
    material.base_color_factor = base_color;
    material.metallic_factor = 0.0;
    material.roughness_factor = roughness;
    material
}

fn torch(position: [f32; 3]) -> PointLight {
    let mut light = PointLight::default();
    light.position = position;
    light.color = [0.85, 0.55, 0.32];
    light.intensity = 4.0;
    light.range = 9.0;
    light
}

/// Meshes of the lit stage, registered once per renderer.
struct StageMeshes {
    floor: MeshHandle,
    pillar: MeshHandle,
    altar: MeshHandle,
}

fn register_stage_meshes(renderer: &mut WgpuRenderer) -> StageMeshes {
    StageMeshes {
        floor: renderer
            .register_mesh(floor_tile_grid(14, 2.0))
            .expect("valid mesh"),
        pillar: renderer
            .register_mesh(octagonal_pillar(0.45, 4.0))
            .expect("valid mesh"),
        altar: renderer
            .register_mesh(altar_block(1.6, 1.2, 0.8))
            .expect("valid mesh"),
    }
}

const PILLARS: u32 = 6;
const PILLAR_RING: f32 = 7.5;

/// A dim ritual floor with pillars, torches, a cool key light with shadows and the tilted camera,
/// before any bullets: layers 1–3 of the frame.
fn lit_stage(meshes: &StageMeshes) -> StageFrame {
    let mut camera = Camera25D::default();
    camera.target = [ORIGIN.x, ORIGIN.y - 0.5];
    camera.tilt_degrees = 62.0;
    camera.fov_y_degrees = 50.0;
    camera.distance = 17.0;

    let mut frame = StageFrame::new();
    frame.base.clear_color = [0.02, 0.022, 0.03, 1.0];
    frame.camera_25d = Some(camera);
    let mut key_light = DirectionalLight::default();
    key_light.direction = [0.35, 0.5, -0.8];
    key_light.color = [0.60, 0.69, 0.85];
    key_light.intensity = 1.4;
    frame.key_light = Some(key_light);
    frame.ambient = AmbientLight::Hemisphere {
        sky_color: [0.14, 0.16, 0.22],
        ground_color: [0.05, 0.045, 0.05],
        intensity: 0.45,
    };
    let mut shadows = ShadowConfig::default();
    shadows.mode = ShadowMode::KeyLight;
    frame.shadow_config = shadows;

    frame
        .materials
        .push(material([0.18, 0.19, 0.22, 1.0], 0.85)); // 0: floor, pillars
    frame.materials.push(material([0.10, 0.08, 0.09, 1.0], 0.6)); // 1: altar
    frame
        .meshes
        .push(mesh_instance(meshes.floor, 0, translation([0.0, 0.0, 0.0])));
    frame.meshes.push(mesh_instance(
        meshes.altar,
        1,
        translation([ORIGIN.x, ORIGIN.y, 0.4]),
    ));
    for i in 0..PILLARS {
        let angle = i as f32 / PILLARS as f32 * dmath::TAU + 0.5;
        let (x, y) = (
            ORIGIN.x + PILLAR_RING * dmath::cos(angle),
            ORIGIN.y + PILLAR_RING * dmath::sin(angle),
        );
        frame
            .meshes
            .push(mesh_instance(meshes.pillar, 0, translation([x, y, 2.0])));
        frame.point_lights.push(torch([x * 0.85, y * 0.85, 3.0]));
    }
    frame
}

/// Number of pixels whose colour differs by more than `threshold` in any channel.
fn changed_pixels(a: &[u8], b: &[u8], threshold: u8) -> usize {
    let (a, _) = a.as_chunks::<4>();
    let (b, _) = b.as_chunks::<4>();
    a.iter()
        .zip(b)
        .filter(|(pa, pb)| {
            pa.iter()
                .zip(pb.iter())
                .any(|(x, y)| x.abs_diff(*y) > threshold)
        })
        .count()
}

#[test]
fn bullets_from_a_sigil_unit_render_over_a_lit_stage() {
    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 90;
    let Some(mut renderer) = offscreen_renderer(WIDTH, HEIGHT) else {
        return;
    };
    let meshes = register_stage_meshes(&mut renderer);
    let mut sim = simulation_with(SHOWCASE_UNIT);
    run_ticks(&mut sim, 90);

    // The same lit stage without and with the extracted bullets.
    let stage_only = lit_stage(&meshes);
    renderer.render_stage(&stage_only).expect("render_stage");
    let without = renderer.read_offscreen_rgba().expect("read-back");

    let mut frame = lit_stage(&meshes);
    let mut plugin = SigilRenderPlugin::new();
    plugin.extract_stage(sim.world(), 0.5, &mut frame);
    let stats = renderer.render_stage(&frame).expect("render_stage");
    let with = renderer.read_offscreen_rgba().expect("read-back");

    let extracted = plugin.last_stats().extracted;
    assert!(extracted > 100, "a busy pattern ({extracted} bullets)");
    assert_eq!(stats.bullets_drawn, extracted);
    assert_eq!(stats.bullets_rejected_invalid, 0);
    assert_eq!(stats.bullets_rejected_palette_space, 0);
    assert!(stats.bullet_point_lights_drawn >= 1);
    assert_eq!(
        stats.point_lights_drawn,
        PILLARS + stats.bullet_point_lights_drawn,
        "torches plus the derived bullet-cloud lights"
    );
    assert_eq!(
        renderer.last_stage_pass_order(),
        RenderLayer::ORDER.as_slice()
    );

    // Bullets visibly change the picture: hundreds of pixels, not a stray few.
    let changed = changed_pixels(&without, &with, 40);
    assert!(
        changed > 300,
        "only {changed} of {} pixels changed with {extracted} bullets on the stage",
        WIDTH * HEIGHT
    );
}

fn out_dir() -> PathBuf {
    std::env::var_os("GRIMOIRE_SIGIL_BULLETS_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target").join("wp35-sigil-bullets"))
}

#[test]
#[ignore = "writes a PNG frame sequence; run explicitly (see this file's module docs)"]
fn render_sigil_bullet_png_sequence() {
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 360;
    /// Ticks before the first frame, so the pattern already fills the stage.
    const WARM_UP_TICKS: u32 = 70;
    /// Simulation ticks per frame: at 60 ticks per second and 24 frames per second, 2.5. The
    /// fractional part lands between ticks, so every other frame is interpolated with alpha 0.5.
    const TICKS_PER_FRAME: f32 = 2.5;
    const FRAMES: u32 = 72;

    let renderer = offscreen_renderer(WIDTH, HEIGHT);
    let mut renderer = renderer.expect("the PNG sequence needs a GPU or software adapter");
    let meshes = register_stage_meshes(&mut renderer);
    let mut sim = simulation_with(SHOWCASE_UNIT);
    run_ticks(&mut sim, WARM_UP_TICKS);

    let dir = out_dir();
    std::fs::create_dir_all(&dir).expect("output directory");
    let mut plugin = SigilRenderPlugin::new();
    let mut ticks_done = 0u32;
    for frame_index in 0..FRAMES {
        let time = frame_index as f32 * TICKS_PER_FRAME;
        let whole = time as u32;
        while ticks_done < whole {
            sim.step(TickInput::default());
            ticks_done += 1;
        }
        let alpha = time - whole as f32;

        let mut frame = lit_stage(&meshes);
        plugin.extract_stage(sim.world(), alpha, &mut frame);
        let stats = renderer.render_stage(&frame).expect("render_stage");
        let image = Image::from_offscreen(
            WIDTH,
            HEIGHT,
            renderer.read_offscreen_rgba().expect("read-back"),
        );
        let path = dir.join(format!("frame_{frame_index:04}.png"));
        image.write_png(&path).expect("write PNG");
        let _ = writeln!(
            std::io::stdout(),
            "frame {frame_index:02}: tick {ticks_done} alpha {alpha:.2} bullets {} bullet lights {} -> {}",
            stats.bullets_drawn,
            stats.bullet_point_lights_drawn,
            path.display()
        );
    }
}
