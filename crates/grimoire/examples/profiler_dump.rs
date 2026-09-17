//! Console example (plan 0002 WP6.3): profiles a small simulation with the facade's profiler and
//! prints the recorded frames as CSV or JSON.
//!
//! No window and no GPU: the example drives its own loop over the same building blocks the main
//! loop of `App::run` uses — the platform's `SystemClock`, `Simulation::step_observed` with the
//! profiler's schedule observer, extraction into a `StageFrame`, a `NullRenderer` — so it also
//! shows how a game with its own loop gets a profile. Without timestamp queries the `gpu` scope
//! is a marked estimate (`estimate = true`).
//!
//! ```text
//! cargo run -p grimoire --example profiler_dump                 # CSV on stdout
//! cargo run -p grimoire --example profiler_dump -- --json       # JSON on stdout
//! GRIMOIRE_EXAMPLE_MAX_FRAMES=600 cargo run -p grimoire --example profiler_dump
//! ```
//!
//! A per-scope summary (mean and worst frame against the budget) goes to stderr.

use std::io::Write as _;
use std::process::ExitCode;
use std::time::Duration;

use grimoire::adapters::debug::{
    Profiler, SCOPE_EXTRACT, SCOPE_FRAME, SCOPE_RENDER, SCOPE_SIM, stats_frame,
};
use grimoire::debug::ProfileLog;
use grimoire::platform::{Clock, SystemClock};
use grimoire::prelude::*;
use grimoire::render::{NullRenderer, Renderer};

const DEFAULT_FRAMES: u64 = 120;
const PARTICLES: u32 = 10_000;
const ARENA: f32 = 100.0;

#[derive(Clone, Debug)]
struct Particle {
    position: Vec2,
    velocity: Vec2,
}
impl_stable_hash!(Particle { position, velocity });

/// Number of particles in the inner half of the arena, written by a parallel system's buffer.
#[derive(Clone, Debug, Default)]
struct Crowding {
    inner: u32,
}
impl_stable_hash!(Crowding { inner });

fn build(sim: &mut Simulation) {
    let mut rng = derive_rng(sim.seed(), 0, 1);
    let world = sim.world_mut();
    world.insert_resource(Crowding::default());
    for _ in 0..PARTICLES {
        let position = Vec2::new(rng.range_f32(-ARENA, ARENA), rng.range_f32(-ARENA, ARENA));
        let velocity = Vec2::new(rng.range_f32(-1.0, 1.0), rng.range_f32(-1.0, 1.0));
        world.spawn((Particle { position, velocity },));
    }
    sim.schedule_mut()
        // Named like an engine subsystem: recorded under the `sigil` scope and its budget.
        .add_system(system_fn("sigil.move_particles", |world| {
            for particle in world.query_mut::<&mut Particle>() {
                particle.position += particle.velocity;
                if particle.position.x.abs() > ARENA {
                    particle.velocity.x = -particle.velocity.x;
                }
                if particle.position.y.abs() > ARENA {
                    particle.velocity.y = -particle.velocity.y;
                }
            }
        }))
        // A parallel stage: its task phase and buffer application go to `collide`.
        .add_parallel_system(parallel_system_fn(
            "collide.count_inner",
            Access::new()
                .read::<Particle>()
                .write_resource::<Crowding>(),
            |world, commands| {
                let inner = world
                    .query::<&Particle>()
                    .filter(|particle| particle.position.length_squared() < ARENA * ARENA * 0.25)
                    .count();
                let inner = u32::try_from(inner).unwrap_or(u32::MAX);
                commands.insert_resource(Crowding { inner });
            },
        ))
        // No prefix: recorded under `app`.
        .add_system(system_fn("report", |_world| {}));
}

