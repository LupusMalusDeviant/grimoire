//! Downstream callers: P0-style code from main-branch patterns compiled against the mirrored P1
//! traits, plus external implementations of the P1 traits (additivity and implementability).
#![allow(dead_code, clippy::all)]

use std::borrow::Cow;
use std::cell::RefCell;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use grimoire::FrameStats;
use grimoire_core::Vec2;
use grimoire_core::math::dmath;
use grimoire_ecs::{World, system_fn};
use grimoire_render::{Camera2D, RenderError, RenderFrame, RenderStats, SpriteInstance, shape};
use grimoire_sim::Simulation;

// ---- P0 renderer (copy of grimoire/src/main_loop.rs ScriptedRenderer) against the P1 trait -----

struct ScriptedRenderer {
    calls: u64,
    fail_on: u64,
    error: fn() -> RenderError,
    resizes: Vec<(u64, u32, u32)>,
}

impl grimoire_render_p1::Renderer for ScriptedRenderer {
    fn resize(&mut self, width: u32, height: u32) {
        self.resizes.push((self.calls, width, height));
    }

    fn render(&mut self, _frame: &RenderFrame) -> Result<RenderStats, RenderError> {
        let call = self.calls;
        self.calls += 1;
        if call == self.fail_on {
            return Err((self.error)());
        }
        Ok(RenderStats {
            draw_calls: 1,
            ..RenderStats::default()
        })
    }

    fn backend_name(&self) -> &str {
        "Scripted"
    }
}

fn drive(renderer: &mut dyn grimoire_render_p1::Renderer) -> Result<grimoire_render_p1::StageStats, RenderError> {
    let frame = grimoire_render_p1::StageFrame::new();
    renderer.render_stage(&frame)
}

// Renderer outside grimoire_render that overrides render_stage and fills StageStats.
struct CountingRenderer;

impl grimoire_render_p1::Renderer for CountingRenderer {
    fn resize(&mut self, _width: u32, _height: u32) {}
    fn render(&mut self, _frame: &RenderFrame) -> Result<RenderStats, RenderError> {
        Ok(RenderStats::default())
    }
    fn backend_name(&self) -> &str {
        "Counting"
    }
    fn supports_stage(&self) -> bool {
        true
    }
    fn render_stage(&mut self, frame: &grimoire_render_p1::StageFrame) -> Result<grimoire_render_p1::StageStats, RenderError> {
        let mut stats = grimoire_render_p1::StageStats::default();
        stats.base = self.render(&frame.base)?;
        stats.bullets_drawn = frame.bullets.len() as u32;
        Ok(stats)
    }
}

// ---- P0 plugins (grimoire main_loop tests, an example game plugin) against the P1 GamePlugin -------

#[derive(Default)]
struct Frames(Rc<RefCell<Vec<FrameStats>>>);

impl grimoire_p1::GamePlugin for Frames {
    fn name(&self) -> &str {
        "frames"
    }

    fn on_frame(&mut self, stats: &FrameStats) {
        self.0.borrow_mut().push(*stats);
    }
}

struct ExampleGame {
    seed: Option<u64>,
}

impl grimoire_p1::GamePlugin for ExampleGame {
    fn name(&self) -> &str {
        "example_game"
    }

    fn build(&mut self, sim: &mut Simulation) {
        let seed = sim.seed();
        self.seed = Some(seed);
        sim.schedule_mut()
            .add_system(system_fn("remember_previous", |_world: &mut World| {}));
    }

    fn extract(&mut self, _world: &World, _alpha: f32, frame: &mut RenderFrame) {
        frame.clear_color = [0.03, 0.01, 0.02, 1.0];
        frame.camera = Camera2D {
            center: [0.0, 0.0],
            world_height: 40.0,
        };
        let angle = dmath::TAU / 8.0;
        frame.sprites.push(SpriteInstance {
            position: (Vec2::ZERO + Vec2::from_angle(angle)).to_array(),
            half_size: [1.2, 1.2],
            rotation: angle,
            shape: shape::QUAD,
            color: [0.45, 0.08, 0.1, 1.0],
        });
    }
}

fn frame_stats_literal() -> FrameStats {
    FrameStats {
        frame: 0,
        sim_tick: 0,
        ticks_this_frame: 0,
        alpha: 0.0,
        frame_time: Duration::ZERO,
        fps: 0.0,
        dropped_time: Duration::ZERO,
        render: RenderStats::default(),
    }
}

// ---- P0-style FileSystem implementation (three methods) against the P1 trait ------------------

struct ReadOnlyFs(Vec<u8>);

impl grimoire_platform_p1::FileSystem for ReadOnlyFs {
    fn read(&self, _path: &Path) -> io::Result<Vec<u8>> {
        Ok(self.0.clone())
    }
    fn write_atomic(&self, _path: &Path, _bytes: &[u8]) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
    fn exists(&self, _path: &Path) -> bool {
        true
    }
}

fn limited(fs: &dyn grimoire_platform_p1::FileSystem) -> io::Result<Vec<u8>> {
    fs.read_limited(Path::new("x"), 16)
}

// ---- P0-style SimError handling against the extended enum --------------------------------------

fn describe(error: &grimoire_sim_p1::SimError) -> &'static str {
    match error {
        grimoire_sim_p1::SimError::BadMagic => "magic",
        grimoire_sim_p1::SimError::UnsupportedVersion(_) => "version",
        _ => "other",
    }
}

