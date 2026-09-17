//! The scene of the M2 showcase `sigil_curtain` (plan 0002 WP5.7), shared by the example
//! (`main.rs` next to this file) and its offscreen test (`tests/sigil_curtain_showcase.rs`, which
//! includes this file with `#[path]`), so the GIF shows exactly what the example draws.
//!
//! One plugin, [`CurtainStage`], does everything a game would do for a Sigil pattern:
//!
//! - `build` installs the compiled unit `tests/fixtures/sigil_curtain_unit_v1.bin` (source
//!   `sigil_curtain.sigil`, whose emitter `curtain` is the reference pattern `02-spiral-curtain`)
//!   and spawns one `Emitter` entity per emitter on top of the altar;
//! - `register_assets` registers the stage meshes with the loop's renderer;
//! - `extract_stage` describes the lit 2.5D stage and hands the live bullets to the bullet pass
//!   through [`SigilRenderPlugin`], interpolated with the loop's `alpha`.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use grimoire::adapters::sigil_render::{BulletExtractionStats, SigilRenderPlugin};
use grimoire::prelude::*;
use grimoire::render::procedural::{altar_block, floor_tile_grid, octagonal_pillar};
use grimoire::render::{
    AmbientLight, DirectionalLight, MaterialHandle, MeshHandle, MeshInstance, PbrMaterial,
    PointLight, ShadowConfig, ShadowMode, StageRendererConfig,
};
use grimoire::sigil::{
    BehaviorRegistryBuilder, BulletPool, Emitter, SigilConfig, SigilLibrary, SigilUnit, install,
};
use grimoire::{PluginError, RenderAssets};

/// Window title of the example.
pub const TITLE: &str = "Grimoire sigil_curtain (WP5.7 showcase)";

/// The compiled Sigil unit of the showcase, kept current by `grimoire_sigilc`'s
/// `tests/unit_fixtures.rs`.
pub const UNIT: &[u8] = include_bytes!("../../tests/fixtures/sigil_curtain_unit_v1.bin");

/// Simulation seed. The curtain draws no randomness; the seed only makes runs comparable.
pub const SEED: u64 = 0x0005_1617_C0A7_A1A5;

/// Ground point the emitters fire from: the top of the altar in the middle of the stage.
pub const ORIGIN: Vec2 = Vec2::new(0.0, 0.0);

/// Pool capacity: the curtain keeps well under a thousand bullets alive.
const CAPACITY: u32 = 8_192;
/// Bullets leaving this square around the origin despawn (the camera sees about ±12 units).
const BOUND: f32 = 24.0;

const PILLARS: u32 = 8;
const PILLAR_RING: f32 = 10.5;

/// What the most recent frame handed to the bullet pass, shared with the owner of the app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CurtainFrame {
    /// Extraction counters of [`SigilRenderPlugin`].
    pub bullets: BulletExtractionStats,
    /// Live bullets in the pool.
    pub live: u32,
    /// Spawns the pool dropped since the start because it was full.
    pub dropped_spawns: u64,
}

/// The stage meshes, registered once per run.
#[derive(Clone, Copy)]
struct StageMeshes {
    floor: MeshHandle,
    pillar: MeshHandle,
    altar: MeshHandle,
}

/// The showcase plugin: the Sigil unit, the lit stage and the bullet extraction.
pub struct CurtainStage {
    meshes: Option<StageMeshes>,
    bullets: SigilRenderPlugin,
    last_frame: Rc<Cell<CurtainFrame>>,
    window: Option<Arc<dyn PlatformWindow>>,
    shown_fps: f64,
}

