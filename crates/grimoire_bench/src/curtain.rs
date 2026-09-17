//! The benchmark scene "Vollvorhang" (PRD-0004 acceptance, plan 0002 WP6.6): about 10,000 live
//! bullets from a mix of every Sigil modifier type, measured tick by tick in the phases a frame
//! spends on them — simulation, extraction, collision, render preparation and the offscreen
//! `render_stage` call — for the wall-clock trend (engine ADR-0010: trend only, never a gate).
//!
//! # The scene
//!
//! Three compiled units under `fixtures/` (sources next to them, kept current by
//! `grimoire_sigilc`'s `tests/unit_fixtures.rs`): `full_curtain_flow` (ring + `accelerate`,
//! spiral + `rotate` with a time-triggered `change_type`, line + `speed_curve`),
//! `full_curtain_weave` (fan + `sine_offset` with a time-triggered `reverse`, wave + `curve`, fan +
//! `mirror` + `rotate`) and `full_curtain_shatter` (ring of heavies that `burst` into fragments
//! after six units, seeded scatter + `accelerate`). Three units, because a unit may use each drawn
//! silhouette only once (PRD-0003 rule 3). Every emitter of every unit runs forever from each of
//! [`CURTAIN_ORIGINS`]; bullets that leave the square of ±[`CURTAIN_HALF`] units despawn. After
//! [`CURTAIN_FILL_TICKS`] ticks the population is steady at about 10,000 (checked by this module's
//! tests), and the 100 dummy enemies of the collision benches stand in the same world.
//!
//! # The phases
//!
//! [`FullCurtain`] runs one phase per call, so the wall-clock binary can time each on its own:
//!
//! - [`FullCurtain::step_sim`]: one `Simulation::step` (the `sigil.*` systems).
//! - [`FullCurtain::extract`]: `grimoire::adapters::sigil_render::extract_bullets` into the stage
//!   frame, as the facade does every frame.
//! - [`FullCurtain::collide`]: what the Sigil → collision adapter of contract §9.6 will do — every
//!   live bullet in slot order with its unit's collision radius, then every enemy entity, through
//!   `rebuild_par`, one `overlapping_batch` query per enemy and the player's `graze_ring`.
//! - [`FullCurtain::prepare_render`]: the render CPU work of the engine without a GPU — the bullet
//!   pass's upload preparation and the clustered lighting's light assignment for the stage frame,
//!   through `grimoire_render::measurement` (the functions the renderer itself calls).
//! - [`FullCurtain::render`]: `render_stage` of the extracted bullets over a lit floor on an
//!   offscreen `WgpuRenderer` (the adapter `GRIMOIRE_GPU_ADAPTER` selects, `software` in CI) — the
//!   same call the facade's profiler times as the render scope — plus the GPU time the renderer's
//!   timestamp queries report. On a software adapter the rasterizer runs on the CPU inside that
//!   call, so its wall time is a trend of the whole software frame, not of the engine's render CPU
//!   work; [`FullCurtain::prepare_render`] is that. [`FullCurtain::sync_gpu`] waits for the GPU
//!   outside the timed region, so queued work never piles up across frames.

use grimoire::adapters::sigil_render::extract_bullets;
use std::time::Duration;

use grimoire::render::measurement::CpuFramePreparation;
use grimoire::render::procedural::floor_tile_grid;
use grimoire::render::{
    AmbientLight, Camera25D, DirectionalLight, LightBudget, MaterialHandle, MeshInstance,
    PbrMaterial, PointLight, RenderError, Renderer, RendererConfig, StageFrame,
    StageRendererConfig, WgpuRenderer,
};
use grimoire_collide::{
    BatchHits, Circle, Collider, ColliderKey, CollisionQuery, GridItem, Hit, Shape, ShapeQuery,
    SpatialGrid,
};
use grimoire_core::math::Vec2;
use grimoire_ecs::{Entity, Executor};
use grimoire_sigil::{
    BehaviorRegistryBuilder, BulletPool, Emitter, SigilConfig, SigilContent, SigilLibrary,
    SigilUnit, install,
};
use grimoire_sim::{Simulation, TickInput};

use crate::scenarios::{
    COLLIDE_BULLET_LAYERS, COLLIDE_ENEMIES, collide_enemies, collide_graze_ring,
    collide_grid_config,
};