fn frames_from_env() -> u64 {
    std::env::var("GRIMOIRE_EXAMPLE_MAX_FRAMES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_FRAMES)
}

fn main() -> ExitCode {
    let json = std::env::args()
        .skip(1)
        .any(|argument| argument == "--json");
    let frames = frames_from_env();

    let clock = SystemClock::new();
    let mut sim = Simulation::new(0x5EED);
    build(&mut sim);
    let mut renderer = NullRenderer::default();
    let mut stage = StageFrame::new();
    let mut profiler = Profiler::default();
    let mut log = ProfileLog::new();
    let mut last = clock.elapsed();

    for frame in 0..frames {
        let frame_start = clock.elapsed();
        let frame_time = frame_start.saturating_sub(last);
        last = frame_start;
        profiler.begin_frame(frame);

        let started = clock.elapsed();
        sim.step_observed(TickInput::default(), &mut profiler.observer(&clock));
        profiler.record(SCOPE_SIM, clock.elapsed().saturating_sub(started));

        profiler.measure(&clock, SCOPE_EXTRACT, || {
            stage.clear();
            for particle in sim.world().query::<&Particle>() {
                stage.base.sprites.push(SpriteInstance {
                    position: particle.position.to_array(),
                    half_size: [0.5, 0.5],
                    shape: shape::CIRCLE,
                    color: [0.9, 0.3, 0.6, 1.0],
                    ..SpriteInstance::default()
                });
            }
        });

        let started = clock.elapsed();
        let render = match renderer.render_stage(&stage) {
            Ok(stats) => stats,
            Err(error) => {
                eprintln!("rendering failed: {error}");
                return ExitCode::FAILURE;
            }
        };
        let render_time = clock.elapsed().saturating_sub(started);
        profiler.record(SCOPE_RENDER, render_time);
        profiler.record_stage_stats(&render, render_time);
        let inner = sim.world().resource::<Crowding>().map_or(0, |c| c.inner);
        profiler.add_counter("particles_inner", u64::from(inner));
        profiler.record(SCOPE_FRAME, clock.elapsed().saturating_sub(frame_start));

        let stats = FrameStats {
            frame,
            sim_tick: sim.tick(),
            ticks_this_frame: 1,
            alpha: 0.0,
            frame_time,
            fps: 0.0,
            dropped_time: Duration::ZERO,
            render: render.base,
        };
        log.push(profiler.profile().to_stats(&stats_frame(&stats, None)));
    }

    print_summary(&log);
    let mut stdout = std::io::stdout().lock();
    let written = if json {
        log.write_json(&mut stdout)
    } else {
        log.write_csv(&mut stdout)
    };
    if let Err(error) = written.map(|()| stdout.flush()) {
        eprintln!("export failed: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Mean and worst frame per scope, against the scope's budget, on stderr.
fn print_summary(log: &ProfileLog) {
    let Some(first) = log.frames().first() else {
        eprintln!("no frames recorded");
        return;
    };
    eprintln!(
        "{:<10} {:>12} {:>12} {:>12}  frames over budget",
        "scope", "mean", "worst", "budget"
    );
    for scope in &first.scopes {
        let totals: Vec<u64> = log
            .frames()
            .iter()
            .filter_map(|frame| frame.scopes.iter().find(|row| row.scope == scope.scope))
            .map(|row| row.total_ns)
            .collect();
        let count = u64::try_from(totals.len()).unwrap_or(u64::MAX).max(1);
        let mean = Duration::from_nanos(totals.iter().sum::<u64>() / count);
        let worst = Duration::from_nanos(totals.iter().copied().max().unwrap_or(0));
        let over = if scope.budget_ns == 0 {
            String::from("-")
        } else {
            totals
                .iter()
                .filter(|&&total| total > scope.budget_ns)
                .count()
                .to_string()
        };
        let budget = if scope.budget_ns == 0 {
            String::from("-")
        } else {
            format!("{:?}", Duration::from_nanos(scope.budget_ns))
        };
        let marker = if scope.estimate { " (estimate)" } else { "" };
        eprintln!(
            "{:<10} {:>12} {:>12} {:>12}  {over}{marker}",
            scope.name,
            format!("{mean:?}"),
            format!("{worst:?}"),
            budget
        );
    }
}
