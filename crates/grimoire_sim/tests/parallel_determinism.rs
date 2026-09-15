//! Parallel determinism gate (engine ADR-0006, building block 7).
//!
//! The parallel scenario (`tests/parallel_scenario/mod.rs`) uses multi-member stages, deferred
//! writes, block random streams, `f32` reductions over blocks and structural stage boundaries.
//! Its isolated-stage run with the sequential executor defines the golden final hash; grouped
//! stages under the sequential, permuted and reversed executors must reproduce every checkpoint.
//! `grimoire_exec/tests/hash_gate.rs` runs the same scenario on thread pools of 1, 2 and N threads.

mod parallel_scenario;

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use grimoire_ecs::{
    Entity, Executor, PermutedExecutor, QUERY_BLOCK_SIZE, SequentialExecutor, StageMode, With,
};
use parallel_scenario::{
    GOLDEN_PARALLEL_FINAL_HASH, HASH_EVERY, Marked, POPULATION, SEED, TOTAL_TICKS,
    build_simulation, checkpoint_report, run_until,
};

/// Observations of the reference run that show the scenario exercises what it should.
struct Reference {
    hashes: Vec<(u64, u64)>,
    initial_blocks: usize,
    marked_counts: BTreeSet<usize>,
    despawns_seen: bool,
    spawns_seen: bool,
}

fn record() -> Reference {
    let mut sim = build_simulation(SEED, StageMode::Isolated);
    sim.world_mut().set_executor(Arc::new(SequentialExecutor));
    let initial_blocks = sim
        .world()
        .par_blocks::<Entity, _>(|block| block.len())
        .len();
    let mut marked_counts = BTreeSet::new();
    let mut despawns_seen = false;
    let mut spawns_seen = false;
    let hashes = run_until(&mut sim, TOTAL_TICKS, |sim| {
        if sim.tick().is_multiple_of(HASH_EVERY / 2) {
            let world = sim.world();
            marked_counts.insert(world.query::<(Entity, With<Marked>)>().count());
            for entity in world.query::<Entity>() {
                despawns_seen |= entity.generation() > 0;
                spawns_seen |= entity.index() as usize >= POPULATION;
            }
        }
    });
    Reference {
        hashes,
        initial_blocks,
        marked_counts,
        despawns_seen,
        spawns_seen,
    }
}

fn reference() -> &'static Reference {
    static REFERENCE: OnceLock<Reference> = OnceLock::new();
    REFERENCE.get_or_init(record)
}

#[test]
fn isolated_reference_matches_golden() {
    let hashes = &reference().hashes;
    let &(tick, hash) = hashes.last().expect("at least the final checkpoint");
    assert_eq!(tick, TOTAL_TICKS);
    assert_eq!(
        hash,
        GOLDEN_PARALLEL_FINAL_HASH,
        "{}",
        checkpoint_report(hashes)
    );
}

fn check_grouped(executor: Arc<dyn Executor>) {
    let mut sim = build_simulation(SEED, StageMode::Grouped);
    sim.world_mut().set_executor(executor);
    let hashes = run_until(&mut sim, TOTAL_TICKS, |_| {});
    assert_eq!(hashes, reference().hashes, "{}", checkpoint_report(&hashes));
}

#[test]
fn grouped_sequential_matches_isolated() {
    check_grouped(Arc::new(SequentialExecutor));
}

#[test]
fn grouped_permuted_matches_isolated() {
    check_grouped(Arc::new(PermutedExecutor::new(1)));
}

#[test]
fn grouped_reversed_matches_isolated() {
    check_grouped(Arc::new(PermutedExecutor::reversed()));
}

#[test]
fn scenario_exercises_blocks_and_structure() {
    let reference = reference();
    assert!(
        reference.initial_blocks >= 12,
        "only {} blocks of {QUERY_BLOCK_SIZE} rows",
        reference.initial_blocks
    );
    assert!(
        reference.marked_counts.len() > 2,
        "Marked counts barely change: {:?}",
        reference.marked_counts
    );
    assert!(reference.marked_counts.iter().any(|&count| count > 0));
    assert!(reference.despawns_seen);
    assert!(reference.spawns_seen);
    let ticks: Vec<u64> = reference.hashes.iter().map(|&(tick, _)| tick).collect();
    let expected: Vec<u64> = (1..=TOTAL_TICKS / HASH_EVERY)
        .map(|n| n * HASH_EVERY)
        .collect();
    assert_eq!(ticks, expected);
}

/// Frozen stage plan; the type path of `Heat` belongs to this test binary.
#[test]
fn stage_plan() {
    let mut sim = build_simulation(SEED, StageMode::Grouped);
    let plan: Vec<String> = sim
        .schedule_mut()
        .stages()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        plan,
        [
            "exclusive [hazard] (exclusive system)",
            "parallel [sense, heat, census] (follows an exclusive system)",
            "parallel [tag] (reads component `parallel_determinism::parallel_scenario::Heat`, written by `heat`)",
            "exclusive [steer] (exclusive system)",
            "exclusive [integrate] (exclusive system)",
            "parallel [emit] (follows an exclusive system)",
        ]
    );
}