/// Scenario name of the simulation phase (contract §15.1).
pub const CURTAIN_SIM_SCENARIO: &str = "curtain_sim_10k";
/// Scenario name of the extraction phase.
pub const CURTAIN_EXTRACT_SCENARIO: &str = "curtain_extract_10k";
/// Scenario name of the collision phase.
pub const CURTAIN_COLLIDE_SCENARIO: &str = "curtain_collide_10k";
/// Scenario name of the render CPU preparation (bullet upload and light clustering, no GPU).
pub const CURTAIN_RENDER_PREP_SCENARIO: &str = "curtain_render_prep_10k";
/// Scenario name of the `render_stage` call on the offscreen renderer.
pub const CURTAIN_RENDER_STAGE_SCENARIO: &str = "curtain_render_stage_10k";
/// Scenario name of the GPU time the renderer's timestamp queries report for that call.
pub const CURTAIN_RENDER_GPU_SCENARIO: &str = "curtain_render_gpu_10k";

/// Budget of the simulation phase per tick in milliseconds: the bullets' share of the P1 stress
/// test (plan 0002 success criteria, PRD-0004). Like every budget here printed next to the median
/// as a trend line; the runner is not the reference hardware, so it is never a verdict (WP6.7
/// relates runner values to the reference).
pub const CURTAIN_SIM_BUDGET_MS: f64 = 1.0;
/// Budget of the extraction phase per frame in milliseconds (render extraction ≤ 0.5 ms).
pub const CURTAIN_EXTRACT_BUDGET_MS: f64 = 0.5;
/// Budget of the collision phase per tick in milliseconds (collision ≤ 1.5 ms).
pub const CURTAIN_COLLIDE_BUDGET_MS: f64 = 1.5;
/// Budget of the render CPU preparation per frame in milliseconds: bullet upload plus clustering
/// (plan 0002 WP3.3, the share of the 3 ms render CPU budget those two steps may take).
pub const CURTAIN_RENDER_PREP_BUDGET_MS: f64 = 1.5;

/// The population the scene is tuned for.
pub const CURTAIN_BULLETS: u32 = 10_000;
/// Ticks before the population is steady; run by [`FullCurtain::build`], never measured.
pub const CURTAIN_FILL_TICKS: u32 = 600;
/// Ticks per wall-clock sample.
pub const CURTAIN_WALLCLOCK_TICKS: u32 = 30;
/// Simulation seed; the scatter block draws from it.
pub const CURTAIN_SEED: u64 = 0x0006_0006_C0A7_A1A5;
/// Half the side of the arena square; bullets beyond it despawn.
pub const CURTAIN_HALF: f32 = 24.0;
/// Where every emitter of every unit fires from.
pub const CURTAIN_ORIGINS: [Vec2; 3] = [
    Vec2::new(-10.0, 8.0),
    Vec2::new(10.0, 8.0),
    Vec2::new(0.0, -6.0),
];
/// Width of the offscreen target of the render phase: a quarter of 1080p per axis.
pub const CURTAIN_RENDER_WIDTH: u32 = 480;
/// Height of the offscreen target of the render phase.
pub const CURTAIN_RENDER_HEIGHT: u32 = 270;

/// Pool capacity: well above the steady population, so no spawn is ever dropped.
const CURTAIN_CAPACITY: u32 = 16_384;

/// The compiled units.
const UNITS: [&[u8]; 3] = [
    include_bytes!("../fixtures/full_curtain_flow_unit_v1.bin"),
    include_bytes!("../fixtures/full_curtain_weave_unit_v1.bin"),
    include_bytes!("../fixtures/full_curtain_shatter_unit_v1.bin"),
];

/// The full-curtain benchmark state: the simulation, the collision working memory, the stage
/// frame the extraction fills, and the offscreen renderer if one could be created.
pub struct FullCurtain {
    sim: Simulation,
    /// Collision radius per library unit and bullet type, read once from the content.
    collision_radii: Vec<Vec<f32>>,
    grid: SpatialGrid,
    items: Vec<GridItem>,
    queries: Vec<ShapeQuery>,
    batch: BatchHits,
    graze: Vec<Hit>,
    stage: StageFrame,
    preparation: CpuFramePreparation,
    renderer: Option<WgpuRenderer>,
}

/// What one [`FullCurtain::render`] call reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurtainFrame {
    /// Bullets the bullet pass drew.
    pub bullets_drawn: u32,
    /// GPU time of the frame from the renderer's timestamp queries, when the adapter has them and
    /// a measurement has completed.
    pub gpu_time: Option<Duration>,
}

