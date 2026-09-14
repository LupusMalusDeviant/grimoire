//! Integration example: a deterministic 60 Hz simulation driving instanced sprites.
//!
//! 5,000 entities move on spirals whose centres drift towards a player dot steered with WASD or
//! the arrow keys. Rendering interpolates between the previous and the current tick with `alpha`;
//! the window title shows the FPS. `Escape` exits, `GRIMOIRE_EXAMPLE_MAX_FRAMES=<n>` exits after
//! `n` frames.
//!
//! ```text
//! GRIMOIRE_WINDOW_MONITOR=secondary GRIMOIRE_WINDOW_FOCUS=0 cargo run -p grimoire --example sim_loop
//! ```

use std::sync::Arc;

use grimoire::prelude::*;

const TITLE: &str = "Grimoire sim_loop";
const SWARM_SIZE: u32 = 5_000;
const SEED: u64 = 0x5EED;
const TICK_RATE_HZ: u32 = 60;
/// Seconds per tick; simulation code scales by this constant, never by measured time.
const DT: f32 = 1.0 / TICK_RATE_HZ as f32;
const WORLD_HEIGHT: f32 = 200.0;
const ARENA_HALF: Vec2 = Vec2::new(170.0, 95.0);
const PLAYER_SPEED: f32 = 60.0;
/// Fraction of the distance to the player a spiral centre covers per tick.
const CENTER_PULL: f32 = 0.002;
const SPAWN_STREAM: u64 = 1;

/// Position after the most recent tick.
#[derive(Clone, Debug)]
struct Position {
    at: Vec2,
}
impl_stable_hash!(Position { at });

/// Position one tick earlier, the start point of render interpolation.
#[derive(Clone, Debug)]
struct PreviousPosition {
    at: Vec2,
}
impl_stable_hash!(PreviousPosition { at });

#[derive(Clone, Debug)]
struct Spiral {
    center: Vec2,
    angle: f32,
    angular_speed: f32,
    base_radius: f32,
    breath: f32,
    breath_speed: f32,
    hue: f32,
}
impl_stable_hash!(Spiral {
    center,
    angle,
    angular_speed,
    base_radius,
    breath,
    breath_speed,
    hue
});

impl Spiral {
    /// The radius breathes between 20 % and 100 % of the base radius, so entities spiral in and out.
    fn position(&self) -> Vec2 {
        let radius = self.base_radius * (0.6 + 0.4 * dmath::sin(self.breath));
        self.center + Vec2::from_angle(self.angle) * radius
    }
}

#[derive(Clone, Debug)]
struct Player;
impl_stable_hash!(Player {});

/// Keeps an angle in `[0, TAU)` so it never loses precision over long runs.
fn wrap_angle(angle: f32) -> f32 {
    if angle >= dmath::TAU {
        angle - dmath::TAU
    } else if angle < 0.0 {
        angle + dmath::TAU
    } else {
        angle
    }
}

fn remember_previous(world: &mut World) {
    for (previous, position) in world.query_mut::<(&mut PreviousPosition, &Position)>() {
        previous.at = position.at;
    }
}

fn steer_player(world: &mut World) {
    let input = world.resource::<TickInput>().copied().unwrap_or_default();
    let slot = input.slots[0];
    let direction = Vec2::new(slot.axis(0), slot.axis(1)).normalize_or_zero();
    for (position, _) in world.query_mut::<(&mut Position, &Player)>() {
        let moved = position.at + direction * (PLAYER_SPEED * DT);
        position.at = Vec2::new(
            moved.x.clamp(-ARENA_HALF.x, ARENA_HALF.x),
            moved.y.clamp(-ARENA_HALF.y, ARENA_HALF.y),
        );
    }
}

fn move_swarm(world: &mut World) {
    let player = world
        .query::<(&Position, &Player)>()
        .next()
        .map_or(Vec2::ZERO, |(position, _)| position.at);
    for (position, spiral) in world.query_mut::<(&mut Position, &mut Spiral)>() {
        spiral.angle = wrap_angle(spiral.angle + spiral.angular_speed * DT);
        spiral.breath = wrap_angle(spiral.breath + spiral.breath_speed * DT);
        spiral.center = spiral.center.lerp(player, CENTER_PULL);
        position.at = spiral.position();
    }
}