/// The showcase app: seed, a stage renderer that may fall back to a software adapter, and
/// [`CurtainStage`]. Returns the handle through which the owner reads the last frame's counters.
pub fn app(window: WindowConfig) -> (AppBuilder, Rc<Cell<CurtainFrame>>) {
    let last_frame = Rc::new(Cell::new(CurtainFrame::default()));
    let mut renderer = StageRendererConfig::default();
    renderer.base.allow_software_fallback = true;
    let app = App::new(window)
        .seed(SEED)
        .stage_renderer_config(renderer)
        .plugin(CurtainStage {
            meshes: None,
            bullets: SigilRenderPlugin::new(),
            last_frame: Rc::clone(&last_frame),
            window: None,
            shown_fps: 0.0,
        });
    (app, last_frame)
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

/// A dim ritual floor with a ring of pillars and torches, the altar in the middle, a cool key
/// light with shadows and the tilted camera: everything of the frame except the bullets.
fn describe_stage(meshes: StageMeshes, stage: &mut StageFrame) {
    let mut camera = Camera25D::default();
    camera.target = [ORIGIN.x, ORIGIN.y - 1.0];
    camera.tilt_degrees = 58.0;
    camera.fov_y_degrees = 50.0;
    camera.distance = 18.0;
    stage.camera_25d = Some(camera);
    stage.base.clear_color = [0.02, 0.022, 0.03, 1.0];

    let mut key_light = DirectionalLight::default();
    key_light.direction = [0.35, 0.5, -0.8];
    key_light.color = [0.60, 0.69, 0.85];
    key_light.intensity = 1.2;
    stage.key_light = Some(key_light);
    stage.ambient = AmbientLight::Hemisphere {
        sky_color: [0.14, 0.16, 0.22],
        ground_color: [0.05, 0.045, 0.05],
        intensity: 0.4,
    };
    let mut shadows = ShadowConfig::default();
    shadows.mode = ShadowMode::KeyLight;
    stage.shadow_config = shadows;

    stage
        .materials
        .push(material([0.18, 0.19, 0.22, 1.0], 0.85)); // 0: floor, pillars
    stage.materials.push(material([0.10, 0.08, 0.09, 1.0], 0.6)); // 1: altar
    stage
        .meshes
        .push(mesh_instance(meshes.floor, 0, translation([0.0, 0.0, 0.0])));
    stage.meshes.push(mesh_instance(
        meshes.altar,
        1,
        translation([ORIGIN.x, ORIGIN.y, 0.4]),
    ));
    for i in 0..PILLARS {
        let angle = i as f32 / PILLARS as f32 * dmath::TAU + 0.4;
        let (x, y) = (
            ORIGIN.x + PILLAR_RING * dmath::cos(angle),
            ORIGIN.y + PILLAR_RING * dmath::sin(angle),
        );
        stage
            .meshes
            .push(mesh_instance(meshes.pillar, 0, translation([x, y, 2.0])));
        stage.point_lights.push(torch([x * 0.85, y * 0.85, 3.0]));
    }
}

impl GamePlugin for CurtainStage {
    fn name(&self) -> &str {
        "sigil_curtain"
    }

    fn build(&mut self, sim: &mut Simulation) {
        let unit = SigilUnit::from_bytes(UNIT).expect("the checked-in unit decodes");
        let (unit_id, emitters) = (unit.id(), unit.emitter_count());
        let registry = BehaviorRegistryBuilder::new(1).build();
        let library = SigilLibrary::new(vec![unit], Arc::clone(&registry))
            .expect("the unit needs no behaviours");
        install(
            sim,
            library,
            registry,
            SigilConfig::new(
                CAPACITY,
                Vec2::new(ORIGIN.x - BOUND, ORIGIN.y - BOUND),
                Vec2::new(ORIGIN.x + BOUND, ORIGIN.y + BOUND),
            ),
        )
        .expect("Sigil installs once");
        for emitter in 0..emitters {
            sim.world_mut().spawn((Emitter {
                unit: unit_id,
                emitter,
                origin: ORIGIN,
                rotation: 0.0,
                started_at: 0,
            },));
        }
    }

    fn register_assets(&mut self, assets: &mut dyn RenderAssets) -> Result<(), PluginError> {
        self.meshes = Some(StageMeshes {
            floor: assets.register_mesh(floor_tile_grid(16, 2.0))?,
            pillar: assets.register_mesh(octagonal_pillar(0.45, 4.0))?,
            altar: assets.register_mesh(altar_block(1.6, 1.2, 0.8))?,
        });
        Ok(())
    }

    fn extract_stage(&mut self, world: &World, alpha: f32, stage: &mut StageFrame) {
        if let Some(meshes) = self.meshes {
            describe_stage(meshes, stage);
        }
        self.bullets.extract_stage(world, alpha, stage);
        let pool = world.resource::<BulletPool>();
        self.last_frame.set(CurtainFrame {
            bullets: self.bullets.last_stats(),
            live: pool.map_or(0, BulletPool::len),
            dropped_spawns: pool.map_or(0, BulletPool::dropped_spawns),
        });
    }

    fn on_frame(&mut self, stats: &FrameStats) {
        // FPS changes once per measurement window; setting the title more often costs time.
        if stats.fps == self.shown_fps {
            return;
        }
        self.shown_fps = stats.fps;
        if let Some(window) = &self.window {
            window.set_title(&format!(
                "{TITLE} - {} bullets - {:.0} FPS",
                self.last_frame.get().bullets.extracted,
                stats.fps
            ));
        }
    }

    fn window_created(&mut self, window: &Arc<dyn PlatformWindow>) {
        self.window = Some(Arc::clone(window));
    }
}