impl FullCurtain {
    /// Builds the scene, runs [`CURTAIN_FILL_TICKS`] ticks and, with `with_renderer`, creates the
    /// offscreen renderer. A missing adapter leaves [`Self::has_renderer`] false.
    ///
    /// # Errors
    /// A renderer creation error other than [`RenderError::NoAdapter`].
    ///
    /// # Panics
    /// Only if the checked-in units stop decoding or installing, which this module's tests catch.
    pub fn build(with_renderer: bool) -> Result<Self, RenderError> {
        let units: Vec<SigilUnit> = UNITS
            .iter()
            .map(|bytes| SigilUnit::from_bytes(bytes).expect("full-curtain unit decodes"))
            .collect();
        let emitters: Vec<_> = units
            .iter()
            .flat_map(|unit| (0..unit.emitter_count()).map(move |index| (unit.id(), index)))
            .collect();
        let registry = BehaviorRegistryBuilder::new(1).build();
        let library =
            SigilLibrary::new(units, registry.clone()).expect("full-curtain library builds");
        let mut sim = Simulation::new(CURTAIN_SEED);
        install(
            &mut sim,
            library,
            registry,
            SigilConfig::new(
                CURTAIN_CAPACITY,
                Vec2::new(-CURTAIN_HALF, -CURTAIN_HALF),
                Vec2::new(CURTAIN_HALF, CURTAIN_HALF),
            ),
        )
        .expect("full-curtain install succeeds");
        for (index, &origin) in CURTAIN_ORIGINS.iter().enumerate() {
            for &(unit, emitter) in &emitters {
                sim.world_mut().spawn((Emitter {
                    unit,
                    emitter,
                    origin,
                    rotation: index as f32 * 0.7,
                    started_at: 0,
                },));
            }
        }
        for collider in collide_enemies() {
            sim.world_mut().spawn((collider,));
        }
        for _ in 0..CURTAIN_FILL_TICKS {
            sim.step(TickInput::default());
        }
        let collision_radii = sim
            .world()
            .resource::<SigilContent>()
            .expect("installed content")
            .library()
            .units()
            .iter()
            .map(|unit| {
                unit.bullet_types()
                    .iter()
                    .map(|bullet_type| bullet_type.collision_radius)
                    .collect()
            })
            .collect();

        let mut curtain = Self {
            sim,
            collision_radii,
            grid: SpatialGrid::new(collide_grid_config()).expect("valid bench grid"),
            items: Vec::with_capacity(CURTAIN_CAPACITY as usize + COLLIDE_ENEMIES as usize),
            queries: Vec::with_capacity(COLLIDE_ENEMIES as usize),
            batch: BatchHits::default(),
            graze: Vec::new(),
            stage: lit_stage(),
            preparation: CpuFramePreparation::new(),
            renderer: None,
        };
        if with_renderer {
            curtain.create_renderer()?;
        }
        Ok(curtain)
    }

    /// Creates the offscreen renderer and describes the lit floor under the bullets.
    fn create_renderer(&mut self) -> Result<(), RenderError> {
        let mut config = StageRendererConfig::default();
        config.base = RendererConfig {
            vsync: false,
            initial_sprite_capacity: 16,
            allow_software_fallback: true,
        };
        let mut renderer = match WgpuRenderer::new_offscreen_staged(
            CURTAIN_RENDER_WIDTH,
            CURTAIN_RENDER_HEIGHT,
            config,
        ) {
            Ok(renderer) => renderer,
            Err(RenderError::NoAdapter) => return Ok(()),
            Err(error) => return Err(error),
        };
        let floor = renderer
            .register_mesh(floor_tile_grid(24, 2.0))
            .expect("procedural floor mesh is valid");
        let mut floor_instance = MeshInstance::default();
        floor_instance.mesh = floor;
        floor_instance.material = MaterialHandle(0);
        self.stage.meshes.push(floor_instance);
        self.renderer = Some(renderer);
        Ok(())
    }

    /// Render CPU preparation of the stage frame without a GPU: the bullet pass's upload
    /// preparation and the light clustering at the renderer's default budget. Returns the bytes
    /// the bullet upload would write.
    pub fn prepare_render(&mut self) -> usize {
        let (_, bytes) = self.preparation.bullet_upload(&self.stage);
        let aspect = CURTAIN_RENDER_WIDTH as f32 / CURTAIN_RENDER_HEIGHT as f32;
        self.preparation
            .light_clustering(&self.stage, LightBudget::default(), aspect);
        bytes
    }

    /// Whether [`Self::build`] created an offscreen renderer.
    #[must_use]
    pub fn has_renderer(&self) -> bool {
        self.renderer.is_some()
    }