#[derive(Default)]
struct Swarm {
    window: Option<Arc<dyn PlatformWindow>>,
    shown_fps: f64,
}

impl GamePlugin for Swarm {
    fn name(&self) -> &str {
        "swarm"
    }

    fn build(&mut self, sim: &mut Simulation) {
        let mut rng = derive_rng(sim.seed(), 0, SPAWN_STREAM);
        let world = sim.world_mut();
        for _ in 0..SWARM_SIZE {
            let direction = if rng.chance(0.5) { 1.0 } else { -1.0 };
            let spiral = Spiral {
                center: Vec2::new(
                    rng.range_f32(-ARENA_HALF.x, ARENA_HALF.x),
                    rng.range_f32(-ARENA_HALF.y, ARENA_HALF.y),
                ),
                angle: rng.range_f32(0.0, dmath::TAU),
                angular_speed: direction * rng.range_f32(0.4, 2.4),
                base_radius: rng.range_f32(6.0, 40.0),
                breath: rng.range_f32(0.0, dmath::TAU),
                breath_speed: rng.range_f32(0.2, 1.2),
                hue: rng.next_f32(),
            };
            let at = spiral.position();
            world.spawn((Position { at }, PreviousPosition { at }, spiral));
        }
        world.spawn((
            Position { at: Vec2::ZERO },
            PreviousPosition { at: Vec2::ZERO },
            Player,
        ));

        sim.schedule_mut()
            .add_system(system_fn("remember_previous", remember_previous))
            .add_system(system_fn("steer_player", steer_player))
            .add_system(system_fn("move_swarm", move_swarm));
    }

    fn extract(&mut self, world: &World, alpha: f32, frame: &mut RenderFrame) {
        frame.clear_color = [0.01, 0.01, 0.02, 1.0];
        frame.camera = Camera2D {
            center: [0.0, 0.0],
            world_height: WORLD_HEIGHT,
        };
        for (previous, position, spiral) in world.query::<(&PreviousPosition, &Position, &Spiral)>()
        {
            frame.sprites.push(SpriteInstance {
                position: previous.at.lerp(position.at, alpha).to_array(),
                half_size: [0.8, 0.8],
                rotation: 0.0,
                shape: shape::CIRCLE,
                color: [
                    spiral.hue,
                    0.25 + 0.5 * (1.0 - spiral.hue),
                    1.0 - spiral.hue,
                    0.85,
                ],
            });
        }
        for (previous, position, _) in world.query::<(&PreviousPosition, &Position, &Player)>() {
            frame.sprites.push(SpriteInstance {
                position: previous.at.lerp(position.at, alpha).to_array(),
                half_size: [2.5, 2.5],
                rotation: 0.0,
                shape: shape::CIRCLE,
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
    }

    fn on_frame(&mut self, stats: &FrameStats) {
        // FPS changes once per measurement window; setting the title more often costs time.
        if stats.fps == self.shown_fps {
            return;
        }
        self.shown_fps = stats.fps;
        if let Some(window) = &self.window {
            window.set_title(&format!(
                "{TITLE} - {} entities - {:.0} FPS - tick {}",
                SWARM_SIZE + 1,
                stats.fps,
                stats.sim_tick
            ));
        }
    }

    fn window_created(&mut self, window: &Arc<dyn PlatformWindow>) {
        self.window = Some(Arc::clone(window));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let mut app = App::new(WindowConfig {
        title: String::from(TITLE),
        ..WindowConfig::default()
    })
    .seed(SEED)
    .tick_rate(TICK_RATE_HZ)
    .renderer_config(RendererConfig {
        allow_software_fallback: true,
        ..RendererConfig::default()
    })
    .exit_key(KeyCode::Escape)
    .plugin(Swarm::default());
    if let Ok(value) = std::env::var("GRIMOIRE_EXAMPLE_MAX_FRAMES") {
        app = app.max_frames(value.parse()?);
    }
    app.run()?;
    Ok(())
}
