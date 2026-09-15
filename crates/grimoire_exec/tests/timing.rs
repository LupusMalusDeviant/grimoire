//! Manual timing checks of the parallel scheduler (engine ADR-0006, PRD-0002 budget).
//!
//! Run only in a measurement session, in release:
//! `cargo test -p grimoire_exec --release --test timing -- --ignored --nocapture --test-threads 1`

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use grimoire_core::impl_stable_hash;
use grimoire_ecs::{
    Access, Executor, Schedule, SequentialExecutor, System, World, parallel_system_fn, system_fn,
};
use grimoire_exec::ThreadPoolExecutor;
use grimoire_sim::{Simulation, TickInput};

#[derive(Clone)]
struct Pos {
    x: f32,
    y: f32,
}
impl_stable_hash!(Pos { x, y });

#[derive(Clone)]
struct Vel {
    x: f32,
    y: f32,
}
impl_stable_hash!(Vel { x, y });

const ENTITIES: usize = 10_000;
const ROUNDS: u32 = 2_000;

fn time(rounds: u32, mut f: impl FnMut()) -> Duration {
    let start = Instant::now();
    for _ in 0..rounds {
        f();
    }
    start.elapsed()
}

fn world(entities: usize) -> World {
    let mut world = World::new();
    for i in 0..entities {
        let f = i as f32;
        world.spawn((Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 }));
    }
    world
}

fn ns_per_entity(elapsed: Duration, entities: usize) -> f64 {
    elapsed.as_nanos() as f64 / (f64::from(ROUNDS) * entities as f64)
}

#[test]
#[ignore = "manual release timing"]
fn blocks_against_plain_queries() {
    let mut plain = world(ENTITIES);
    let query = time(ROUNDS, || {
        for (pos, vel) in plain.query_mut::<(&mut Pos, &Vel)>() {
            pos.x += vel.x;
            pos.y += vel.y;
        }
    });
    let run_blocks = |executor: Arc<dyn Executor>| {
        let mut world = world(ENTITIES);
        world.set_executor(executor);
        time(ROUNDS, || {
            world.par_blocks_mut::<(&mut Pos, &Vel), _>(|block| {
                for (pos, vel) in block {
                    pos.x += vel.x;
                    pos.y += vel.y;
                }
            });
        })
    };
    let sequential = run_blocks(Arc::new(SequentialExecutor));
    let one = run_blocks(Arc::new(ThreadPoolExecutor::new(1).expect("pool")));
    let four = run_blocks(Arc::new(ThreadPoolExecutor::new(4).expect("pool")));
    println!(
        "query_mut {:.2} ns/entity, blocks sequential {:.2}, pool 1 {:.2}, pool 4 {:.2}",
        ns_per_entity(query, ENTITIES),
        ns_per_entity(sequential, ENTITIES),
        ns_per_entity(one, ENTITIES),
        ns_per_entity(four, ENTITIES)
    );
    black_box(&plain);
    assert!(
        sequential.as_secs_f64() <= query.as_secs_f64() * 1.10,
        "sequential blocks {sequential:?} exceed 1.10 × query_mut {query:?}"
    );
}

fn movers(world: &mut World) {
    for (pos, vel) in world.query_mut::<(&mut Pos, &Vel)>() {
        pos.x += vel.x * 0.016;
        pos.y += vel.y * 0.016;
        if pos.x * pos.x + pos.y * pos.y > 2_500.0 {
            pos.x *= 0.5;
            pos.y *= 0.5;
        }
    }
}

/// Fastest of `samples`, which is robust against one-off spikes of the machine.
fn fastest(samples: &[Duration]) -> Duration {
    samples.iter().copied().min().unwrap_or(Duration::ZERO)
}

#[test]
#[ignore = "manual release timing"]
fn stepping_with_the_default_executor_costs_no_more_than_a_direct_loop() {
    const MOVERS: usize = 500;
    const STEPS: u32 = 20_000;
    /// Timed blocks per variant. One sample each was dominated by run-to-run noise above the
    /// 5 % margin, so both variants are warmed up, alternate their order and compare their
    /// fastest block.
    const BLOCKS: usize = 11;

    let mut sim = Simulation::new(1);
    for i in 0..MOVERS {
        let f = i as f32 * 0.1;
        sim.world_mut()
            .spawn((Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 }));
    }
    sim.schedule_mut().add_system(system_fn("movers", movers));
    let mut step = || time(STEPS, || sim.step(TickInput::default()));

    let mut direct_world = world(MOVERS);
    let mut systems: Vec<Box<dyn System>> = vec![Box::new(system_fn("movers", movers))];
    let mut direct = || {
        time(STEPS, || {
            direct_world.insert_resource(grimoire_sim::Tick(0));
            direct_world.insert_resource(grimoire_sim::SimSeed(1));
            direct_world.insert_resource(TickInput::default());
            for system in &mut systems {
                system.run(&mut direct_world);
            }
        })
    };

    black_box(step());
    black_box(direct());
    let mut stepped = Vec::with_capacity(BLOCKS);
    let mut direct_samples = Vec::with_capacity(BLOCKS);
    for block in 0..BLOCKS {
        if block % 2 == 0 {
            stepped.push(step());
            direct_samples.push(direct());
        } else {
            direct_samples.push(direct());
            stepped.push(step());
        }
    }
    let (stepped, direct) = (fastest(&stepped), fastest(&direct_samples));
    println!(
        "Simulation::step {stepped:?}, direct loop {direct:?} for {STEPS} steps (fastest of {BLOCKS} blocks)"
    );
    assert!(
        stepped.as_secs_f64() <= direct.as_secs_f64() * 1.05,
        "step {stepped:?} exceeds 1.05 × direct loop {direct:?} (fastest of {BLOCKS} blocks)"
    );
}

#[test]
#[ignore = "manual release timing"]
fn dispatch_overhead_of_a_two_system_stage() {
    const RUNS: u32 = 20_000;
    let mut world = world(16);
    world.set_executor(Arc::new(ThreadPoolExecutor::new(4).expect("pool")));
    let mut schedule = Schedule::new();
    for name in ["left", "right"] {
        schedule.add_parallel_system(parallel_system_fn(
            name,
            Access::new().read::<Pos>(),
            |world, _| {
                black_box(world.query::<&Pos>().count());
            },
        ));
    }
    let elapsed = time(RUNS, || schedule.run(&mut world));
    println!(
        "two-system stage on a 4-thread pool: {:.1} µs per run",
        elapsed.as_secs_f64() * 1e6 / f64::from(RUNS)
    );
}