    /// One line naming the renderer's adapter, if there is a renderer.
    #[must_use]
    pub fn adapter_report_line(&self) -> Option<String> {
        self.renderer
            .as_ref()
            .map(WgpuRenderer::adapter_report_line)
    }

    /// Live bullets in the pool.
    #[must_use]
    pub fn live(&self) -> u32 {
        self.pool().len()
    }

    /// Spawns the pool dropped since the start because it was full.
    #[must_use]
    pub fn dropped_spawns(&self) -> u64 {
        self.pool().dropped_spawns()
    }

    fn pool(&self) -> &BulletPool {
        self.sim
            .world()
            .resource::<BulletPool>()
            .expect("installed pool")
    }

    /// The simulation.
    #[must_use]
    pub fn simulation(&self) -> &Simulation {
        &self.sim
    }

    /// Phase 1: one simulation tick.
    pub fn step_sim(&mut self) {
        self.sim.step(TickInput::default());
    }

    /// Phase 2: extracts the live bullets into the stage frame and returns how many.
    pub fn extract(&mut self) -> u32 {
        self.stage.bullets.clear();
        extract_bullets(self.sim.world(), 1.0, &mut self.stage.bullets).extracted
    }

    /// Phase 3: rebuilds the collision grid from the pool and the enemies on `executor`, answers
    /// one query per enemy and the player's graze ring, and returns the hits found.
    pub fn collide(&mut self, executor: &dyn Executor) -> u64 {
        self.items.clear();
        self.queries.clear();
        let world = self.sim.world();
        let pool = world.resource::<BulletPool>().expect("installed pool");
        let columns = pool.columns();
        let slots = columns
            .alive
            .iter()
            .zip(columns.generation)
            .zip(columns.unit)
            .zip(columns.bullet_type)
            .zip(columns.position)
            .enumerate();
        for (slot, ((((&alive, &generation), &unit), &bullet_type), &position)) in slots {
            if !alive {
                continue;
            }
            self.items.push(GridItem {
                key: ColliderKey::pool(slot as u32, generation),
                shape: Shape::Circle(Circle {
                    center: position,
                    radius: self.collision_radii[usize::from(unit)][usize::from(bullet_type)],
                }),
                layers: COLLIDE_BULLET_LAYERS,
            });
        }
        for (entity, collider) in world.query::<(Entity, &Collider)>() {
            self.items.push(GridItem {
                key: ColliderKey::from_entity(entity),
                shape: collider.shape,
                layers: collider.layers,
            });
            self.queries.push(ShapeQuery {
                shape: collider.shape,
                mask: COLLIDE_BULLET_LAYERS,
            });
        }
        self.grid.rebuild_par(executor, self.items.iter().copied());
        self.grid
            .overlapping_batch(executor, &self.queries, &mut self.batch);
        self.grid.graze_ring(
            &collide_graze_ring(),
            COLLIDE_BULLET_LAYERS,
            &mut self.graze,
        );
        (0..self.batch.len())
            .map(|index| self.batch.hits(index).len() as u64)
            .sum::<u64>()
            + self.graze.len() as u64
    }

    /// Entries of the collision grid after the last [`Self::collide`].
    #[must_use]
    pub fn grid_len(&self) -> usize {
        self.grid.len()
    }

    /// Renders the stage frame offscreen, or returns `None` without a renderer. Does not wait for
    /// the GPU; see [`Self::sync_gpu`].
    pub fn render(&mut self) -> Option<Result<CurtainFrame, RenderError>> {
        let renderer = self.renderer.as_mut()?;
        Some(
            renderer
                .render_stage(&self.stage)
                .map(|stats| CurtainFrame {
                    bullets_drawn: stats.bullets_drawn,
                    gpu_time: stats.gpu_time,
                }),
        )
    }

    /// Waits until the GPU has finished the last frame (a readback of the offscreen target), so
    /// the next timed [`Self::render`] never pays for queued work. Does nothing without a renderer.
    ///
    /// # Errors
    /// The renderer's readback error.
    pub fn sync_gpu(&mut self) -> Result<(), RenderError> {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.read_offscreen_rgba()?;
        }
        Ok(())
    }
}