// ---- External implementations of P1 traits ----------------------------------------------------

struct Recorder(Vec<String>);

impl grimoire_ecs_p1::SystemObserver for Recorder {
    fn system_finished(&mut self, system: grimoire_ecs_p1::SystemInfo<'_>, world: &World) {
        self.0.push(format!("{}:{}", system.name, world.entity_count()));
    }
}

fn observe(sim: &mut Simulation) {
    use grimoire_sim_p1::SimulationP1;
    let mut recorder = Recorder(Vec::new());
    sim.step_observed(grimoire_sim::TickInput::default(), &mut recorder);
}

struct GameQuery(Vec<grimoire_collide::GridItem>);

impl grimoire_collide::CollisionQuery for GameQuery {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn overlapping(&self, shape: &grimoire_collide::Shape, mask: grimoire_collide::LayerMask, out: &mut Vec<grimoire_collide::Hit>) {
        out.clear();
        for item in &self.0 {
            if item.layers.intersects(mask) && grimoire_collide::overlaps(&item.shape, shape) {
                out.push(grimoire_collide::Hit {
                    key: item.key,
                    layers: item.layers,
                });
            }
        }
    }
    fn graze_ring(&self, _ring: &grimoire_collide::GrazeRing, _mask: grimoire_collide::LayerMask, out: &mut Vec<grimoire_collide::Hit>) {
        out.clear();
    }
}

struct LoopbackTransport(Vec<grimoire_debug::Frame>);

impl grimoire_debug::DebugTransport for LoopbackTransport {
    fn poll(&mut self, inbox: &mut Vec<grimoire_debug::Frame>) -> Result<(), grimoire_debug::TransportError> {
        inbox.append(&mut self.0);
        Ok(())
    }
    fn send(&mut self, frame: &grimoire_debug::Frame) -> Result<(), grimoire_debug::TransportError> {
        if frame.payload.len() > 4 {
            return Err(grimoire_debug::TransportError::Io {
                kind: io::ErrorKind::Other,
                message: String::new(),
            });
        }
        self.0.push(frame.clone());
        Ok(())
    }
    fn is_connected(&self) -> bool {
        true
    }
    fn disconnect(&mut self) {}
}

// Game-side harness constructs the run result for compare (§15.2); tests change fields afterwards.
fn golden_run() -> grimoire_bench::GoldenRun {
    let mut run = grimoire_bench::GoldenRun::new(1, 60, 60, grimoire_sim_p1::ContentManifestHash::EMPTY, true, Vec::new());
    run.algorithms.sim_rng += 1;
    run
}

fn query_is_empty(query: &dyn grimoire_collide::CollisionQuery) -> bool {
    query.is_empty()
}

// A game-owned emitter is built by struct literal (§11.4 "vom Spiel erzeugte Komponenten").
fn game_emitter() -> grimoire_sigil::Emitter {
    grimoire_sigil::Emitter {
        unit: grimoire_sigil::UnitId(7),
        emitter: 0,
        origin: Vec2::ZERO,
        rotation: 0.0,
        started_at: 0,
    }
}

// ---- Former defects (fixed) and boundaries that must keep failing (own features) ---------------

/// §12 / §2 rule 12: an AssetSource outside grimoire_assets has to return `&[AssetEntry]`.
/// Fixed: external sources build entries with `AssetEntry::new`.
mod asset_entry {
    use super::*;
    use grimoire_assets::{AssetEntry, AssetError, AssetId, AssetKind, AssetSource, Sha256};

    pub struct DirectorySource {
        entries: Vec<AssetEntry>,
    }

    impl DirectorySource {
        pub fn scan(ids: &[u64]) -> Self {
            let entries = ids
                .iter()
                .map(|id| AssetEntry::new(AssetId(*id), AssetKind::SIGIL, 1, 0, Sha256([0; 32])))
                .collect();
            Self { entries }
        }
    }

    impl AssetSource for DirectorySource {
        fn name(&self) -> &str {
            "directory"
        }
        fn entries(&self) -> &[AssetEntry] {
            &self.entries
        }
        fn read(&self, id: AssetId) -> Result<Cow<'_, [u8]>, AssetError> {
            Err(AssetError::NotFound(id))
        }
    }
}

/// §7.2: a unit test of an observer outside grimoire_ecs (facade profiler, §9.7) feeds calls.
/// Must fail: StageInfo has no public constructor (§2 rule 13 exemption).
#[cfg(feature = "expect-fail-stage-info")]
fn feed_observer(observer: &mut dyn grimoire_ecs_p1::SystemObserver) {
    let stage = grimoire_ecs_p1::StageInfo {
        index: 0,
        exclusive: true,
        first_system: 0,
        len: 1,
    };
    observer.stage_started(stage);
}

/// §9.3 step 3: the loop reads "eine Camera25D im zuletzt gerenderten StageFrame".
/// Must fail: no camera field on StageFrame before WP2.2 (§9.2).
#[cfg(feature = "expect-fail-camera-field")]
fn aim_from_stage(stage: &grimoire_render_p1::StageFrame) -> Option<[i16; 2]> {
    let camera = stage.camera.as_ref()?;
    grimoire_p1::sample_aim(camera, [0.0, 0.0], [1.0, 1.0], Vec2::ZERO)
}

fn unused() {
    let _ = (drive, limited, describe, observe, golden_run, game_emitter, frame_stats_literal, query_is_empty);
    let _: Option<Cow<'static, [u8]>> = None;
}
