//! Contract §11.7 hash gate, executor part (Plan 0002 WP5.2): the pool scenario hashes identically
//! with `SequentialExecutor` and `PermutedExecutor` (seeds 1 and 2, reversed). The thread-pool part
//! (1, 2 and N threads) runs the same scenario in `grimoire_exec/tests/hash_gate.rs`.

mod pool_scenario;

use std::sync::Arc;

use grimoire_ecs::{Executor, PermutedExecutor, QUERY_BLOCK_SIZE, SequentialExecutor};

fn run_with(executor: Arc<dyn Executor>) -> (Vec<(u64, u64)>, u32) {
    let mut sim = pool_scenario::build_simulation(pool_scenario::SEED);
    sim.world_mut().set_executor(executor);
    let mut max_slots = 0;
    let checkpoints = pool_scenario::run_until(&mut sim, pool_scenario::TOTAL_TICKS, |sim| {
        max_slots = max_slots.max(pool_scenario::slot_count(sim));
    });
    (checkpoints, max_slots)
}

#[test]
fn pool_scenario_is_independent_of_the_executor() {
    let (reference, max_slots) = run_with(Arc::new(SequentialExecutor));
    assert!(
        max_slots as usize > 3 * QUERY_BLOCK_SIZE,
        "the gate needs more than three pool blocks, reached {max_slots} slots"
    );
    for executor in [
        Arc::new(PermutedExecutor::new(1)) as Arc<dyn Executor>,
        Arc::new(PermutedExecutor::new(2)),
        Arc::new(PermutedExecutor::reversed()),
    ] {
        assert_eq!(run_with(executor).0, reference);
    }
    let final_hash = reference.last().map(|&(_, hash)| hash);
    assert_eq!(
        final_hash,
        Some(pool_scenario::GOLDEN_POOL_FINAL_HASH),
        "pool scenario golden changed; if intentional, GOLDEN_POOL_FINAL_HASH = {:#018x}",
        final_hash.unwrap_or_default()
    );
}