/// The stage frame without its floor mesh: camera, key light, ambient light, the floor material
/// and 16 point lights. The floor mesh instance joins once a renderer registered the mesh; the CPU
/// preparation needs no mesh.
fn lit_stage() -> StageFrame {
    let mut stage = StageFrame::new();
    let mut camera = Camera25D::default();
    camera.target = [0.0, 0.0];
    camera.tilt_degrees = 62.0;
    camera.fov_y_degrees = 50.0;
    camera.distance = 48.0;
    stage.camera_25d = Some(camera);
    let mut key_light = DirectionalLight::default();
    key_light.direction = [0.3, 0.45, -0.85];
    key_light.intensity = 0.8;
    stage.key_light = Some(key_light);
    stage.ambient = AmbientLight::Hemisphere {
        sky_color: [0.1, 0.1, 0.14],
        ground_color: [0.03, 0.03, 0.03],
        intensity: 0.5,
    };
    let mut material = PbrMaterial::default();
    material.base_color_factor = [0.4, 0.38, 0.36, 1.0];
    material.roughness_factor = 0.6;
    stage.materials.push(material);
    for index in 0..16u8 {
        let mut light = PointLight::default();
        light.position = [
            f32::from(index % 4) * 12.0 - 18.0,
            f32::from(index / 4) * 12.0 - 18.0,
            1.5,
        ];
        light.color = [1.0, 0.6, 0.3];
        light.intensity = 4.0;
        light.range = 8.0;
        stage.point_lights.push(light);
    }
    stage
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use grimoire_ecs::{PermutedExecutor, SequentialExecutor};

    use super::*;

    #[test]
    fn full_curtain_holds_ten_thousand_bullets_of_every_type_in_all_phases() {
        let mut curtain = FullCurtain::build(false).expect("no renderer requested");
        assert!(!curtain.has_renderer());
        for _ in 0..40 {
            curtain.step_sim();
            let live = curtain.live();
            assert!(
                (9_600..=10_400).contains(&live),
                "{live} live bullets, expected about {CURTAIN_BULLETS}"
            );
            assert_eq!(curtain.extract(), live, "every visual maps");
            assert_eq!(
                curtain.prepare_render(),
                live as usize * std::mem::size_of::<grimoire::render::BulletInstance>(),
                "every extracted bullet is prepared for upload"
            );
            assert!(curtain.collide(&SequentialExecutor) > 0);
            assert_eq!(curtain.grid_len(), live as usize + COLLIDE_ENEMIES as usize);
        }
        assert_eq!(curtain.dropped_spawns(), 0);

        // Every bullet type of all three units is alive at once, including burst fragments one
        // cascade level deep and the embers the spiral grains turn into.
        let columns = curtain.pool().columns();
        let types: BTreeSet<_> = columns
            .alive
            .iter()
            .enumerate()
            .filter(|&(_, &alive)| alive)
            .map(|(slot, _)| (columns.unit[slot], columns.bullet_type[slot]))
            .collect();
        assert_eq!(types.len(), 9, "{types:?}");
        assert!(
            columns
                .alive
                .iter()
                .zip(columns.cascade)
                .any(|(&alive, &depth)| alive && depth > 0),
            "burst fragments are alive"
        );
    }

    #[test]
    fn full_curtain_is_deterministic_and_executor_independent() {
        let mut first = FullCurtain::build(false).expect("no renderer requested");
        let mut second = FullCurtain::build(false).expect("no renderer requested");
        for _ in 0..5 {
            first.step_sim();
            second.step_sim();
            assert_eq!(
                first.simulation().state_hash(),
                second.simulation().state_hash()
            );
            assert_eq!(
                first.collide(&SequentialExecutor),
                second.collide(&PermutedExecutor::reversed())
            );
        }
    }

    /// The render phase draws every extracted bullet. Needs an adapter; skipped without one
    /// unless `GRIMOIRE_REQUIRE_GPU_ADAPTER=1`.
    #[test]
    fn full_curtain_render_phase_draws_every_extracted_bullet() {
        let mut curtain = FullCurtain::build(true).expect("renderer creation");
        if !curtain.has_renderer() {
            assert!(
                !matches!(
                    std::env::var("GRIMOIRE_REQUIRE_GPU_ADAPTER").as_deref(),
                    Ok("1" | "true")
                ),
                "no GPU adapter found although GRIMOIRE_REQUIRE_GPU_ADAPTER=1"
            );
            eprintln!("no GPU adapter; the full-curtain render phase was not exercised");
            return;
        }
        curtain.step_sim();
        let extracted = curtain.extract();
        let frame = curtain.render().expect("a renderer").expect("render_stage");
        assert_eq!(frame.bullets_drawn, extracted);
        curtain.sync_gpu().expect("readback");
    }
}
